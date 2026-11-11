//! The device-management orchestration service.

use std::collections::HashSet;
use std::sync::Arc;

use thiserror::Error;
use uuid::Uuid;

use crate::contexts::device_management::application::registry::DeviceDriverRegistry;
use crate::contexts::device_management::application::remux::RemuxSource;
use crate::contexts::device_management::application::resolver::{
    DeviceResolveError, DeviceResolver,
};
use crate::contexts::device_management::application::rtsp_clip::{ClipStream, RtspClipSource};
use crate::contexts::device_management::application::rtsp_live::{
    LiveStreamError, LiveStreamOptions, MjpegStream, RtspLiveSource,
};
use crate::contexts::device_management::application::rtsp_still::RtspStillSource;
use crate::contexts::device_management::application::write::{
    PreviewAction, PreviewItem, WriteOp, WritePreview,
};
use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, Channel, ClipEncoding, DeviceDriver, DeviceInfo, DiskStatus,
    DriverError, ExportRequest, NewUser, PtzCommand, PtzSupport, RecordSpan, RecordingStatus,
    SnapshotRef, StreamTransport, UserAccount, UserUpdate,
};

/// A failure from a device-management operation.
#[derive(Debug, Error)]
pub enum DeviceServiceError {
    /// The target could not be resolved to a device.
    #[error(transparent)]
    Resolve(#[from] DeviceResolveError),
    /// The device driver failed.
    #[error(transparent)]
    Driver(#[from] DriverError),
    /// The write request was structurally invalid (e.g. a blank user name).
    #[error("invalid device write request: {0}")]
    InvalidRequest(String),
    /// Every live-restream slot is taken; retry later (HTTP 429).
    #[error("all live-stream slots are busy")]
    LiveBusy,
}

/// Resolves connection targets and runs management operations.
///
/// The service itself carries no write gating: authorization for the
/// device-mutating operations (PTZ, user provisioning/removal) is an RBAC
/// decision (`cctv` resource) made at the route layer BEFORE the service is
/// invoked, exactly like every other authorized endpoint.
#[derive(Clone)]
pub struct DeviceManagementService {
    resolver: Arc<dyn DeviceResolver>,
    registry: DeviceDriverRegistry,
    /// Optional RTSP-keyframe grabber used to synthesize a still for families
    /// whose native `snapshot()` is unsupported but which expose a stream.
    /// `None` disables the fallback (the driver's `Unsupported` surfaces as-is).
    rtsp_still: Option<Arc<dyn RtspStillSource>>,
    /// Optional live MJPEG restreamer over the driver's RTSP `stream_uri()`.
    /// `None` disables the live plane (`Unsupported`).
    rtsp_live: Option<Arc<dyn RtspLiveSource>>,
    /// Optional clip exporter over the driver's RTSP `playback_uri()`.
    /// `None` disables export fallback (`Unsupported`).
    rtsp_clip: Option<Arc<dyn RtspClipSource>>,
    /// Optional remuxer for the driver's native fast file download
    /// (`export_download()`). When both this and a native download are present,
    /// export uses the fast path; otherwise it falls back to `rtsp_clip`.
    remux: Option<Arc<dyn RemuxSource>>,
    /// Kill-switch for the substream-preferred live plane (see
    /// [`Self::live_mjpeg`]). Off by default (today's main-stream-only
    /// behavior is unchanged); flip with
    /// [`Self::with_live_prefer_substream`] once a per-family live spot-check
    /// has confirmed the substream plays cleanly through the restreamer.
    prefer_substream: bool,
}

impl DeviceManagementService {
    /// Construct the service from a resolver and a driver registry, with no
    /// RTSP still fallback.
    pub fn new(resolver: Arc<dyn DeviceResolver>, registry: DeviceDriverRegistry) -> Self {
        Self {
            resolver,
            registry,
            rtsp_still: None,
            rtsp_live: None,
            rtsp_clip: None,
            remux: None,
            prefer_substream: false,
        }
    }

    /// Attach an RTSP-keyframe grabber so a device that cannot answer a native
    /// still (e.g. XMeye firmware rejecting `OPSNAP`) still yields a picture
    /// from its live stream.
    #[must_use]
    pub fn with_rtsp_still(mut self, rtsp_still: Arc<dyn RtspStillSource>) -> Self {
        self.rtsp_still = Some(rtsp_still);
        self
    }

    /// Attach a live MJPEG restreamer so the console can show motion video
    /// from the driver's RTSP stream, not just stills.
    #[must_use]
    pub fn with_rtsp_live(mut self, rtsp_live: Arc<dyn RtspLiveSource>) -> Self {
        self.rtsp_live = Some(rtsp_live);
        self
    }

    /// Attach a clip exporter so families without a native export API can still
    /// export a recorded window via their RTSP playback URL.
    #[must_use]
    pub fn with_rtsp_clip(mut self, rtsp_clip: Arc<dyn RtspClipSource>) -> Self {
        self.rtsp_clip = Some(rtsp_clip);
        self
    }

    /// Attach a remuxer so a family with a native fast file download (Dahua
    /// `loadfile`) can export a recorded window several× faster than realtime,
    /// remuxing the device container to MP4 with `-c copy` instead of an
    /// RTSP-playback copy that runs at recording speed.
    #[must_use]
    pub fn with_remux(mut self, remux: Arc<dyn RemuxSource>) -> Self {
        self.remux = Some(remux);
        self
    }

    /// Toggle the substream-preferred live plane (see [`Self::live_mjpeg`]).
    /// Off by default; wire this to an env var at the composition root once a
    /// per-family live spot-check has confirmed substream playback.
    #[must_use]
    pub fn with_live_prefer_substream(mut self, enabled: bool) -> Self {
        self.prefer_substream = enabled;
        self
    }

    /// Start a live MJPEG restream of a channel's RTSP feed.
    ///
    /// A pure READ (the route layer gates it on `Read × ConnectionTarget`,
    /// exactly like `snapshot`). The returned stream owns the transcode; the
    /// consumer dropping it tears the media process down.
    ///
    /// When [`Self::with_live_prefer_substream`] has been enabled, this first
    /// tries the channel's lower-resolution substream
    /// ([`DeviceDriver::substream_uri`]) — cheaper to decode — before falling
    /// back to the main stream on any non-`Busy` failure: the substream URI is
    /// SPECULATIVE per its own doc comment, so a failure to resolve or play it
    /// just means "not available", never "the device is down". A `Busy` result
    /// from the FIRST (substream) attempt is returned immediately WITHOUT
    /// retrying the main stream: busy is a capacity fault of the shared
    /// ffmpeg-slot semaphore, not a source fault, so retrying would only
    /// observe the same busy semaphore again while spending its own timeout
    /// budget twice. Off by default (`prefer_substream = false`), so today's
    /// main-stream-only behavior is unchanged until a per-family live
    /// spot-check confirms every substream override plays cleanly. Both
    /// attempts reuse the SAME live-source first-frame/stall gate the ffmpeg
    /// adapter already enforces — no new timeout is introduced, so worst-case
    /// latency is bounded by 2× that existing timeout, not unbounded.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved, the family exposes no RTSP
    /// stream, no restreamer is configured, all restream slots are busy
    /// ([`LiveStreamError::Busy`]), or the device/transcode fails.
    pub async fn live_mjpeg(
        &self,
        target_id: Uuid,
        channel: Option<u16>,
        options: LiveStreamOptions,
    ) -> Result<MjpegStream, DeviceServiceError> {
        let Some(live) = self.rtsp_live.as_ref() else {
            return Err(DriverError::Unsupported.into());
        };
        let driver = self.driver_for(target_id).await?;
        if self.prefer_substream
            && let Some(outcome) = self
                .live_via_substream(&*driver, channel, options, live.as_ref())
                .await
        {
            return outcome;
        }
        let stream = driver.stream_uri(channel).await?;
        if stream.transport != StreamTransport::Rtsp {
            return Err(DriverError::Unsupported.into());
        }
        live.mjpeg(&stream.uri, options)
            .await
            .map_err(map_live_stream_error)
    }

    /// Try the live restream over the channel's substream (the
    /// `prefer_substream` branch of [`Self::live_mjpeg`]).
    ///
    /// Returns `None` to signal "fall back to the main stream": no substream
    /// URI resolved, the resolved URI was not RTSP, or the substream playback
    /// failed with anything other than [`LiveStreamError::Busy`]. Returns
    /// `Some(_)` to short-circuit `live_mjpeg` with a FINAL result — either the
    /// substream succeeded, or it reported `Busy`, which must never be retried
    /// against the main stream (see `live_mjpeg`'s doc comment).
    async fn live_via_substream(
        &self,
        driver: &dyn DeviceDriver,
        channel: Option<u16>,
        options: LiveStreamOptions,
        live: &dyn RtspLiveSource,
    ) -> Option<Result<MjpegStream, DeviceServiceError>> {
        let stream = driver.substream_uri(channel).await.ok()?;
        if stream.transport != StreamTransport::Rtsp {
            return None;
        }
        match live.mjpeg(&stream.uri, options).await {
            Ok(mjpeg) => Some(Ok(mjpeg)),
            Err(LiveStreamError::Busy) => Some(Err(DeviceServiceError::LiveBusy)),
            Err(LiveStreamError::Driver(_)) => None,
        }
    }

    /// Resolve the target and build its driver.
    async fn driver_for(
        &self,
        target_id: Uuid,
    ) -> Result<Box<dyn DeviceDriver>, DeviceServiceError> {
        let device = self.resolver.resolve(target_id).await?;
        self.registry.driver(target_id, &device)
    }

    /// Read the device's static capability set — the control shape the UI renders
    /// from.
    ///
    /// Resolving builds the driver but does NOT contact the device, so the
    /// capability shape is available even for an offline device (the UI can then
    /// render disabled controls rather than a blank error).
    ///
    /// # Errors
    ///
    /// Fails only if the target cannot be resolved (missing family/address/creds).
    pub async fn capabilities(&self, target_id: Uuid) -> Result<Capabilities, DeviceServiceError> {
        let driver = self.driver_for(target_id).await?;
        Ok(driver.capabilities())
    }

    /// Read device identity and firmware.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails.
    pub async fn get_info(&self, target_id: Uuid) -> Result<DeviceInfo, DeviceServiceError> {
        Ok(self.driver_for(target_id).await?.get_info().await?)
    }

    /// Enumerate the device's camera/input channels.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails (including
    /// [`DriverError::Unsupported`] for a family without channel discovery).
    pub async fn list_channels(&self, target_id: Uuid) -> Result<Vec<Channel>, DeviceServiceError> {
        Ok(self.driver_for(target_id).await?.list_channels().await?)
    }

    /// List the device's user accounts.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails.
    pub async fn list_users(
        &self,
        target_id: Uuid,
    ) -> Result<Vec<UserAccount>, DeviceServiceError> {
        Ok(self.driver_for(target_id).await?.list_users().await?)
    }

    /// Enumerate the device's own authority/permission-token catalog.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver does not expose a
    /// structural authority catalog.
    pub async fn list_authorities(
        &self,
        target_id: Uuid,
    ) -> Result<Vec<String>, DeviceServiceError> {
        Ok(self.driver_for(target_id).await?.list_authorities().await?)
    }

    /// Read the device's disk/storage health.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails.
    pub async fn disk_status(
        &self,
        target_id: Uuid,
    ) -> Result<Vec<DiskStatus>, DeviceServiceError> {
        Ok(self.driver_for(target_id).await?.disk_status().await?)
    }

    /// Read the device's recording state.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails.
    pub async fn recording_status(
        &self,
        target_id: Uuid,
    ) -> Result<RecordingStatus, DeviceServiceError> {
        Ok(self.driver_for(target_id).await?.recording_status().await?)
    }

    /// Search recorded footage for one channel over a time window.
    ///
    /// A pure metadata read: no bytes move. Distinct from
    /// [`recording_status`](Self::recording_status), which is a coarse,
    /// device-wide readout — this answers "what footage exists on channel N
    /// between these two timestamps", which an archive timeline needs.
    ///
    /// # Errors
    ///
    /// [`DeviceServiceError::InvalidRequest`] when the window is empty or
    /// inverted; otherwise fails if the target cannot be resolved or the
    /// driver call fails (including [`DriverError::Unsupported`] for a family
    /// without per-channel search).
    pub async fn search_recordings(
        &self,
        target_id: Uuid,
        request: ExportRequest,
    ) -> Result<Vec<RecordSpan>, DeviceServiceError> {
        if request.end <= request.start {
            return Err(DeviceServiceError::InvalidRequest(
                "search window end must be after start".to_owned(),
            ));
        }
        Ok(self
            .driver_for(target_id)
            .await?
            .search_recordings(&request)
            .await?)
    }

    /// Capture a still image from a channel (or the device's default channel).
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails
    /// (including [`DriverError::Unsupported`] when the device cannot snapshot).
    pub async fn snapshot(
        &self,
        target_id: Uuid,
        channel: Option<u16>,
    ) -> Result<SnapshotRef, DeviceServiceError> {
        let driver = self.driver_for(target_id).await?;
        match driver.snapshot(channel).await {
            Ok(snapshot) => Ok(snapshot),
            // The native still failed but the device may still serve video.
            // Three cases fall through to the RTSP-keyframe grabber:
            //   - `Unsupported`: the family has no native still at all (XMeye/
            //     Sofia rejecting `OPSNAP`).
            //   - `Device`/`Protocol`: the family HAS a native still endpoint
            //     but this unit answered with junk (a dahua-compatible recorder
            //     that returns an HTML page instead of a JPEG, an empty channel,
            //     a sub-stream-only camera). Its RTSP feed frequently still
            //     decodes, so a keyframe is the right recovery, not a 502.
            // `Auth`/`ClockSkew`/`Transport` are NOT retried here: bad
            // credentials and unreachable devices fail the RTSP grab the same
            // way, so falling through would only add latency to a certain
            // failure.
            Err(
                error @ (DriverError::Unsupported
                | DriverError::Device(_)
                | DriverError::Protocol(_)),
            ) => self.snapshot_via_rtsp(&*driver, channel, error).await,
            Err(error) => Err(error.into()),
        }
    }

    /// Fall back to an RTSP keyframe grab after a native `snapshot()` failure.
    ///
    /// EVERY exit here returns `native_error` — the original, informative
    /// native-snapshot failure — never a secondary error surfaced by the
    /// fallback machinery itself. A missing grabber, a confirmed-disabled
    /// channel, a failed stream-URI lookup, and a failed keyframe grab are all
    /// just "the fallback didn't pan out"; none of them is new information
    /// worth surfacing over the native error the caller can actually act on.
    async fn snapshot_via_rtsp(
        &self,
        driver: &dyn DeviceDriver,
        channel: Option<u16>,
        native_error: DriverError,
    ) -> Result<SnapshotRef, DeviceServiceError> {
        let Some(grabber) = self.rtsp_still.as_ref() else {
            return Err(native_error.into());
        };
        // Fail fast on a channel the device has positively reported disabled:
        // skip the ffmpeg grab entirely rather than paying a full
        // `still_timeout` spawn-and-die cycle for a confirmed-doomed source.
        if let Some(probed_channel) = channel
            && channel_known_disabled(driver, probed_channel).await
        {
            return Err(native_error.into());
        }
        let Ok(stream) = driver.stream_uri(channel).await else {
            return Err(native_error.into());
        };
        if stream.transport != StreamTransport::Rtsp {
            return Err(native_error.into());
        }
        let Ok(jpeg) = grabber.keyframe(&stream.uri).await else {
            return Err(native_error.into());
        };
        Ok(SnapshotRef::Bytes(jpeg))
    }

    /// Issue a pan/tilt/zoom control command against a channel.
    ///
    /// A device-mutating control action; the route layer authorizes it with
    /// `Execute × cctv` before calling in. The service then performs read-only
    /// channel discovery and calls the mutating driver method only when the exact
    /// channel reports [`PtzSupport::Supported`]. Unknown, unsupported, absent,
    /// or failed discovery all fail closed without issuing a PTZ command.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails (including
    /// [`DriverError::Unsupported`] when the device has no PTZ).
    pub async fn ptz(
        &self,
        target_id: Uuid,
        channel: u16,
        command: PtzCommand,
    ) -> Result<(), DeviceServiceError> {
        let driver = self.driver_for(target_id).await?;
        if !channel_ptz_supported(&*driver, channel).await {
            return Err(DriverError::Unsupported.into());
        }
        Ok(driver.ptz(channel, command).await?)
    }

    /// Export recorded footage from a channel over a time window.
    ///
    /// A pure READ of stored footage (not gated by the write flag). The clip is
    /// returned as inline bytes so the REST layer can stream it as a download.
    ///
    /// # Errors
    ///
    /// [`DeviceServiceError::InvalidRequest`] when the window is empty or
    /// inverted; otherwise fails if the target cannot be resolved or the driver
    /// call fails (including [`DriverError::Unsupported`] for a family without
    /// export).
    pub async fn export_clip(
        &self,
        target_id: Uuid,
        request: ExportRequest,
        encoding: ClipEncoding,
    ) -> Result<ClipStream, DeviceServiceError> {
        if request.end <= request.start {
            return Err(DeviceServiceError::InvalidRequest(
                "export window end must be after start".to_owned(),
            ));
        }
        let driver = self.driver_for(target_id).await?;
        // Fast path: a family with a native file download (Dahua `loadfile`)
        // streams the recorded window over HTTP several× faster than realtime,
        // and the remuxer repackages the device container to MP4 with `-c copy`
        // (~0 CPU). Only taken when both a native download and a remuxer exist.
        if let Some(remux) = self.remux.as_ref() {
            // Reserve the remux slot BEFORE opening the device-side download: the
            // download itself dials the device and starts an authenticated
            // session (e.g. Dahua `loadfile.cgi?action=startLoad`), which is
            // exactly the resource the concurrency cap exists to bound. Reserving
            // first means an over-the-cap request is rejected as `Busy` without
            // ever touching the device; the old order let it dial the device and
            // only THEN discover it had to be torn down.
            let reservation = match remux.try_reserve() {
                Ok(reservation) => Some(reservation),
                Err(DriverError::Busy(_)) if self.rtsp_clip.is_some() => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(reservation) = reservation {
                if let Some(download) = driver.export_download(&request).await? {
                    return Ok(remux
                        .remux_reserved(download, reservation, encoding)
                        .await?);
                }
                // No native download for this family/window: drop the reservation
                // (releasing the slot immediately) and fall through to RTSP.
                drop(reservation);
            }
        }
        // Fallback: copy the family's RTSP playback window to a STREAMING MP4 via
        // the shared ffmpeg clip plane (realtime, universal) — never buffered whole.
        let Some(clipper) = self.rtsp_clip.as_ref() else {
            return Err(DriverError::Unsupported.into());
        };
        let playback = driver.playback_uri(&request).await?;
        if playback.transport != StreamTransport::Rtsp {
            return Err(DriverError::Unsupported.into());
        }
        // Copy exactly the requested window: `-t` (with regenerated timestamps)
        // cuts the clip at the device's realtime playback, which does not
        // reliably close the RTSP session at the end time.
        let window = (request.end - request.start).num_seconds().max(1) as u64;
        let filename = format!(
            "clip-ch{ch}-{start}.mp4",
            ch = request.channel,
            start = request.start.format("%Y%m%dT%H%M%SZ"),
        );
        Ok(clipper
            .export(&playback.uri, &filename, window, encoding)
            .await?)
    }

    /// Provision users on the device.
    ///
    /// A device-mutating write; the route layer authorizes it with
    /// `Create × cctv` before calling in.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails. A
    /// per-user device rejection is reported inside the returned
    /// [`BulkOutcome`], not as an error.
    pub async fn create_users(
        &self,
        target_id: Uuid,
        users: &[NewUser],
    ) -> Result<BulkOutcome, DeviceServiceError> {
        for user in users {
            ensure_non_blank(&user.name)?;
        }
        Ok(self
            .driver_for(target_id)
            .await?
            .create_users(users)
            .await?)
    }

    /// Remove users from the device by login name.
    ///
    /// A device-mutating write; the route layer authorizes it with
    /// `Delete × cctv` before calling in.
    ///
    /// # Errors
    ///
    /// Fails if the target cannot be resolved or the driver call fails. A
    /// per-user device rejection is reported inside the returned
    /// [`BulkOutcome`], not as an error.
    pub async fn delete_users(
        &self,
        target_id: Uuid,
        usernames: &[String],
    ) -> Result<BulkOutcome, DeviceServiceError> {
        for name in usernames {
            ensure_non_blank(name)?;
        }
        Ok(self
            .driver_for(target_id)
            .await?
            .delete_users(usernames)
            .await?)
    }

    /// Edit users on the device (password/group/authorities).
    ///
    /// A device-mutating write; the route layer authorizes it with `Update × cctv`
    /// before calling in. Each update is validated as structurally sound before
    /// any device call: a blank name or a vacuous update (no facet to change) is a
    /// [`DeviceServiceError::InvalidRequest`], never a silent no-op.
    ///
    /// # Errors
    ///
    /// [`DeviceServiceError::InvalidRequest`] when an update's name is blank or it
    /// carries no change; otherwise fails if the target cannot be resolved or the
    /// driver call fails. A per-user device rejection is reported inside the
    /// returned [`BulkOutcome`], not as an error.
    pub async fn update_users(
        &self,
        target_id: Uuid,
        updates: &[UserUpdate],
    ) -> Result<BulkOutcome, DeviceServiceError> {
        for update in updates {
            ensure_non_blank(&update.name)?;
            ensure_has_changes(update)?;
        }
        Ok(self
            .driver_for(target_id)
            .await?
            .update_users(updates)
            .await?)
    }

    /// Dry-run a user-management write: report what a subsequent write would do
    /// without mutating the device.
    ///
    /// Reads the device's current user list (never writes) to classify each
    /// requested name as create-vs-skip (for a create) or delete-vs-missing (for
    /// a delete). A preview is safe for any caller who may read the target; the
    /// route layer reports separately whether the caller could carry the write
    /// out.
    ///
    /// # Errors
    ///
    /// [`DeviceServiceError::InvalidRequest`] when a requested name is blank;
    /// [`DeviceServiceError::Driver`] carrying [`DriverError::Unsupported`] when
    /// the device does not support user management; otherwise fails if the
    /// target cannot be resolved or the user-list read fails.
    pub async fn preview_users(
        &self,
        target_id: Uuid,
        op: WriteOp,
    ) -> Result<WritePreview, DeviceServiceError> {
        let driver = self.driver_for(target_id).await?;
        if !driver.capabilities().users {
            return Err(DriverError::Unsupported.into());
        }
        let per_item = match op {
            WriteOp::CreateUsers(users) => preview_create(&*driver, &users).await?,
            WriteOp::DeleteUsers(usernames) => preview_delete(&*driver, &usernames).await?,
        };
        Ok(WritePreview { per_item })
    }
}

/// Map a live-restream failure onto the service error: `Busy` maps to the
/// dedicated 429 variant, everything else passes the driver error through.
fn map_live_stream_error(error: LiveStreamError) -> DeviceServiceError {
    match error {
        LiveStreamError::Busy => DeviceServiceError::LiveBusy,
        LiveStreamError::Driver(driver_error) => driver_error.into(),
    }
}

/// Whether `channel` may receive a PTZ command.
///
/// The gate is CONFIRMED-NEGATIVE, not confirmed-positive: only a family whose
/// discovery positively proves this channel has no PTZ configuration
/// ([`PtzSupport::Unsupported`]) is refused here. A confirmed-positive gate is
/// wrong because [`PtzSupport::Supported`] is a *structural discovery* verdict
/// that only ONVIF can currently produce — the three native families report
/// [`PtzSupport::Unknown`] for every channel, so requiring `Supported` refused
/// PTZ on every native device even though their `ptz()` implementations are
/// complete.
///
/// "We do not know" therefore falls through to the driver, whose real device
/// reply is a better answer than our guess. The driver-level capability check
/// stays: a family that cannot encode PTZ at all is still refused without any
/// device I/O. Note this is a UX pre-check, never a security boundary — the
/// route has already authorized `cctv:ptz` for this target before calling in.
async fn channel_ptz_supported(driver: &dyn DeviceDriver, channel: u16) -> bool {
    if !driver.capabilities().ptz {
        return false;
    }
    if !driver.capabilities().channels {
        // No channel discovery to consult: the driver's own reply decides.
        return true;
    }
    let Ok(channels) = driver.list_channels().await else {
        // Discovery failed; it proves nothing about PTZ. Let the device answer.
        return true;
    };
    !channels.iter().any(|candidate| {
        candidate.index == channel && candidate.ptz_support == PtzSupport::Unsupported
    })
}

/// Whether channel `channel` on `driver` is CONFIRMED disabled — the family
/// supports channel discovery ([`Capabilities::channels`]), the probe
/// succeeds, and the returned channel list carries this exact index with
/// `enabled == Some(false)`. Every other case — no channel-discovery
/// capability, a probe error, the channel absent from the list, or `enabled`
/// being `None`/`Some(true)` — means "we do not know", which is NOT the same
/// as "known disabled", so this returns `false`.
async fn channel_known_disabled(driver: &dyn DeviceDriver, channel: u16) -> bool {
    if !driver.capabilities().channels {
        return false;
    }
    let Ok(channels) = driver.list_channels().await else {
        return false;
    };
    channels
        .iter()
        .any(|probed| probed.index == channel && probed.enabled == Some(false))
}

/// Classify each requested user for a create: `Skip` when the name already
/// exists on the device, `Create` otherwise. Reads the device's user list once;
/// never writes.
async fn preview_create(
    driver: &dyn DeviceDriver,
    users: &[NewUser],
) -> Result<Vec<PreviewItem>, DeviceServiceError> {
    for user in users {
        ensure_non_blank(&user.name)?;
    }
    if users.is_empty() {
        return Ok(Vec::new());
    }
    let existing = existing_names(driver).await?;
    Ok(users
        .iter()
        .map(|user| {
            if existing.contains(user.name.as_str()) {
                preview_item(
                    &user.name,
                    PreviewAction::Skip,
                    "user already exists on device",
                )
            } else {
                preview_item(
                    &user.name,
                    PreviewAction::Create,
                    "user does not exist on device",
                )
            }
        })
        .collect())
}

/// Classify each requested name for a delete: `Delete` when the name is present
/// on the device, `Missing` otherwise. Reads the device's user list once; never
/// writes.
async fn preview_delete(
    driver: &dyn DeviceDriver,
    usernames: &[String],
) -> Result<Vec<PreviewItem>, DeviceServiceError> {
    for name in usernames {
        ensure_non_blank(name)?;
    }
    if usernames.is_empty() {
        return Ok(Vec::new());
    }
    let existing = existing_names(driver).await?;
    Ok(usernames
        .iter()
        .map(|name| {
            if existing.contains(name.as_str()) {
                preview_item(name, PreviewAction::Delete, "user exists on device")
            } else {
                preview_item(name, PreviewAction::Missing, "user not present on device")
            }
        })
        .collect())
}

/// Read the device's current login names — a READ-only operation.
async fn existing_names(driver: &dyn DeviceDriver) -> Result<HashSet<String>, DeviceServiceError> {
    Ok(driver
        .list_users()
        .await?
        .into_iter()
        .map(|user| user.name)
        .collect())
}

/// Build one preview verdict.
fn preview_item(name: &str, action: PreviewAction, reason: &str) -> PreviewItem {
    PreviewItem {
        name: name.to_owned(),
        action,
        reason: reason.to_owned(),
    }
}

/// Reject a structurally invalid (blank) login name. Purely structural and
/// language-neutral: it never interprets the name's content.
fn ensure_non_blank(name: &str) -> Result<(), DeviceServiceError> {
    if name.trim().is_empty() {
        return Err(DeviceServiceError::InvalidRequest(
            "user name must not be blank".to_owned(),
        ));
    }
    Ok(())
}

/// Reject a vacuous update (identity only, nothing to change). Rejecting it here
/// keeps the driver from a call that would either no-op silently or clobber
/// fields; purely structural and language-neutral.
fn ensure_has_changes(update: &UserUpdate) -> Result<(), DeviceServiceError> {
    if !update.has_changes() {
        return Err(DeviceServiceError::InvalidRequest(
            "user update must change at least one of password, group, or authorities".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::contexts::device_management::application::resolver::ResolvedDevice;
    use crate::contexts::device_management::domain::{DeviceFamily, Secret, StreamUri};
    use crate::contexts::device_management::infrastructure::transport::{
        DeviceEndpoint, DeviceStream, DeviceTransport,
    };
    use crate::contexts::device_management::protocol::dvrip as codec;

    /// Resolver that always yields the same DVRIP device.
    struct FakeResolver {
        family: DeviceFamily,
    }

    #[async_trait]
    impl DeviceResolver for FakeResolver {
        async fn resolve(&self, _target_id: Uuid) -> Result<ResolvedDevice, DeviceResolveError> {
            Ok(ResolvedDevice {
                family: self.family,
                endpoint: DeviceEndpoint::new("device.test", 34567),
                username: "admin".to_owned(),
                password: Some(Secret::new("pw")),
                become_password: None,
            })
        }
    }

    /// Resolver that reports a missing family.
    struct MissingResolver;

    #[async_trait]
    impl DeviceResolver for MissingResolver {
        async fn resolve(&self, target_id: Uuid) -> Result<ResolvedDevice, DeviceResolveError> {
            Err(DeviceResolveError::UnknownFamily(target_id))
        }
    }

    /// Transport replaying a fixed script of JSON responses (one per request),
    /// counting `connect` calls and served requests. The counters let a test
    /// prove a code path stayed read-only, or never touched the device at all.
    struct CountingTransport {
        responses: Vec<Value>,
        connects: Arc<AtomicUsize>,
        requests: Arc<AtomicUsize>,
    }

    impl CountingTransport {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses,
                connects: Arc::new(AtomicUsize::new(0)),
                requests: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn connect_count(&self) -> usize {
            self.connects.load(Ordering::SeqCst)
        }

        fn request_count(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DeviceTransport for CountingTransport {
        async fn connect(
            &self,
            _endpoint: &DeviceEndpoint,
        ) -> Result<Box<dyn DeviceStream>, DriverError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            let responses = self.responses.clone();
            let requests = Arc::clone(&self.requests);
            tokio::spawn(async move {
                for response in responses {
                    let mut header = [0u8; codec::HEADER_LEN];
                    if server.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let declared =
                        u32::from_le_bytes([header[16], header[17], header[18], header[19]])
                            as usize;
                    let mut body = vec![0u8; declared];
                    if server.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    requests.fetch_add(1, Ordering::SeqCst);
                    let json = serde_json::to_string(&response).expect("serialize");
                    if server
                        .write_all(&codec::encode(0, 0, 0, &json))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
            Ok(Box::new(client))
        }
    }

    /// Build a service over a caller-supplied transport (so the test can read its
    /// counters afterward).
    fn service_with_transport(
        transport: Arc<CountingTransport>,
        family: DeviceFamily,
    ) -> DeviceManagementService {
        let registry = DeviceDriverRegistry::new(transport);
        DeviceManagementService::new(Arc::new(FakeResolver { family }), registry)
    }

    /// Convenience: the transport handle discarded.
    fn service_with(responses: Vec<Value>, family: DeviceFamily) -> DeviceManagementService {
        service_with_transport(Arc::new(CountingTransport::new(responses)), family)
    }

    /// A keyframe grabber that records the URL it was handed and returns a fixed
    /// JPEG (SOI marker + filler), so a test can prove the fallback fired and
    /// carried an `rtsp://` URL.
    struct RecordingStill {
        url: std::sync::Mutex<Option<String>>,
        jpeg: Vec<u8>,
    }

    impl RecordingStill {
        fn new() -> Self {
            Self {
                url: std::sync::Mutex::new(None),
                jpeg: vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10],
            }
        }
    }

    #[async_trait]
    impl RtspStillSource for RecordingStill {
        async fn keyframe(&self, rtsp_url: &str) -> Result<Vec<u8>, DriverError> {
            *self.url.lock().unwrap() = Some(rtsp_url.to_owned());
            Ok(self.jpeg.clone())
        }
    }

    /// The DVRIP wire script for a device that logs in then rejects OPSNAP with a
    /// non-OK status frame (`Ret 102`) — the XMeye "no native still" case.
    fn opsnap_reject_script() -> Vec<Value> {
        vec![
            json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"}),
            json!({"Name": "OPSNAP", "Ret": 102, "SessionID": "0x00000005"}),
        ]
    }

    /// A new user with no group and no authorities — the minimal well-formed
    /// provisioning request.
    fn new_user(name: &str, password: &str) -> NewUser {
        NewUser {
            name: name.to_owned(),
            password: Secret::new(password),
            group: None,
            authorities: Vec::new(),
        }
    }

    #[tokio::test]
    async fn get_info_resolves_and_drives_the_device() {
        let login = json!({"Ret": 100, "SessionID": "0x00000005"});
        let system_info = json!({"Ret": 100, "SystemInfo": {"HardWare": "AHB8016T-MH",
            "SoftWareVersion": "V4.03", "SerialNo": "sn123", "VideoInChannel": 16}});
        let service = service_with(vec![login, system_info], DeviceFamily::DvripSofia);

        let info = service.get_info(Uuid::nil()).await.expect("get_info");
        assert_eq!(info.model.as_deref(), Some("AHB8016T-MH"));
        assert_eq!(info.channels, Some(16));
    }

    #[tokio::test]
    async fn read_methods_preserve_driver_specific_data() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let channels = service_with(
            vec![
                login.clone(),
                json!({"Name": "ChannelTitle", "Ret": 100, "SessionID": "0x5",
                    "ChannelTitle": ["Entrance", "Back"]}),
                json!({"Name": "PTZProtocol", "Ret": 100, "SessionID": "0x5",
                    "PTZProtocol": [{"Enable": true}, {"Enable": false}]}),
            ],
            DeviceFamily::DvripSofia,
        )
        .list_channels(Uuid::nil())
        .await
        .expect("channels");
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].name.as_deref(), Some("Entrance"));
        assert_eq!(channels[0].ptz_support, PtzSupport::Supported);
        assert_eq!(channels[1].ptz_support, PtzSupport::Unsupported);

        let authorities = service_with(
            vec![
                login.clone(),
                json!({"Name": "AuthorityList", "Ret": 100, "SessionID": "0x5",
                    "AuthorityList": ["Monitor_01", "Replay_01"]}),
            ],
            DeviceFamily::DvripSofia,
        )
        .list_authorities(Uuid::nil())
        .await
        .expect("authorities");
        assert_eq!(authorities, ["Monitor_01", "Replay_01"]);

        let storage = json!({"Name": "StorageInfo", "Ret": 100, "SessionID": "0x5",
        "StorageInfo": [{"Partition": [{
            "TotalSpace": "0x00000100",
            "RemainSpace": "0x00000040",
            "Status": 0,
            "IsCurrent": true,
            "NewStartTime": "2026-06-18 19:02:25",
            "NewEndTime": "2026-07-17 10:32:28"
        }]}]});
        let disks = service_with(
            vec![login.clone(), storage.clone()],
            DeviceFamily::DvripSofia,
        )
        .disk_status(Uuid::nil())
        .await
        .expect("disks");
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].name, "disk1-1");
        assert!(disks[0].used_bytes > 0);

        let recording = service_with(vec![login, storage], DeviceFamily::DvripSofia)
            .recording_status(Uuid::nil())
            .await
            .expect("recording");
        assert!(recording.active);
        assert_eq!(recording.spans.len(), 1);
    }

    #[tokio::test]
    async fn resolve_failure_surfaces_as_service_error() {
        let registry = DeviceDriverRegistry::new(Arc::new(CountingTransport::new(vec![])));
        let service = DeviceManagementService::new(Arc::new(MissingResolver), registry);

        let error = service
            .get_info(Uuid::nil())
            .await
            .expect_err("should fail");
        assert!(matches!(
            error,
            DeviceServiceError::Resolve(DeviceResolveError::UnknownFamily(_))
        ));
    }

    #[tokio::test]
    async fn unsupported_operation_surfaces_as_driver_error() {
        // ONVIF is a view-only driver: list_users is genuinely Unsupported and
        // must surface as a driver error without touching the network.
        let service = service_with(vec![], DeviceFamily::Onvif);
        let error = service
            .list_users(Uuid::nil())
            .await
            .expect_err("should fail");
        assert!(matches!(
            error,
            DeviceServiceError::Driver(DriverError::Unsupported)
        ));
    }

    /// A family without a per-channel search primitive (the trait default)
    /// must surface `Unsupported` without touching the network — same
    /// contract as every other capability-gated read.
    #[tokio::test]
    async fn search_recordings_surfaces_unsupported_for_a_view_only_driver() {
        let service = service_with(vec![], DeviceFamily::Onvif);
        let request = ExportRequest {
            channel: 1,
            start: "2026-07-25T04:00:00Z".parse().expect("start"),
            end: "2026-07-25T05:00:00Z".parse().expect("end"),
        };

        let error = service
            .search_recordings(Uuid::nil(), request)
            .await
            .expect_err("should fail");

        assert!(matches!(
            error,
            DeviceServiceError::Driver(DriverError::Unsupported)
        ));
    }

    /// An inverted window must be rejected before any device I/O, exactly
    /// like `export_clip`'s own window validation.
    #[tokio::test]
    async fn search_recordings_rejects_an_inverted_window_before_touching_the_device() {
        let service = service_with(vec![], DeviceFamily::Onvif);
        let request = ExportRequest {
            channel: 1,
            start: "2026-07-25T05:00:00Z".parse().expect("start"),
            end: "2026-07-25T04:00:00Z".parse().expect("end"),
        };

        let error = service
            .search_recordings(Uuid::nil(), request)
            .await
            .expect_err("should fail");

        assert!(matches!(error, DeviceServiceError::InvalidRequest(_)));
    }

    // Create reaches the driver and returns a BulkOutcome.
    #[tokio::test]
    async fn create_users_reaches_driver() {
        let login = json!({"Ret": 100, "SessionID": "0x00000005"});
        let add_ok = json!({"Ret": 100, "SessionID": "0x5"});
        let transport = Arc::new(CountingTransport::new(vec![login, add_ok]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let outcome = service
            .create_users(Uuid::nil(), &[new_user("guard1", "s3cr3t")])
            .await
            .expect("create_users reaches the driver");

        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());
        assert_eq!(transport.connect_count(), 1, "the driver dialed the device");
    }

    // A blank name is rejected before any device contact.
    #[tokio::test]
    async fn create_users_rejects_a_blank_name_before_touching_the_device() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let error = service
            .create_users(Uuid::nil(), &[new_user("   ", "pw")])
            .await
            .expect_err("a blank name is invalid");

        assert!(matches!(error, DeviceServiceError::InvalidRequest(_)));
        assert_eq!(transport.connect_count(), 0);
    }

    // Delete reaches the driver and returns a BulkOutcome.
    #[tokio::test]
    async fn delete_users_reaches_driver() {
        let login = json!({"Ret": 100, "SessionID": "0x00000005"});
        let del_ok = json!({"Ret": 100, "SessionID": "0x5"});
        let transport = Arc::new(CountingTransport::new(vec![login, del_ok]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let outcome = service
            .delete_users(Uuid::nil(), &["guard1".to_owned()])
            .await
            .expect("delete_users reaches the driver");

        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());
        assert_eq!(transport.connect_count(), 1);
    }

    // A blank name is rejected before any device contact.
    #[tokio::test]
    async fn delete_users_rejects_a_blank_name_before_touching_the_device() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let error = service
            .delete_users(Uuid::nil(), &["   ".to_owned()])
            .await
            .expect_err("a blank name is invalid");

        assert!(matches!(error, DeviceServiceError::InvalidRequest(_)));
        assert_eq!(transport.connect_count(), 0);
    }

    /// A well-formed update carrying a single facet to change.
    fn user_update(name: &str, group: &str) -> UserUpdate {
        UserUpdate {
            name: name.to_owned(),
            password: None,
            group: Some(group.to_owned()),
            authorities: None,
        }
    }

    // Update reaches the driver and returns a BulkOutcome.
    #[tokio::test]
    async fn update_users_reaches_driver() {
        let login = json!({"Ret": 100, "SessionID": "0x00000005"});
        // A group-only edit is one ModifyUser frame after login.
        let modify_ok = json!({"Ret": 100, "SessionID": "0x5"});
        let transport = Arc::new(CountingTransport::new(vec![login, modify_ok]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let update = user_update("guard1", "admin");
        let outcome = service
            .update_users(Uuid::nil(), std::slice::from_ref(&update))
            .await
            .expect("update_users reaches the driver");

        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());
        assert_eq!(transport.connect_count(), 1);
    }

    // A vacuous update (nothing to change) is rejected before any device contact.
    #[tokio::test]
    async fn update_users_rejects_a_vacuous_update_before_touching_the_device() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let update = UserUpdate {
            name: "guard1".to_owned(),
            password: None,
            group: None,
            authorities: None,
        };
        let error = service
            .update_users(Uuid::nil(), std::slice::from_ref(&update))
            .await
            .expect_err("a vacuous update is invalid");

        assert!(matches!(error, DeviceServiceError::InvalidRequest(_)));
        assert_eq!(
            transport.connect_count(),
            0,
            "no device contact on structurally invalid input"
        );
    }

    // A blank name is rejected before any device contact.
    #[tokio::test]
    async fn update_users_rejects_a_blank_name_before_touching_the_device() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let update = user_update("   ", "admin");
        let error = service
            .update_users(Uuid::nil(), std::slice::from_ref(&update))
            .await
            .expect_err("a blank name is invalid");

        assert!(matches!(error, DeviceServiceError::InvalidRequest(_)));
        assert_eq!(transport.connect_count(), 0);
    }

    /// The action a preview assigned to `name`, or a panic if it is absent.
    fn action_for(preview: &WritePreview, name: &str) -> PreviewAction {
        preview
            .per_item
            .iter()
            .find(|item| item.name == name)
            .map(|item| item.action)
            .expect("preview item present")
    }

    // (c) Preview classifies a create against the device's current users: an
    // existing name is Skip, a new name is Create. It only reads (login + Users
    // query), never issuing a write frame.
    #[tokio::test]
    async fn preview_create_reports_create_and_skip_without_writing() {
        let login = json!({"Ret": 100, "SessionID": "0x00000005"});
        let users_reply = json!({"Ret": 100, "SessionID": "0x5",
            "Users": [{"Name": "existing", "Group": "user", "AuthorityList": []}]});
        let transport = Arc::new(CountingTransport::new(vec![login, users_reply]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let op = WriteOp::CreateUsers(vec![new_user("existing", "pw"), new_user("fresh", "pw")]);
        let preview = service
            .preview_users(Uuid::nil(), op)
            .await
            .expect("preview");

        assert_eq!(action_for(&preview, "existing"), PreviewAction::Skip);
        assert_eq!(action_for(&preview, "fresh"), PreviewAction::Create);

        // Read-only: one session, login + Users query only, no write frame.
        assert_eq!(transport.connect_count(), 1);
        assert_eq!(transport.request_count(), 2);
    }

    // (c) Preview classifies a delete: a present name is Delete, an absent name
    // is Missing. It only reads, never issuing a write frame.
    #[tokio::test]
    async fn preview_delete_reports_delete_and_missing_without_writing() {
        let login = json!({"Ret": 100, "SessionID": "0x00000005"});
        let users_reply = json!({"Ret": 100, "SessionID": "0x5",
            "Users": [{"Name": "present", "Group": "user", "AuthorityList": []}]});
        let transport = Arc::new(CountingTransport::new(vec![login, users_reply]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let op = WriteOp::DeleteUsers(vec!["present".to_owned(), "absent".to_owned()]);
        let preview = service
            .preview_users(Uuid::nil(), op)
            .await
            .expect("preview");

        assert_eq!(action_for(&preview, "present"), PreviewAction::Delete);
        assert_eq!(action_for(&preview, "absent"), PreviewAction::Missing);
        assert_eq!(transport.request_count(), 2, "preview is read-only");
    }

    // A device that serves video but rejects OPSNAP falls back to an RTSP
    // keyframe when a grabber is attached; the fallback bytes come back as a
    // still, and the grabber is handed an `rtsp://` URL.
    #[tokio::test]
    async fn snapshot_falls_back_to_rtsp_keyframe_when_native_still_unsupported() {
        let still = Arc::new(RecordingStill::new());
        let service = service_with(opsnap_reject_script(), DeviceFamily::DvripSofia)
            .with_rtsp_still(Arc::clone(&still) as Arc<dyn RtspStillSource>);

        let snapshot = service
            .snapshot(Uuid::nil(), None)
            .await
            .expect("rtsp fallback yields a still");
        match snapshot {
            SnapshotRef::Bytes(bytes) => assert_eq!(bytes, still.jpeg),
            other => panic!("expected fallback JPEG bytes, got {other:?}"),
        }
        let url = still
            .url
            .lock()
            .unwrap()
            .clone()
            .expect("grabber was called");
        assert!(url.starts_with("rtsp://"), "grabber got an rtsp url: {url}");
    }

    // A device whose native still endpoint answers with junk (not a JPEG) is a
    // `Device` error, not `Unsupported` — but its RTSP feed usually still
    // decodes, so the keyframe fallback must fire here too (the dahua-compatible
    // recorder returning an HTML page case).
    #[tokio::test]
    async fn snapshot_falls_back_to_rtsp_keyframe_when_native_still_returns_junk() {
        let still = Arc::new(RecordingStill::new());
        // login ok, then OPSNAP answers with a non-status, non-JPEG payload ->
        // `extract_jpeg` returns `DriverError::Device`.
        let script = vec![
            json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"}),
            json!("this is not a jpeg and not a status frame"),
        ];
        let service = service_with(script, DeviceFamily::DvripSofia)
            .with_rtsp_still(Arc::clone(&still) as Arc<dyn RtspStillSource>);

        let snapshot = service
            .snapshot(Uuid::nil(), None)
            .await
            .expect("device-error native still still falls back to rtsp");
        match snapshot {
            SnapshotRef::Bytes(bytes) => assert_eq!(bytes, still.jpeg),
            other => panic!("expected fallback JPEG bytes, got {other:?}"),
        }
        assert!(
            still.url.lock().unwrap().is_some(),
            "the rtsp grabber was called for the device-error case"
        );
    }

    // Without a grabber attached, the driver's `Unsupported` surfaces unchanged
    // (the fallback is opt-in, never silently swallowing the error).
    #[tokio::test]
    async fn snapshot_surfaces_unsupported_when_no_rtsp_grabber() {
        let service = service_with(opsnap_reject_script(), DeviceFamily::DvripSofia);

        let error = service
            .snapshot(Uuid::nil(), None)
            .await
            .expect_err("no grabber => unsupported surfaces");
        assert!(matches!(
            error,
            DeviceServiceError::Driver(DriverError::Unsupported)
        ));
    }

    // capabilities() resolves + builds the driver but never contacts the device.
    #[tokio::test]
    async fn capabilities_returns_driver_flags_without_device_io() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::Onvif);

        let caps = service
            .capabilities(Uuid::nil())
            .await
            .expect("capabilities");
        // ONVIF is the management-plane fallback: no user/disk/recording/export,
        // but Profile S gives it real channel enumeration and PTZ control.
        assert!(caps.info);
        assert!(caps.snapshot);
        assert!(caps.streaming);
        assert!(!caps.users);
        assert!(caps.ptz);
        assert!(!caps.export);
        assert!(caps.channels);
        assert_eq!(
            transport.connect_count(),
            0,
            "capabilities must not dial the device"
        );
    }

    // An inverted/empty export window is rejected before the target is resolved
    // or the device is contacted.
    #[tokio::test]
    async fn export_rejects_an_inverted_window_before_touching_the_device() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::Dahua);
        let now = chrono::Utc::now();
        let request = ExportRequest {
            channel: 1,
            start: now,
            end: now - chrono::Duration::hours(1),
        };

        // `ClipStream` is not `Debug` (it wraps a byte stream), so match the
        // result rather than `expect_err`.
        let result = service
            .export_clip(Uuid::nil(), request, ClipEncoding::Copy)
            .await;
        assert!(matches!(result, Err(DeviceServiceError::InvalidRequest(_))));
        assert_eq!(
            transport.connect_count(),
            0,
            "no device contact on an invalid window"
        );
    }

    struct BusyRemux {
        reservation_attempts: AtomicUsize,
    }

    #[async_trait]
    impl RemuxSource for BusyRemux {
        fn try_reserve(
            &self,
        ) -> Result<
            Box<dyn crate::contexts::device_management::application::remux::RemuxReservation>,
            DriverError,
        > {
            self.reservation_attempts.fetch_add(1, Ordering::SeqCst);
            Err(DriverError::Busy("test remux slot is busy".to_owned()))
        }

        async fn remux_reserved(
            &self,
            _download: crate::contexts::device_management::domain::DownloadStream,
            _reservation: Box<
                dyn crate::contexts::device_management::application::remux::RemuxReservation,
            >,
            _encoding: ClipEncoding,
        ) -> Result<ClipStream, DriverError> {
            Err(DriverError::Unsupported)
        }
    }

    struct RecordingClip {
        calls: AtomicUsize,
        last_export: std::sync::Mutex<Option<(String, u64, ClipEncoding)>>,
    }

    #[async_trait]
    impl RtspClipSource for RecordingClip {
        async fn export(
            &self,
            _rtsp_url: &str,
            filename: &str,
            max_seconds: u64,
            encoding: ClipEncoding,
        ) -> Result<ClipStream, DriverError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_export.lock().unwrap() = Some((filename.to_owned(), max_seconds, encoding));
            Ok(ClipStream {
                content_type: "video/mp4".to_owned(),
                filename: filename.to_owned(),
                bytes: Box::pin(futures_util::stream::empty()),
            })
        }
    }

    #[tokio::test]
    async fn export_falls_back_to_rtsp_when_optional_native_remux_is_busy() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let remux = Arc::new(BusyRemux {
            reservation_attempts: AtomicUsize::new(0),
        });
        let clip = Arc::new(RecordingClip {
            calls: AtomicUsize::new(0),
            last_export: std::sync::Mutex::new(None),
        });
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::Dahua)
            .with_remux(Arc::clone(&remux) as Arc<dyn RemuxSource>)
            .with_rtsp_clip(Arc::clone(&clip) as Arc<dyn RtspClipSource>);
        let start = chrono::Utc::now();
        let request = ExportRequest {
            channel: 1,
            start,
            end: start + chrono::Duration::minutes(1),
        };

        let result = service
            .export_clip(Uuid::nil(), request, ClipEncoding::Copy)
            .await;

        assert!(
            result.is_ok(),
            "optional native capacity must not block RTSP export"
        );
        assert_eq!(remux.reservation_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(clip.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            transport.connect_count(),
            0,
            "the fallback only resolves a playback URL; it does not open native download I/O"
        );
    }

    #[tokio::test]
    async fn export_enforces_capacity_and_a_one_second_floor() {
        let busy_transport = Arc::new(CountingTransport::new(vec![]));
        let busy_remux = Arc::new(BusyRemux {
            reservation_attempts: AtomicUsize::new(0),
        });
        let busy_service = service_with_transport(Arc::clone(&busy_transport), DeviceFamily::Dahua)
            .with_remux(Arc::clone(&busy_remux) as Arc<dyn RemuxSource>);
        let start = "2026-07-25T04:00:00Z".parse().expect("start");
        let request = ExportRequest {
            channel: 7,
            start,
            end: start + chrono::Duration::milliseconds(500),
        };

        let busy = busy_service
            .export_clip(Uuid::nil(), request.clone(), ClipEncoding::Copy)
            .await;
        assert!(matches!(
            busy,
            Err(DeviceServiceError::Driver(DriverError::Busy(_)))
        ));
        assert_eq!(busy_remux.reservation_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            busy_transport.connect_count(),
            0,
            "capacity rejection must happen before native device I/O"
        );

        let clip = Arc::new(RecordingClip {
            calls: AtomicUsize::new(0),
            last_export: std::sync::Mutex::new(None),
        });
        let fallback_transport = Arc::new(CountingTransport::new(vec![]));
        let fallback_service =
            service_with_transport(Arc::clone(&fallback_transport), DeviceFamily::Dahua)
                .with_rtsp_clip(Arc::clone(&clip) as Arc<dyn RtspClipSource>);

        let exported = fallback_service
            .export_clip(Uuid::nil(), request, ClipEncoding::BrowserCompatible)
            .await
            .expect("subsecond RTSP window exports");
        let observed = clip
            .last_export
            .lock()
            .unwrap()
            .clone()
            .expect("clipper observed the export");
        assert_eq!(observed.0, "clip-ch7-20260725T040000Z.mp4");
        assert_eq!(observed.1, 1, "ffmpeg must receive a non-zero duration");
        assert_eq!(observed.2, ClipEncoding::BrowserCompatible);
        assert_eq!(exported.filename, observed.0);
        assert_eq!(fallback_transport.connect_count(), 0);

        let unsupported = service_with(vec![], DeviceFamily::Dahua)
            .export_clip(
                Uuid::nil(),
                ExportRequest {
                    channel: 1,
                    start,
                    end: start + chrono::Duration::seconds(1),
                },
                ClipEncoding::Copy,
            )
            .await;
        assert!(matches!(
            unsupported,
            Err(DeviceServiceError::Driver(DriverError::Unsupported))
        ));
    }

    // A structurally invalid request (blank name) is rejected before any device
    // read.
    #[tokio::test]
    async fn preview_rejects_a_blank_user_name() {
        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        let op = WriteOp::CreateUsers(vec![new_user("   ", "pw")]);
        let error = service
            .preview_users(Uuid::nil(), op)
            .await
            .expect_err("blank name is invalid");

        assert!(matches!(error, DeviceServiceError::InvalidRequest(_)));
        assert_eq!(
            transport.request_count(),
            0,
            "no device read on structurally invalid input"
        );
    }

    #[tokio::test]
    async fn empty_user_previews_are_device_free_and_unsupported_users_fail_closed() {
        let driver = StubDriver::plain();
        assert!(preview_create(&driver, &[]).await.unwrap().is_empty());
        assert!(preview_delete(&driver, &[]).await.unwrap().is_empty());

        let transport = Arc::new(CountingTransport::new(vec![]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::Onvif);
        let error = service
            .preview_users(
                Uuid::nil(),
                WriteOp::DeleteUsers(vec!["operator".to_owned()]),
            )
            .await
            .expect_err("a view-only driver cannot preview user writes");
        assert!(matches!(
            error,
            DeviceServiceError::Driver(DriverError::Unsupported)
        ));
        assert_eq!(transport.connect_count(), 0);
    }

    /// A probe outcome for [`StubDriver::list_channels`], driving the
    /// `channel_known_disabled` unit tests without a real device round-trip.
    enum ChannelProbe {
        /// No channel-discovery capability at all (mirrors the trait's
        /// default `Unsupported` `list_channels`).
        Unsupported,
        /// The probe succeeds with a fixed channel list.
        List(Vec<Channel>),
        /// The probe itself fails (transport/device error) — "we do not
        /// know", never "known disabled".
        Error,
    }

    /// A minimal [`DeviceDriver`] double for the channel-probe and
    /// RTSP-fallback-extraction unit tests: `capabilities()` and
    /// `list_channels()` are driven explicitly, `stream_uri()` returns a fixed
    /// RTSP URL — or fails, when `stream_uri_fails` is set, to reproduce the
    /// bug-1 regression where the fallback's own URI lookup masked the
    /// original native-snapshot error — and every OTHER required trait method
    /// panics loudly if a test accidentally reaches it.
    struct StubDriver {
        capabilities: Capabilities,
        channel_probe: ChannelProbe,
        stream_uri_fails: bool,
    }

    impl StubDriver {
        /// A driver with channel discovery off and a healthy `stream_uri` —
        /// the common case for tests that only care about the RTSP fallback
        /// extraction, not channel probing.
        fn plain() -> Self {
            Self {
                capabilities: Capabilities::default(),
                channel_probe: ChannelProbe::Unsupported,
                stream_uri_fails: false,
            }
        }
    }

    #[async_trait]
    impl DeviceDriver for StubDriver {
        fn capabilities(&self) -> Capabilities {
            self.capabilities
        }

        async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn create_users(&self, _users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn delete_users(&self, _usernames: &[String]) -> Result<BulkOutcome, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn snapshot(&self, _channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
            panic!("not exercised by channel-probe tests")
        }

        async fn stream_uri(&self, _channel: Option<u16>) -> Result<StreamUri, DriverError> {
            if self.stream_uri_fails {
                return Err(DriverError::Transport("stub stream_uri failure".to_owned()));
            }
            Ok(StreamUri {
                uri: "rtsp://device.test/ch".to_owned(),
                transport: StreamTransport::Rtsp,
            })
        }

        async fn list_channels(&self) -> Result<Vec<Channel>, DriverError> {
            match &self.channel_probe {
                ChannelProbe::Unsupported => Err(DriverError::Unsupported),
                ChannelProbe::List(channels) => Ok(channels.clone()),
                ChannelProbe::Error => Err(DriverError::Transport("probe failed".to_owned())),
            }
        }
    }

    /// A channel fixture with just the fields `channel_known_disabled` reads.
    fn channel(index: u16, enabled: Option<bool>) -> Channel {
        Channel {
            index,
            name: None,
            enabled,
            ptz_support: PtzSupport::Unknown,
            recording: None,
        }
    }

    #[tokio::test]
    async fn channel_ptz_supported_admits_a_structurally_supported_channel() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ptz: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![Channel {
                ptz_support: PtzSupport::Supported,
                ..channel(1, Some(true))
            }]),
            stream_uri_fails: false,
        };
        assert!(channel_ptz_supported(&driver, 1).await);
        // A channel the probe never mentioned is not a proven negative.
        assert!(channel_ptz_supported(&driver, 2).await);
    }

    #[tokio::test]
    async fn channel_ptz_supported_refuses_only_a_confirmed_unsupported_channel() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ptz: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![
                Channel {
                    ptz_support: PtzSupport::Unsupported,
                    ..channel(1, Some(true))
                },
                Channel {
                    ptz_support: PtzSupport::Supported,
                    ..channel(2, Some(true))
                },
            ]),
            stream_uri_fails: false,
        };
        assert!(!channel_ptz_supported(&driver, 1).await);
        assert!(channel_ptz_supported(&driver, 2).await);
    }

    /// The three native families report `Unknown` for every channel (only ONVIF
    /// can produce `Supported`), so an unknown verdict must fall through to the
    /// driver — a confirmed-positive gate refused PTZ on the whole native fleet.
    #[tokio::test]
    async fn channel_ptz_supported_falls_through_on_unknown_and_probe_errors() {
        let unknown = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ptz: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![channel(1, Some(true))]),
            stream_uri_fails: false,
        };
        assert!(channel_ptz_supported(&unknown, 1).await);

        let error = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ptz: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::Error,
            stream_uri_fails: false,
        };
        assert!(channel_ptz_supported(&error, 1).await);
    }

    /// A family that cannot encode PTZ at all is still refused before any
    /// device I/O — the driver-level capability remains a hard gate.
    #[tokio::test]
    async fn channel_ptz_supported_false_without_the_driver_ptz_capability() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ptz: false,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![Channel {
                ptz_support: PtzSupport::Supported,
                ..channel(1, Some(true))
            }]),
            stream_uri_fails: false,
        };
        assert!(!channel_ptz_supported(&driver, 1).await);
    }

    /// Without channel discovery there is nothing to consult, so the device's
    /// own reply decides rather than a blanket refusal.
    #[tokio::test]
    async fn channel_ptz_supported_true_without_channel_discovery() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: false,
                ptz: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::Unsupported,
            stream_uri_fails: false,
        };
        assert!(channel_ptz_supported(&driver, 1).await);
    }

    #[tokio::test]
    async fn ptz_reaches_the_driver_only_for_a_non_negative_channel_verdict() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let channel_titles = json!({"Name": "ChannelTitle", "Ret": 100, "SessionID": "0x5",
            "ChannelTitle": ["Entrance"]});
        let ptz_protocol = json!({"Name": "PTZProtocol", "Ret": 100, "SessionID": "0x5",
            "PTZProtocol": [{"Enable": true}]});
        let transport = Arc::new(CountingTransport::new(vec![
            login,
            channel_titles,
            ptz_protocol,
        ]));
        let service = service_with_transport(Arc::clone(&transport), DeviceFamily::DvripSofia);

        service
            .ptz(Uuid::nil(), 1, PtzCommand::Preset { id: 4 })
            .await
            .expect("structurally supported PTZ command reaches the driver");

        assert_eq!(transport.connect_count(), 2, "probe then PTZ session");
        assert_eq!(transport.request_count(), 5, "three probe frames plus PTZ");
    }

    #[tokio::test]
    async fn channel_known_disabled_true_when_positively_reported_disabled() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![channel(1, Some(false))]),
            stream_uri_fails: false,
        };
        assert!(channel_known_disabled(&driver, 1).await);
    }

    #[tokio::test]
    async fn channel_known_disabled_false_without_channel_capability() {
        let driver = StubDriver {
            capabilities: Capabilities::default(), // channels: false
            channel_probe: ChannelProbe::List(vec![channel(1, Some(false))]),
            stream_uri_fails: false,
        };
        assert!(!channel_known_disabled(&driver, 1).await);
    }

    #[tokio::test]
    async fn channel_known_disabled_false_on_probe_error() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::Error,
            stream_uri_fails: false,
        };
        assert!(!channel_known_disabled(&driver, 1).await);
    }

    #[tokio::test]
    async fn channel_known_disabled_false_when_enabled_is_none_or_true() {
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![channel(1, None), channel(2, Some(true))]),
            stream_uri_fails: false,
        };
        assert!(!channel_known_disabled(&driver, 1).await);
        assert!(!channel_known_disabled(&driver, 2).await);
    }

    // Bug-1 regression: the RTSP fallback's OWN `stream_uri()` lookup failing
    // must not mask the original, informative native-snapshot error. Calls the
    // extracted fallback directly (rather than through `snapshot()`) because no
    // real driver's `stream_uri()` can be made to fail via a canned wire
    // script -- it never contacts the device (see `StubDriver`'s doc comment).
    #[tokio::test]
    async fn snapshot_fallback_preserves_native_error_when_its_own_stream_uri_fails() {
        let still = Arc::new(RecordingStill::new());
        let service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_still(Arc::clone(&still) as Arc<dyn RtspStillSource>);
        let driver = StubDriver {
            stream_uri_fails: true,
            ..StubDriver::plain()
        };
        let native_error = DriverError::Device("native still rejected".to_owned());

        let error = service
            .snapshot_via_rtsp(&driver, None, native_error)
            .await
            .expect_err("a failed fallback stream_uri lookup must not succeed");

        assert!(
            matches!(&error, DeviceServiceError::Driver(DriverError::Device(msg)) if msg == "native still rejected"),
            "expected the ORIGINAL native error to survive, got {error:?}"
        );
        assert!(
            still.url.lock().unwrap().is_none(),
            "the grabber must never be reached once stream_uri fails"
        );
    }

    /// A keyframe grabber that always fails — the failing counterpart to
    /// [`RecordingStill`], used to reproduce the bug-2 regression.
    struct FailingStill;

    #[async_trait]
    impl RtspStillSource for FailingStill {
        async fn keyframe(&self, _rtsp_url: &str) -> Result<Vec<u8>, DriverError> {
            Err(DriverError::Transport("keyframe grab failed".to_owned()))
        }
    }

    // Bug-2 regression: the RTSP fallback's OWN keyframe grab failing must not
    // mask the original native-snapshot error (here `Unsupported`, from the
    // real DVRIP OPSNAP-reject script) with the grabber's own unrelated
    // transport error. Goes through the public `snapshot()` entry point, so
    // this also proves the `snapshot_via_rtsp` extraction did not
    // break the existing wiring.
    #[tokio::test]
    async fn snapshot_preserves_native_error_when_the_rtsp_keyframe_grab_fails() {
        let service = service_with(opsnap_reject_script(), DeviceFamily::DvripSofia)
            .with_rtsp_still(Arc::new(FailingStill) as Arc<dyn RtspStillSource>);

        let error = service
            .snapshot(Uuid::nil(), None)
            .await
            .expect_err("a failed keyframe grab must not succeed");

        assert!(
            matches!(error, DeviceServiceError::Driver(DriverError::Unsupported)),
            "expected the ORIGINAL native Unsupported error to survive, got {error:?}"
        );
    }

    #[tokio::test]
    async fn snapshot_fallback_skips_auth_failures_and_confirmed_disabled_channels() {
        let auth_still = Arc::new(RecordingStill::new());
        let auth_service = service_with(
            vec![json!({"Name": "", "Ret": 205, "SessionID": "0x0"})],
            DeviceFamily::DvripSofia,
        )
        .with_rtsp_still(Arc::clone(&auth_still) as Arc<dyn RtspStillSource>);
        let auth_error = auth_service
            .snapshot(Uuid::nil(), Some(1))
            .await
            .expect_err("authentication failure must not trigger RTSP fallback");
        assert!(matches!(
            auth_error,
            DeviceServiceError::Driver(DriverError::Auth)
        ));
        assert!(auth_still.url.lock().unwrap().is_none());

        let disabled_still = Arc::new(RecordingStill::new());
        let disabled_service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_still(Arc::clone(&disabled_still) as Arc<dyn RtspStillSource>);
        let driver = StubDriver {
            capabilities: Capabilities {
                channels: true,
                ..Capabilities::default()
            },
            channel_probe: ChannelProbe::List(vec![channel(1, Some(false))]),
            stream_uri_fails: false,
        };
        let disabled_error = disabled_service
            .snapshot_via_rtsp(
                &driver,
                Some(1),
                DriverError::Device("native still rejected".to_owned()),
            )
            .await
            .expect_err("known-disabled channel must fail without spawning a grabber");
        assert!(
            matches!(&disabled_error, DeviceServiceError::Driver(DriverError::Device(message)) if message == "native still rejected")
        );
        assert!(disabled_still.url.lock().unwrap().is_none());
    }

    /// One scripted [`ScriptedLive::mjpeg`] result.
    enum LiveOutcome {
        Ok,
        Busy,
        Fail,
    }

    /// A live-restream double that replays a fixed script of outcomes (one per
    /// call), recording how many calls it received -- lets a test prove the
    /// substream-preferred `live_mjpeg` tried the right number of sources, and
    /// (for the busy case) that the main stream was never touched.
    struct ScriptedLive {
        outcomes: std::sync::Mutex<std::collections::VecDeque<LiveOutcome>>,
        calls: AtomicUsize,
    }

    impl ScriptedLive {
        fn new(outcomes: Vec<LiveOutcome>) -> Self {
            Self {
                outcomes: std::sync::Mutex::new(outcomes.into()),
                calls: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RtspLiveSource for ScriptedLive {
        async fn mjpeg(
            &self,
            _rtsp_url: &str,
            _options: LiveStreamOptions,
        ) -> Result<MjpegStream, LiveStreamError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcomes.lock().unwrap().pop_front() {
                Some(LiveOutcome::Ok) => Ok(MjpegStream {
                    boundary: "frame",
                    bytes: Box::pin(futures_util::stream::empty()),
                }),
                Some(LiveOutcome::Busy) => Err(LiveStreamError::Busy),
                Some(LiveOutcome::Fail) | None => Err(LiveStreamError::Driver(
                    DriverError::Device("substream playback failed".to_owned()),
                )),
            }
        }
    }

    // Substream-preferred live plane: the substream attempt fails (non-busy),
    // so `live_mjpeg` falls back to the main stream and succeeds there.
    #[tokio::test]
    async fn live_mjpeg_falls_back_to_main_stream_when_substream_fails() {
        let live = Arc::new(ScriptedLive::new(vec![LiveOutcome::Fail, LiveOutcome::Ok]));
        let service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_live(Arc::clone(&live) as Arc<dyn RtspLiveSource>)
            .with_live_prefer_substream(true);

        let result = service
            .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
            .await;

        assert!(result.is_ok(), "the main-stream fallback must succeed");
        assert_eq!(
            live.call_count(),
            2,
            "substream attempt then main-stream fallback"
        );
    }

    // A `Busy` result from the substream attempt is returned immediately: the
    // main stream is never tried (busy is a capacity fault of the shared
    // ffmpeg-slot semaphore, not a source fault).
    #[tokio::test]
    async fn live_mjpeg_returns_busy_immediately_without_retrying_main_stream() {
        let live = Arc::new(ScriptedLive::new(vec![LiveOutcome::Busy]));
        let service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_live(Arc::clone(&live) as Arc<dyn RtspLiveSource>)
            .with_live_prefer_substream(true);

        let error = service
            .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
            .await
            .expect_err("busy must surface as an error");

        assert!(matches!(error, DeviceServiceError::LiveBusy));
        assert_eq!(
            live.call_count(),
            1,
            "the main stream must never be tried after a busy substream"
        );
    }

    // Without the kill-switch enabled, behavior is unchanged: only the main
    // stream is ever tried (the substream-preferred plane is opt-in).
    #[tokio::test]
    async fn live_mjpeg_never_tries_substream_when_kill_switch_is_off() {
        let live = Arc::new(ScriptedLive::new(vec![LiveOutcome::Ok]));
        let service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_live(Arc::clone(&live) as Arc<dyn RtspLiveSource>);

        let result = service
            .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
            .await;

        assert!(
            result.is_ok(),
            "the single main-stream attempt must succeed"
        );
        assert_eq!(live.call_count(), 1, "prefer_substream defaults to off");
    }

    #[tokio::test]
    async fn live_mjpeg_maps_failures_and_short_circuits_a_healthy_substream() {
        let unavailable = service_with(vec![], DeviceFamily::DvripSofia)
            .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
            .await
            .expect_err("a missing live source is unsupported");
        assert!(matches!(
            unavailable,
            DeviceServiceError::Driver(DriverError::Unsupported)
        ));

        let busy_live = Arc::new(ScriptedLive::new(vec![LiveOutcome::Busy]));
        let busy_service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_live(Arc::clone(&busy_live) as Arc<dyn RtspLiveSource>);
        let busy = busy_service
            .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
            .await
            .expect_err("main-stream capacity exhaustion must map to LiveBusy");
        assert!(matches!(busy, DeviceServiceError::LiveBusy));

        let failed_live = Arc::new(ScriptedLive::new(vec![LiveOutcome::Fail]));
        let failed_service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_live(Arc::clone(&failed_live) as Arc<dyn RtspLiveSource>);
        let failed = failed_service
            .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
            .await
            .expect_err("driver failures must keep their original error class");
        assert!(
            matches!(failed, DeviceServiceError::Driver(DriverError::Device(message)) if message == "substream playback failed")
        );

        let substream_live = Arc::new(ScriptedLive::new(vec![LiveOutcome::Ok]));
        let substream_service = service_with(vec![], DeviceFamily::DvripSofia)
            .with_rtsp_live(Arc::clone(&substream_live) as Arc<dyn RtspLiveSource>)
            .with_live_prefer_substream(true);
        assert!(
            substream_service
                .live_mjpeg(Uuid::nil(), None, LiveStreamOptions::default())
                .await
                .is_ok()
        );
        assert_eq!(
            substream_live.call_count(),
            1,
            "a healthy substream must short-circuit the main stream"
        );

        let unresolved_substream = StubDriver {
            stream_uri_fails: true,
            ..StubDriver::plain()
        };
        let unused_live = ScriptedLive::new(vec![LiveOutcome::Ok]);
        assert!(
            substream_service
                .live_via_substream(
                    &unresolved_substream,
                    None,
                    LiveStreamOptions::default(),
                    &unused_live,
                )
                .await
                .is_none()
        );
        assert_eq!(unused_live.call_count(), 0);
    }
}
