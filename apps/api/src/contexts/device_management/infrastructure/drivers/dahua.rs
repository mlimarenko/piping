//! Dahua driver (HTTP CGI on the device's HTTP port, RFC 2617 Digest auth).
//!
//! Dahua's management plane is a set of `/cgi-bin/*.cgi?action=...` endpoints that
//! answer with flat, indexed `key=value` text (not XML). Each operation is one or
//! more independent authenticated `GET`s via an [`HttpDeviceClient`]; there is no
//! session to hold. Read operations (`get_info`, `list_users`, `disk_status`),
//! `stream_uri`, snapshot capture (`snapshot.cgi`), and user provisioning
//! (`create_users`/`delete_users`) are implemented and match responses verified
//! against a live device (2026-07-17). Recording enumeration uses the stateful
//! `mediaFileFind` factory/find-next loop.
//!
//! Dahua's user-write plane is, like the reads, plain authenticated `GET`s:
//! `userManager.cgi?action=addUser&user.Name=…` and `…&action=deleteUser&name=…`.
//! Every query value is percent-encoded, so a password containing `&` or `=`
//! cannot break out of its `key=value` slot; the plaintext is never logged.
//! The same percent-encoding is applied to the `user`/`password` RTSP userinfo
//! in `stream_uri` and `playback_uri`, so a credential containing `@`, `:`,
//! `/`, `?`, `#`, or whitespace cannot break the URL — or defeat the
//! [`StreamUri`] `Debug` redaction, which locates the userinfo by its first
//! unescaped `@`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, Channel, DeviceDriver, DeviceInfo, DiskState, DiskStatus,
    DownloadStream, DriverError, ExportRequest, LinkFailurePolicy, NewUser, PtzCommand, PtzSupport,
    PtzVector, RecordSpan, RecordingStatus, RemuxHints, Secret, SnapshotRef, StreamTransport,
    StreamUri, UserAccount, UserUpdate,
};
use crate::contexts::device_management::infrastructure::http_client::HttpDeviceClient;
use crate::contexts::device_management::infrastructure::transport::DeviceEndpoint;

/// System identity/serial CGI.
const PATH_SYSTEM_INFO: &str = "/cgi-bin/magicBox.cgi?action=getSystemInfo";
/// Model/type CGI.
const PATH_DEVICE_TYPE: &str = "/cgi-bin/magicBox.cgi?action=getDeviceType";
/// Firmware/build CGI.
const PATH_SOFTWARE_VERSION: &str = "/cgi-bin/magicBox.cgi?action=getSoftwareVersion";
/// Full user-account listing CGI.
const PATH_USERS: &str = "/cgi-bin/userManager.cgi?action=getUserInfoAll";
/// User-provisioning CGI prefix (query parameters are appended, url-encoded).
const PATH_ADD_USER: &str = "/cgi-bin/userManager.cgi?action=addUser";
/// User-edit CGI prefix (query parameters are appended, url-encoded). The exact
/// `action=modifyUser` parameter names follow Dahua's `addUser` convention
/// (`user.Name`/`user.Password`/`user.Group`/`user.AuthorityList`) and are **not
/// yet live-verified** — validate against real hardware before enabling writes.
const PATH_MODIFY_USER: &str = "/cgi-bin/userManager.cgi?action=modifyUser";
/// User-removal CGI prefix (the `name` parameter is appended, url-encoded).
const PATH_DELETE_USER: &str = "/cgi-bin/userManager.cgi?action=deleteUser";
/// Group assigned when a [`NewUser`] does not specify one.
const DEFAULT_USER_GROUP: &str = "user";
/// Storage/disk health CGI.
const PATH_STORAGE: &str = "/cgi-bin/storageDevice.cgi?action=getDeviceAllInfo";
/// Still-image capture CGI prefix (the 1-based `channel` is appended).
const PATH_SNAPSHOT: &str = "/cgi-bin/snapshot.cgi";
/// Per-channel display-name config CGI (answers `table.ChannelTitle[N].Name=...`).
const PATH_CHANNEL_TITLE: &str = "/cgi-bin/configManager.cgi?action=getConfig&name=ChannelTitle";
/// Per-channel PTZ protocol config CGI (answers `table.PTZProtocol[N].Enable=...`);
/// `Enable=true` means the channel has a PTZ head wired to that protocol slot.
const PATH_PTZ_PROTOCOL: &str = "/cgi-bin/configManager.cgi?action=getConfig&name=PTZProtocol";
/// PTZ control CGI prefix (`action`/`channel`/`code`/`arg*` are appended).
const PATH_PTZ: &str = "/cgi-bin/ptz.cgi";
/// Recorded-media search CGI prefix (`action`/`object`/`condition.*` appended).
const PATH_MEDIA_FIND: &str = "/cgi-bin/mediaFileFind.cgi";
/// Native fast file-download CGI: `action=startLoad` streams the `.dav` recorded
/// footage for a channel/time window several times faster than realtime, unlike
/// RTSP playback (which runs at recording speed). Verified live on Dahua-167.
const PATH_LOADFILE: &str = "/cgi-bin/loadfile.cgi";
/// How far back the recording-window search reaches (device retains far less in
/// practice; a wide ceiling just means "give me the whole archive window").
const RECORDING_LOOKBACK_DAYS: i64 = 90;
/// Page size for the `findNextFile` loop, and the "recording is live" recency
/// threshold: a newest span ending within this of now means recording is active.
const MEDIA_FIND_PAGE: u32 = 100;
/// Direction codes a PTZ stop must halt, covering every axis a move could start.
const PTZ_STOP_CODES: [&str; 6] = ["Up", "Down", "Left", "Right", "ZoomTele", "ZoomWide"];
/// Maximum Dahua PTZ speed step; a normalized velocity scales onto `1..=8`.
const PTZ_MAX_SPEED: i32 = 8;
/// RTSP egress port (the near-universal Dahua default).
const RTSP_PORT: u16 = 554;
/// Timeout for the whole `recording_status` walk (one `mediaFileFind` cycle
/// per channel). Each request already has its own per-request timeout in the
/// shared [`HttpDeviceClient`], but that alone does not bound the *number* of
/// requests a multi-channel/multi-page walk can make, so this caps the whole
/// call's wall-clock time too.
const RECORDING_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Upper bound on `findNextFile` pages per channel, so a device that never
/// signals exhaustion (via a short page) cannot loop forever.
const MEDIA_FIND_MAX_PAGES: u32 = 200;
/// Upper bound on spans retained for one `recording_status` call, across every
/// channel and page. Enforced by trimming each page to the remaining budget
/// before it is appended, so neither a device that never signals exhaustion
/// nor one that stuffs far more than [`MEDIA_FIND_PAGE`] items into a single
/// reply can grow the result (or the driver's memory) without bound.
const MEDIA_FIND_MAX_SPANS: usize = 20_000;
/// Upper bound on channels the recording-status walk aggregates; far above any
/// real Dahua NVR's channel count, just enough that a misbehaving channel list
/// cannot turn this into an unbounded per-channel fan-out.
const RECORDING_STATUS_MAX_CHANNELS: usize = 64;

/// A Dahua driver bound to one device.
pub struct DahuaDriver {
    http: Arc<dyn HttpDeviceClient>,
    endpoint: DeviceEndpoint,
    username: String,
    password: Secret,
}

/// Releases a `mediaFileFind` factory handle (`close` then `destroy`) exactly
/// once, so a `recording_status` call that hits a timeout/cap, errors, or has
/// its future dropped/cancelled by the caller still frees the device-side
/// handle instead of leaking it.
///
/// The happy path awaits [`release`](Self::release) directly. `Drop` is the
/// fallback for every other exit: it cannot `.await`, so it fires the same two
/// requests on a detached task instead, mirroring
/// `crate::proxy::ws::ActiveBridgeGuard`.
struct MediaFindHandleGuard {
    http: Arc<dyn HttpDeviceClient>,
    object: i64,
    released: bool,
}

impl MediaFindHandleGuard {
    fn new(http: Arc<dyn HttpDeviceClient>, object: i64) -> Self {
        Self {
            http,
            object,
            released: false,
        }
    }

    /// Close then destroy the handle, awaiting both so the caller observes
    /// completion before it returns. Marks the guard released so `Drop` does
    /// not also spawn a redundant cleanup task.
    async fn release(&mut self) {
        let object = self.object;
        let _ = self
            .http
            .get_text(&format!("{PATH_MEDIA_FIND}?action=close&object={object}"))
            .await;
        let _ = self
            .http
            .get_text(&format!("{PATH_MEDIA_FIND}?action=destroy&object={object}"))
            .await;
        self.released = true;
    }
}

impl Drop for MediaFindHandleGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // `Drop` cannot `.await`; a still-live handle (early return, the
        // caps above, a timeout, or the caller cancelling this call) is
        // released best-effort on a detached task instead of leaking it on
        // the device. Outside a Tokio runtime (e.g. plain unit-test drop)
        // there is nowhere to spawn onto, so cleanup is simply skipped.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let http = self.http.clone();
        let object = self.object;
        handle.spawn(async move {
            let _ = http
                .get_text(&format!("{PATH_MEDIA_FIND}?action=close&object={object}"))
                .await;
            let _ = http
                .get_text(&format!("{PATH_MEDIA_FIND}?action=destroy&object={object}"))
                .await;
        });
    }
}

impl DahuaDriver {
    /// Construct a driver reaching the device via `http`.
    pub fn new(
        http: Arc<dyn HttpDeviceClient>,
        endpoint: DeviceEndpoint,
        username: impl Into<String>,
        password: Secret,
    ) -> Self {
        Self {
            http,
            endpoint,
            username: username.into(),
            password,
        }
    }

    /// Run one [`collect_channel_recording`](Self::collect_channel_recording)
    /// cycle per channel and aggregate the spans, so one idle or erroring
    /// channel on a multi-channel device does not decide the whole device's
    /// status.
    ///
    /// Uses the same fatal-link classifier (`stops_all_work`), but this is a
    /// read walk, not a committed-write outcome: a
    /// [`DriverError::Transport`] means the device itself is unreachable, so
    /// it aborts the whole walk immediately. Any other per-channel error is
    /// skipped so the remaining channels still aggregate — unless every
    /// channel fails that way, in which case the last such error is surfaced
    /// rather than silently reporting an empty/inactive status for a device
    /// that turned out not to be readable at all.
    async fn collect_all_channels(
        &self,
        channels: &[u16],
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        let mut spans = Vec::new();
        let mut last_error = None;
        let mut any_channel_ok = false;
        for &channel in channels {
            match self.collect_channel_recording(channel, start, end).await {
                Ok(channel_spans) => {
                    any_channel_ok = true;
                    spans.extend(channel_spans);
                }
                Err(error) if stops_all_work(&error) => return Err(error),
                Err(error) => last_error = Some(error),
            }
            if spans.len() >= MEDIA_FIND_MAX_SPANS {
                spans.truncate(MEDIA_FIND_MAX_SPANS);
                break;
            }
        }
        if !any_channel_ok && let Some(error) = last_error {
            return Err(error);
        }
        Ok(spans)
    }

    /// One full `mediaFileFind` cycle (`factory.create` → `findFile` → paged
    /// `findNextFile` → `close`/`destroy`) for a single channel — the exact
    /// sequence verified against a live device (see the module docs), just
    /// parameterized by channel. `recording_status` repeats this per channel
    /// rather than assuming one factory handle can be reused across
    /// sequential channel searches, which is **not** live-verified.
    async fn collect_channel_recording(
        &self,
        channel: u16,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        let created = self
            .http
            .get_text(&format!("{PATH_MEDIA_FIND}?action=factory.create"))
            .await?;
        let object = parse_factory_object(&created).ok_or_else(|| {
            DriverError::Protocol("mediaFileFind factory has no object".to_owned())
        })?;

        // The handle is released on every exit path: the happy path awaits
        // `release` below; an error from `page_media_find`, or the caller
        // dropping this future, both fall through to the guard's `Drop`.
        let mut guard = MediaFindHandleGuard::new(self.http.clone(), object);
        let spans = self.page_media_find(object, channel, start, end).await?;
        guard.release().await;
        Ok(spans)
    }

    /// Page `findNextFile` for one channel until the device stops returning
    /// items, [`MEDIA_FIND_MAX_PAGES`] is reached, or the running total hits
    /// [`MEDIA_FIND_MAX_SPANS`] — whichever comes first — collecting the
    /// recorded spans. Called between factory create and close/destroy.
    async fn page_media_find(
        &self,
        object: i64,
        channel: u16,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        let condition = format!(
            "condition.Channel={channel}&condition.StartTime={start}&condition.EndTime={end}&condition.Types[0]=dav",
            start = dahua_time_param(start),
            end = dahua_time_param(end),
        );
        self.http
            .get_text(&format!(
                "{PATH_MEDIA_FIND}?action=findFile&object={object}&{condition}"
            ))
            .await?;

        let mut spans = Vec::new();
        for _ in 0..MEDIA_FIND_MAX_PAGES {
            let batch = self
                .http
                .get_text(&format!(
                    "{PATH_MEDIA_FIND}?action=findNextFile&object={object}&count={MEDIA_FIND_PAGE}"
                ))
                .await?;
            let (found, page) = parse_media_items(&batch);
            let page_len = page.len();
            let remaining = MEDIA_FIND_MAX_SPANS.saturating_sub(spans.len());
            spans.extend(page.into_iter().take(remaining));
            // The device returns fewer than a full page when the archive is
            // exhausted; a zero/short page ends the loop, as does the running
            // total reaching the cap (whether from many pages or one
            // oversized reply).
            if found < MEDIA_FIND_PAGE as usize || remaining <= page_len {
                break;
            }
        }
        Ok(spans)
    }
}

#[async_trait]
impl DeviceDriver for DahuaDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            info: true,
            users: true,
            disk: true,
            recording: true,
            snapshot: true,
            streaming: true,
            channels: true,
            ptz: true,
            export: true,
            user_update: true,
            authority_catalog: false,
            recording_search: true,
        }
    }

    async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
        let system_info = self.http.get_text(PATH_SYSTEM_INFO).await?;
        let device_type = self.http.get_text(PATH_DEVICE_TYPE).await?;
        let software_version = self.http.get_text(PATH_SOFTWARE_VERSION).await?;
        Ok(parse_system_info(
            &system_info,
            &device_type,
            &software_version,
        ))
    }

    async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
        let body = self.http.get_text(PATH_USERS).await?;
        Ok(parse_users(&body))
    }

    async fn create_users(&self, users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
        let mut outcome = BulkOutcome::default();
        for (index, user) in users.iter().enumerate() {
            let path = build_add_user_path(user);
            match self.http.get_text(&path).await {
                Ok(body) if response_reports_ok(&body) => outcome.succeeded.push(user.name.clone()),
                Ok(body) => outcome
                    .failed
                    .push((user.name.clone(), DriverError::Device(trimmed(&body)))),
                Err(error) if stops_all_work(&error) => {
                    return outcome.abort_after_link_failure(
                        user.name.clone(),
                        error,
                        users[index + 1..].iter().map(|user| user.name.clone()),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((user.name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn delete_users(&self, usernames: &[String]) -> Result<BulkOutcome, DriverError> {
        let mut outcome = BulkOutcome::default();
        for (index, username) in usernames.iter().enumerate() {
            let path = format!(
                "{PATH_DELETE_USER}&name={name}",
                name = encode_query_value(username),
            );
            match self.http.get_text(&path).await {
                Ok(body) if response_reports_ok(&body) => outcome.succeeded.push(username.clone()),
                Ok(body) => outcome
                    .failed
                    .push((username.clone(), DriverError::Device(trimmed(&body)))),
                Err(error) if stops_all_work(&error) => {
                    return outcome.abort_after_link_failure(
                        username.clone(),
                        error,
                        usernames[index + 1..].iter().cloned(),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((username.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn update_users(&self, updates: &[UserUpdate]) -> Result<BulkOutcome, DriverError> {
        // A per-user loop over the same stateless CGI plane as `create_users`,
        // reusing its `OK`/verbatim-body classification and transport-abort rule:
        // a `Device` error is one user's failure; a `Transport` error stops
        // further calls while keeping any acknowledged writes in the outcome.
        // Only the changed facets are sent (see `build_modify_user_path`);
        // the caller guarantees each update carries at least one change.
        let mut outcome = BulkOutcome::default();
        for (index, update) in updates.iter().enumerate() {
            let path = build_modify_user_path(update);
            match self.http.get_text(&path).await {
                Ok(body) if response_reports_ok(&body) => {
                    outcome.succeeded.push(update.name.clone());
                }
                Ok(body) => outcome
                    .failed
                    .push((update.name.clone(), DriverError::Device(trimmed(&body)))),
                Err(error) if stops_all_work(&error) => {
                    return outcome.abort_after_link_failure(
                        update.name.clone(),
                        error,
                        updates[index + 1..]
                            .iter()
                            .map(|update| update.name.clone()),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((update.name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
        let body = self.http.get_text(PATH_STORAGE).await?;
        Ok(parse_disks(&body))
    }

    async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
        // Dahua enumerates recorded media through a stateful `mediaFileFind`
        // handle, one cycle per channel (create a factory object, run one
        // findFile over a wide window, page findNextFile until the device
        // stops returning items, then close and destroy the handle — see
        // `collect_channel_recording`/`MediaFindHandleGuard`). Channels come
        // from `list_channels`, falling back to channel 1 alone when that read
        // fails or reports none, so a single idle/hardcoded channel no longer
        // decides a multi-channel NVR's whole status. The aggregate spans give
        // the archive window (oldest/newest) and per-span list; "active" is
        // inferred from the newest span reaching近 now. The whole walk is
        // bounded (`RECORDING_STATUS_TIMEOUT`, `MEDIA_FIND_MAX_PAGES`,
        // `MEDIA_FIND_MAX_SPANS`, `RECORDING_STATUS_MAX_CHANNELS`) so it cannot
        // hang or grow without bound. Timestamps are device-local
        // `YYYY-MM-DD HH:MM:SS` with no zone — read as UTC, consistent with
        // the rest of the driver.
        let now = Utc::now();
        let start = now - chrono::Duration::days(RECORDING_LOOKBACK_DAYS);
        let end = now + chrono::Duration::days(1);

        let mut wire_channels: Vec<u16> = self
            .list_channels()
            .await
            .map(|channels| channels.iter().map(|c| c.index).collect())
            .unwrap_or_default();
        if wire_channels.is_empty() {
            wire_channels.push(1);
        }
        wire_channels.truncate(RECORDING_STATUS_MAX_CHANNELS);

        let spans = tokio::time::timeout(
            RECORDING_STATUS_TIMEOUT,
            self.collect_all_channels(&wire_channels, start, end),
        )
        .await
        .map_err(|_| DriverError::Protocol("mediaFileFind timed out".to_owned()))??;

        let oldest = spans.iter().map(|s| s.start).min();
        let newest = spans.iter().map(|s| s.end).max();
        let active = newest.is_some_and(|n| now - n < chrono::Duration::minutes(10));
        Ok(RecordingStatus {
            active,
            oldest,
            newest,
            spans,
        })
    }

    async fn search_recordings(
        &self,
        request: &ExportRequest,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        // The exact per-channel primitive `recording_status` already runs in a
        // loop over every channel; here the caller names the one channel and
        // window it actually wants, so a single cycle answers it — and can run
        // well within the caller's own request timeout rather than the
        // device-wide `RECORDING_STATUS_TIMEOUT`.
        let spans = self
            .collect_channel_recording(request.channel, request.start, request.end)
            .await?;
        Ok(spans
            .into_iter()
            .map(|span| RecordSpan {
                channel: Some(request.channel),
                ..span
            })
            .collect())
    }

    async fn snapshot(&self, channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
        // `snapshot.cgi?channel=<1-based>` answers with `image/jpeg`; the HTTP
        // client validates the JPEG marker, so the bytes can be wrapped directly.
        let channel = channel.unwrap_or(1);
        let path = format!("{PATH_SNAPSHOT}?channel={channel}");
        let jpeg = self.http.get_bytes(&path).await?;
        Ok(SnapshotRef::Bytes(jpeg))
    }

    async fn list_channels(&self) -> Result<Vec<Channel>, DriverError> {
        let body = self.http.get_text(PATH_CHANNEL_TITLE).await?;
        let mut channels = parse_channels(&body);
        // Best-effort enrichment: a device that rejects/lacks this config query
        // (older firmware, or the CGI itself disabled) leaves every channel at
        // its already-`Unknown` default rather than failing the whole listing.
        if let Ok(ptz_body) = self.http.get_text(PATH_PTZ_PROTOCOL).await {
            apply_ptz_support(&mut channels, &ptz_body);
        }
        Ok(channels)
    }

    async fn ptz(&self, channel: u16, command: PtzCommand) -> Result<(), DriverError> {
        // Dahua PTZ is `ptz.cgi?action=start&code=<Direction|Zoom>&arg2=<speed>`,
        // halted by `action=stop`. A continuous move maps the dominant axis to one
        // code; a preset recall uses `code=GotoPreset&arg2=<id>`.
        match command {
            PtzCommand::Move(vector) => {
                let Some((code, speed)) = dahua_move_code(vector.clamped()) else {
                    // An all-zero move is a no-op (the client sends Stop to halt).
                    return Ok(());
                };
                let path = format!(
                    "{PATH_PTZ}?action=start&channel={channel}&code={code}&arg1=0&arg2={speed}&arg3=0"
                );
                self.http.get_text(&path).await?;
            }
            PtzCommand::Stop => {
                // Dahua stops per-code, so halt every axis a move could have started;
                // a per-code error must not prevent stopping the others.
                for code in PTZ_STOP_CODES {
                    let path = format!(
                        "{PATH_PTZ}?action=stop&channel={channel}&code={code}&arg1=0&arg2=0&arg3=0"
                    );
                    let _ = self.http.get_text(&path).await;
                }
            }
            PtzCommand::Preset { id } => {
                let path = format!(
                    "{PATH_PTZ}?action=start&channel={channel}&code=GotoPreset&arg1=0&arg2={id}&arg3=0"
                );
                self.http.get_text(&path).await?;
            }
        }
        Ok(())
    }

    async fn stream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        let channel = channel.unwrap_or(1);
        // The userinfo is percent-encoded (see module docs): a raw `@`/`:` in
        // the credentials would otherwise both break the URL's structure and
        // defeat `StreamUri`'s `Debug` redaction, which splits on the first
        // unescaped `@`.
        let uri = format!(
            "rtsp://{user}:{password}@{host}:{RTSP_PORT}/cam/realmonitor?channel={channel}&subtype=0",
            user = encode_query_value(&self.username),
            password = encode_query_value(self.password.expose()),
            host = self.endpoint.host,
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn substream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        // Same shape as `stream_uri` above, but `subtype=1` selects Dahua's
        // secondary (lower-resolution) stream instead of the main stream
        // (`subtype=0`). Whether/how a given device exposes a distinct
        // substream is device-config dependent and this is **not yet
        // live-verified** against real hardware; per the trait doc on
        // `substream_uri`, treat the resolved URI as speculative and fall
        // back to `stream_uri` if it fails to play.
        let channel = channel.unwrap_or(1);
        let uri = format!(
            "rtsp://{user}:{password}@{host}:{RTSP_PORT}/cam/realmonitor?channel={channel}&subtype=1",
            user = encode_query_value(&self.username),
            password = encode_query_value(self.password.expose()),
            host = self.endpoint.host,
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn playback_uri(&self, request: &ExportRequest) -> Result<StreamUri, DriverError> {
        // Dahua RTSP playback: `/cam/playback?channel=N&starttime=..&endtime=..`
        // with `YYYY_MM_DD_HH_MM_SS` timestamps (device-local; we pass the
        // requested UTC window verbatim, matching how recording times are read).
        // The userinfo is percent-encoded for the same reason as `stream_uri`.
        let uri = format!(
            "rtsp://{user}:{password}@{host}:{RTSP_PORT}/cam/playback?channel={channel}&starttime={start}&endtime={end}",
            user = encode_query_value(&self.username),
            password = encode_query_value(self.password.expose()),
            host = self.endpoint.host,
            channel = request.channel,
            start = request.start.format("%Y_%m_%d_%H_%M_%S"),
            end = request.end.format("%Y_%m_%d_%H_%M_%S"),
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn export_download(
        &self,
        request: &ExportRequest,
    ) -> Result<Option<DownloadStream>, DriverError> {
        // Dahua `loadfile.cgi?action=startLoad` streams the `.dav` recorded footage
        // for the window directly — no `mediaFileFind` handle needed. Times use the
        // same device-local `YYYY-MM-DD HH:MM:SS` (space percent-encoded) form as
        // the media search; `subtype=0` selects the main stream. This is the fast
        // path: an HTTP file pull runs several× faster than realtime and ~0 CPU,
        // and the media plane remuxes the `.dav` to MP4 with `-c copy`.
        let path = format!(
            "{PATH_LOADFILE}?action=startLoad&channel={channel}&startTime={start}&endTime={end}&subtype=0",
            channel = request.channel,
            start = dahua_time_param(request.start),
            end = dahua_time_param(request.end),
        );
        let bytes = self.http.download_stream(&path).await?;
        let filename = format!(
            "dahua-ch{channel}-{start}",
            channel = request.channel,
            start = request.start.format("%Y%m%dT%H%M%SZ"),
        );
        // A real `.dav` container: the prober reads its own timing, so no hints.
        Ok(Some(DownloadStream {
            bytes,
            filename,
            hints: RemuxHints::default(),
        }))
    }
}

// ── User-write request building & response classification ──────────────────

/// Build the `addUser` request path for one [`NewUser`], percent-encoding every
/// value so it stays inside its `key=value` slot.
///
/// `user.Group` defaults to [`DEFAULT_USER_GROUP`] when the account carries none.
/// `user.AuthorityList` is the device's authority tokens joined by literal commas
/// (each token encoded individually), matching Dahua's list-parameter convention.
fn build_add_user_path(user: &NewUser) -> String {
    let group = user.group.as_deref().unwrap_or(DEFAULT_USER_GROUP);
    let authorities = user
        .authorities
        .iter()
        .map(|authority| encode_query_value(authority))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{PATH_ADD_USER}&user.Name={name}&user.Password={password}&user.Group={group}\
         &user.Sharable=false&user.Reserved=false&user.AuthorityList={authorities}",
        name = encode_query_value(&user.name),
        password = encode_query_value(user.password.expose()),
        group = encode_query_value(group),
    )
}

/// Build the `modifyUser` request path for one [`UserUpdate`], percent-encoding
/// every value so it stays inside its `key=value` slot.
///
/// `user.Name` (the identity) is always present; `user.Password`, `user.Group`,
/// and `user.AuthorityList` are appended only for the facets the update changes,
/// so an unset field is left untouched on the device. Authority tokens are joined
/// by literal commas (each encoded individually), matching Dahua's list-parameter
/// convention exactly as `build_add_user_path` does.
///
/// The `action=modifyUser` parameter names are taken from Dahua's `addUser`
/// convention and are **not yet live-verified** (see [`PATH_MODIFY_USER`]).
fn build_modify_user_path(update: &UserUpdate) -> String {
    let mut path = format!(
        "{PATH_MODIFY_USER}&user.Name={name}",
        name = encode_query_value(&update.name),
    );
    if let Some(password) = &update.password {
        path.push_str(&format!(
            "&user.Password={}",
            encode_query_value(password.expose()),
        ));
    }
    if let Some(group) = &update.group {
        path.push_str(&format!("&user.Group={}", encode_query_value(group)));
    }
    if let Some(authorities) = &update.authorities {
        let joined = authorities
            .iter()
            .map(|authority| encode_query_value(authority))
            .collect::<Vec<_>>()
            .join(",");
        path.push_str(&format!("&user.AuthorityList={joined}"));
    }
    path
}

/// Percent-encode a value for safe embedding in a URL: a CGI query value, or
/// (for `stream_uri`/`playback_uri`) the `user`/`password` RTSP userinfo.
///
/// [`NON_ALPHANUMERIC`] is deliberately aggressive: it encodes every reserved
/// character (`&`, `=`, `+`, `%`, space, `@`, `:`, `/`, `?`, `#`, …), so a
/// password like `a&b=c` travels as `a%26b%3Dc` and can never be mistaken for
/// query structure, and one containing `@`/`:`/whitespace cannot break RTSP
/// userinfo delimiters (or the userinfo-based `Debug` redaction) either. The
/// device percent-decodes CGI query values back on receipt; RTSP userinfo is
/// decoded by the RTSP/media client.
fn encode_query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// Whether a Dahua CGI write reported success.
///
/// The success plane is a single protocol literal: Dahua answers a completed
/// write with the exact body `OK` (surrounding whitespace/line endings aside).
/// This is an exact match, not a substring check: a body that merely contains
/// `OK` — a `200 OK` status line, a `TOKEN`, or a proxy/reboot HTML page — is
/// not a success. Any other body is treated as a failure and surfaced verbatim
/// (trimmed) — including "user already exists" / "no such user". We deliberately
/// do **not** pattern-match those messages to fake idempotent success: Dahua's
/// error text is device- and locale-specific, so a natural-language marker list
/// would be brittle and is disallowed by the codebase's domain-neutral rules. A
/// caller that wants upsert semantics inspects [`BulkOutcome::failed`] and its
/// device message rather than relying on the driver to guess intent.
fn response_reports_ok(body: &str) -> bool {
    body.trim() == "OK"
}

/// A device-error variant that stops a bulk loop at the link rather than being
/// recorded as one item's failure.
///
/// Typed transport failures stop the loop: the byte channel to the device is
/// gone, so no remaining item could succeed. The acknowledged prefix is kept;
/// when the failure hits before anything was acknowledged the batch is a
/// top-level `Err` (`LinkFailurePolicy::ErrorWithoutAcknowledged`). Every other
/// error is per-request and kept as a failed entry so partial success is
/// preserved.
fn stops_all_work(error: &DriverError) -> bool {
    matches!(
        error,
        DriverError::Transport(_) | DriverError::Timeout(_) | DriverError::Unreachable(_)
    )
}

/// A trimmed owned copy of a device response body, for use as an error message.
fn trimmed(body: &str) -> String {
    body.trim().to_owned()
}

// ── Pure parsers (structure verified against a live device) ─────────────────

/// Parse flat `key=value` lines into a map keyed by the full left-hand side
/// (e.g. `users[0].Name`). Blank keys and lines without `=` are skipped; the
/// value keeps everything after the first `=`.
fn parse_dahua_kv(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| !key.trim().is_empty())
        .map(|(key, value)| (key.trim().to_owned(), value.to_owned()))
        .collect()
}

/// Combine the three identity CGIs into a single [`DeviceInfo`].
///
/// `channels` and `device_time` are left `None` — Dahua exposes no cheap flat
/// field for them.
fn parse_system_info(system_info: &str, device_type: &str, software_version: &str) -> DeviceInfo {
    let system = parse_dahua_kv(system_info);
    let device = parse_dahua_kv(device_type);
    let software = parse_dahua_kv(software_version);
    DeviceInfo {
        model: non_empty(device.get("type")),
        firmware: non_empty(software.get("version")).map(firmware_head),
        serial: non_empty(system.get("serialNumber")),
        channels: None,
        device_time: None,
    }
}

/// Take the firmware version up to the first comma (dropping the `,build:...`
/// suffix), or the whole string when there is no comma.
fn firmware_head(version: String) -> String {
    version
        .split_once(',')
        .map_or(version.as_str(), |(head, _)| head)
        .to_owned()
}

/// A non-empty owned copy of an optional field.
fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Map a normalized PTZ velocity vector onto one Dahua direction/zoom `code`
/// plus a `1..=8` speed. Zoom wins when it is the largest-magnitude axis;
/// otherwise pan/tilt combine into a cardinal or diagonal direction
/// (`Left`/`RightUp`/…). Returns `None` for an all-zero (no-op) vector.
fn dahua_move_code(vector: PtzVector) -> Option<(String, i32)> {
    let magnitude = vector
        .pan
        .abs()
        .max(vector.tilt.abs())
        .max(vector.zoom.abs());
    if magnitude <= 0.0 {
        return None;
    }
    let speed = (magnitude * PTZ_MAX_SPEED as f32).round() as i32;
    let speed = speed.clamp(1, PTZ_MAX_SPEED);

    // Zoom dominates when its magnitude is at least as large as both pan and tilt.
    if vector.zoom.abs() >= vector.pan.abs() && vector.zoom.abs() >= vector.tilt.abs() {
        let code = if vector.zoom > 0.0 {
            "ZoomTele"
        } else {
            "ZoomWide"
        };
        return Some((code.to_owned(), speed));
    }

    let horizontal = axis_label(vector.pan, "Right", "Left");
    let vertical = axis_label(vector.tilt, "Up", "Down");
    let code = match (horizontal, vertical) {
        ("", "") => return None,
        (horiz, "") => horiz.to_owned(),
        ("", vert) => vert.to_owned(),
        // Dahua diagonal codes are horizontal-then-vertical: `LeftUp`, `RightDown`.
        (horiz, vert) => format!("{horiz}{vert}"),
    };
    Some((code, speed))
}

/// The label for one axis: `positive` when the value is > 0, `negative` when
/// < 0, and empty when the axis is idle.
fn axis_label(value: f32, positive: &'static str, negative: &'static str) -> &'static str {
    if value > 0.0 {
        positive
    } else if value < 0.0 {
        negative
    } else {
        ""
    }
}

/// Parse the `ChannelTitle` config (`table.ChannelTitle[N].Name=<title>`) into
/// 1-based [`Channel`]s ordered by the device's 0-based index. A blank title
/// yields a nameless channel; the index still identifies it.
fn parse_channels(text: &str) -> Vec<Channel> {
    let mut by_index: BTreeMap<usize, String> = BTreeMap::new();
    for (key, value) in parse_dahua_kv(text) {
        if let Some(index) = channel_title_index(&key) {
            by_index.insert(index, value);
        }
    }
    by_index
        .into_iter()
        .map(|(wire_index, name)| Channel {
            index: u16::try_from(wire_index)
                .unwrap_or(u16::MAX)
                .saturating_add(1),
            name: Some(name)
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty()),
            enabled: None,
            ptz_support: PtzSupport::Unknown,
            recording: None,
        })
        .collect()
}

/// Set each channel's `ptz_support` from a `PTZProtocol` config body
/// (`table.PTZProtocol[N].Enable=<true|false>`), keyed by the same 0-based
/// wire index `ChannelTitle` uses. A channel with no matching `Enable` entry
/// keeps its existing `Unknown` — the device answered but said nothing
/// specific about that channel, which is not the same as a confirmed negative.
fn apply_ptz_support(channels: &mut [Channel], ptz_body: &str) {
    let mut enabled_by_index: BTreeMap<usize, bool> = BTreeMap::new();
    for (key, value) in parse_dahua_kv(ptz_body) {
        if let Some((index, "Enable")) = indexed_field(&key, "table.PTZProtocol") {
            enabled_by_index.insert(index, value.eq_ignore_ascii_case("true"));
        }
    }
    for channel in channels.iter_mut() {
        let wire_index = usize::from(channel.index.saturating_sub(1));
        if let Some(enabled) = enabled_by_index.get(&wire_index) {
            channel.ptz_support = if *enabled {
                PtzSupport::Supported
            } else {
                PtzSupport::Unsupported
            };
        }
    }
}

/// Parse a `table.ChannelTitle[N].Name` key into its 0-based index `N`.
fn channel_title_index(key: &str) -> Option<usize> {
    let rest = key.strip_prefix("table.ChannelTitle[")?;
    let close = rest.find(']')?;
    let index: usize = rest[..close].parse().ok()?;
    // Only the `.Name` leaf carries the title; ignore any sibling fields.
    rest[close + 1..].strip_prefix(".Name").map(|_| index)
}

/// Parse `users[N].*` entries into accounts, grouped and ordered by index `N`.
fn parse_users(text: &str) -> Vec<UserAccount> {
    let mut builders: BTreeMap<usize, UserBuilder> = BTreeMap::new();
    for (key, value) in parse_dahua_kv(text) {
        let Some((index, field)) = indexed_field(&key, "users") else {
            continue;
        };
        let builder = builders.entry(index).or_default();
        match field {
            "Name" => builder.name = Some(value),
            "Group" => builder.group = Some(value),
            other => {
                if let Some(authority_index) = list_index(other, "AuthorityList") {
                    builder.authorities.insert(authority_index, value);
                }
            }
        }
    }
    builders
        .into_values()
        .filter_map(UserBuilder::build)
        .collect()
}

/// Accumulates the fields of one `users[N]` entry before validation.
#[derive(Default)]
struct UserBuilder {
    name: Option<String>,
    group: Option<String>,
    /// Authority tokens keyed by their `AuthorityList[i]` index, for stable order.
    authorities: BTreeMap<usize, String>,
}

impl UserBuilder {
    /// Finalize into a [`UserAccount`], dropping entries with no login name.
    fn build(self) -> Option<UserAccount> {
        let name = self.name.filter(|n| !n.is_empty())?;
        Some(UserAccount {
            name,
            group: self.group.filter(|g| !g.is_empty()),
            authorities: self.authorities.into_values().collect(),
        })
    }
}

/// Parse `list.info[A].Detail[B].*` entries into per-detail disk records, keeping
/// only details that report a positive total capacity.
fn parse_disks(text: &str) -> Vec<DiskStatus> {
    let mut builders: BTreeMap<(usize, usize), DiskBuilder> = BTreeMap::new();
    for (key, value) in parse_dahua_kv(text) {
        let Some((slot, field)) = detail_field(&key) else {
            continue;
        };
        let builder = builders.entry(slot).or_default();
        match field {
            "TotalBytes" => builder.total_bytes = value.parse().ok(),
            "UsedBytes" => builder.used_bytes = value.parse().ok(),
            "IsError" => builder.is_error = value.eq_ignore_ascii_case("true"),
            "Path" => builder.path = Some(value),
            "Type" => builder.kind = Some(value),
            _ => {}
        }
    }
    builders
        .into_iter()
        .filter_map(|((info, detail), builder)| builder.build(info, detail))
        .collect()
}

/// Accumulates the fields of one `Detail[B]` storage entry before validation.
#[derive(Default)]
struct DiskBuilder {
    total_bytes: Option<u64>,
    used_bytes: Option<u64>,
    is_error: bool,
    path: Option<String>,
    kind: Option<String>,
}

impl DiskBuilder {
    /// Finalize into a [`DiskStatus`], dropping empty (zero-total) slots.
    ///
    /// State is `Error` when the device flagged the disk, `ReadOnly` when it
    /// reports a read-only `Type` (inferred from the protocol literal, not seen
    /// on the sampled device), otherwise `Ok`.
    fn build(self, info: usize, detail: usize) -> Option<DiskStatus> {
        let total_bytes = self.total_bytes.filter(|total| *total > 0)?;
        let name = self
            .path
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| format!("disk{info}-{detail}"));
        let state = if self.is_error {
            DiskState::Error
        } else if self
            .kind
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("ReadOnly"))
        {
            DiskState::ReadOnly
        } else {
            DiskState::Ok
        };
        Some(DiskStatus {
            name,
            total_bytes,
            used_bytes: self.used_bytes.unwrap_or(0),
            state,
            is_error: self.is_error,
        })
    }
}

/// Parse an indexed key like `users[3].Name` into `(3, "Name")`. The remaining
/// field (e.g. `AuthorityList[0]`) is returned verbatim for further parsing.
fn indexed_field<'a>(key: &'a str, prefix: &str) -> Option<(usize, &'a str)> {
    let rest = key.strip_prefix(prefix)?.strip_prefix('[')?;
    let close = rest.find(']')?;
    let index: usize = rest[..close].parse().ok()?;
    let field = rest[close + 1..].strip_prefix('.')?;
    Some((index, field))
}

/// Parse a list field like `AuthorityList[2]` into its index `2`.
fn list_index(field: &str, prefix: &str) -> Option<usize> {
    let rest = field.strip_prefix(prefix)?.strip_prefix('[')?;
    let close = rest.find(']')?;
    rest[..close].parse().ok()
}

/// Parse a storage key like `list.info[0].Detail[1].Path` into `((0, 1), "Path")`.
fn detail_field(key: &str) -> Option<((usize, usize), &str)> {
    let rest = key.strip_prefix("list.info[")?;
    let close = rest.find(']')?;
    let info: usize = rest[..close].parse().ok()?;
    let rest = rest[close + 1..].strip_prefix(".Detail[")?;
    let close = rest.find(']')?;
    let detail: usize = rest[..close].parse().ok()?;
    let field = rest[close + 1..].strip_prefix('.')?;
    Some(((info, detail), field))
}

/// Format a UTC instant as the device-local `YYYY-MM-DD HH:MM:SS` mediaFileFind
/// expects, percent-encoding the space so it survives the query string.
fn dahua_time_param(when: DateTime<Utc>) -> String {
    when.format("%Y-%m-%d%%20%H:%M:%S").to_string()
}

/// Parse a `factory.create` reply (`result=<object>` or `<object>`) into the
/// handle id used by subsequent find calls.
fn parse_factory_object(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    let value = trimmed
        .lines()
        .find_map(|line| line.split_once('=').map(|(_, v)| v))
        .unwrap_or(trimmed);
    value.trim().parse().ok()
}

/// Parse a `findNextFile` reply into `(found_count, spans)`.
///
/// The body is `found=<n>` followed by `items[i].StartTime=YYYY-MM-DD HH:MM:SS`
/// / `items[i].EndTime=...` lines (device-local, read as UTC). A span is kept
/// only when both its timestamps parse and end is after start.
fn parse_media_items(text: &str) -> (usize, Vec<RecordSpan>) {
    let mut found = 0usize;
    let mut starts: BTreeMap<usize, DateTime<Utc>> = BTreeMap::new();
    let mut ends: BTreeMap<usize, DateTime<Utc>> = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "found" {
            found = value.parse().unwrap_or(found);
            continue;
        }
        let Some((index, field)) = parse_item_index(key) else {
            continue;
        };
        match field {
            "StartTime" => {
                if let Some(ts) = parse_dahua_time(value) {
                    starts.insert(index, ts);
                }
            }
            "EndTime" => {
                if let Some(ts) = parse_dahua_time(value) {
                    ends.insert(index, ts);
                }
            }
            _ => {}
        }
    }
    let spans = starts
        .into_iter()
        .filter_map(|(index, start)| {
            let end = *ends.get(&index)?;
            (end > start).then_some(RecordSpan {
                start,
                end,
                channel: None,
            })
        })
        .collect();
    (found, spans)
}

/// Extract the numeric index from an `items[<i>].<Field>` key.
fn parse_item_index(key: &str) -> Option<(usize, &str)> {
    let rest = key.strip_prefix("items[")?;
    let close = rest.find(']')?;
    let index: usize = rest[..close].parse().ok()?;
    let field = rest[close + 1..].strip_prefix('.')?;
    Some((index, field))
}

/// Parse a device-local `YYYY-MM-DD HH:MM:SS` timestamp as UTC (the device has
/// no zone in this API; the rest of the driver treats naive times as UTC).
fn parse_dahua_time(value: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn parse_factory_object_reads_result_or_bare_number() {
        assert_eq!(parse_factory_object("result=1465967\r\n"), Some(1465967));
        assert_eq!(parse_factory_object("42\r\n"), Some(42));
        assert_eq!(parse_factory_object("garbage"), None);
    }

    #[test]
    fn parse_media_items_pairs_start_and_end_per_index() {
        let body = "found=2\r\n\
items[0].Channel=1\r\n\
items[0].StartTime=2026-07-01 08:00:00\r\n\
items[0].EndTime=2026-07-01 09:00:00\r\n\
items[0].Type=dav\r\n\
items[1].StartTime=2026-07-01 09:00:00\r\n\
items[1].EndTime=2026-07-01 10:30:00\r\n";
        let (found, spans) = parse_media_items(body);
        assert_eq!(found, 2);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[0].start,
            parse_dahua_time("2026-07-01 08:00:00").unwrap()
        );
        assert_eq!(
            spans[1].end,
            parse_dahua_time("2026-07-01 10:30:00").unwrap()
        );
        // Aggregation the driver performs: oldest/newest across spans.
        assert_eq!(spans.iter().map(|s| s.start).min(), Some(spans[0].start));
        assert_eq!(spans.iter().map(|s| s.end).max(), Some(spans[1].end));
    }

    #[test]
    fn parse_media_items_drops_incomplete_or_inverted_spans() {
        // index 0 has only a StartTime; index 1 has end<=start — both dropped.
        let body = "found=2\r\n\
items[0].StartTime=2026-07-01 08:00:00\r\n\
items[1].StartTime=2026-07-01 10:00:00\r\n\
items[1].EndTime=2026-07-01 09:00:00\r\n";
        let (_found, spans) = parse_media_items(body);
        assert!(spans.is_empty());
    }

    #[test]
    fn dahua_time_param_encodes_the_space() {
        let ts = parse_dahua_time("2026-07-01 08:30:00").unwrap();
        assert_eq!(dahua_time_param(ts), "2026-07-01%2008:30:00");
    }

    const SAMPLE_SYSTEM_INFO: &str = "\
serialNumber=3J04ABCPAZ12345\r
processor=SSC335\r
";
    const SAMPLE_DEVICE_TYPE: &str = "type=DHI-XVR4116HS-S2\r\n";
    const SAMPLE_SOFTWARE_VERSION: &str = "version=3.218.0000002.1,build:2017-12-29\r\n";

    const SAMPLE_USERS: &str = "\
users[0].Name=admin\r
users[0].Group=admin\r
users[0].Memo=admin 's account\r
users[0].Sharable=true\r
users[0].AuthorityList[0]=System\r
users[0].AuthorityList[1]=Monitor_01\r
users[1].Name=viewer\r
users[1].Group=user\r
users[1].AuthorityList[0]=Monitor_01\r
";

    const SAMPLE_STORAGE: &str = "\
list.info[0].Detail[0].TotalBytes=1000204886016\r
list.info[0].Detail[0].UsedBytes=500000000000\r
list.info[0].Detail[0].IsError=false\r
list.info[0].Detail[0].Path=/dev/sda0\r
list.info[0].Detail[0].Type=ReadWrite\r
list.info[0].Detail[1].TotalBytes=2000409772032\r
list.info[0].Detail[1].UsedBytes=100000000000\r
list.info[0].Detail[1].IsError=true\r
list.info[0].Detail[1].Path=/dev/sdb0\r
list.info[0].Detail[1].Type=ReadWrite\r
list.info[0].Detail[2].TotalBytes=0\r
list.info[0].Detail[2].IsError=false\r
";

    /// A canned [`HttpDeviceClient`] that replays fixed text (and, for snapshots,
    /// fixed bytes) per request path.
    struct FakeHttpClient {
        responses: HashMap<String, String>,
        byte_responses: HashMap<String, Vec<u8>>,
    }

    impl FakeHttpClient {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
                    .collect(),
                byte_responses: HashMap::new(),
            }
        }

        /// Attach canned binary bodies keyed by request path, for `get_bytes`.
        fn with_bytes(mut self, pairs: &[(&str, &[u8])]) -> Self {
            self.byte_responses = pairs
                .iter()
                .map(|(path, body)| ((*path).to_owned(), (*body).to_vec()))
                .collect();
            self
        }
    }

    #[async_trait]
    impl HttpDeviceClient for FakeHttpClient {
        async fn get_text(&self, path: &str) -> Result<String, DriverError> {
            self.responses
                .get(path)
                .cloned()
                .ok_or_else(|| DriverError::Device(format!("no canned response for {path}")))
        }

        async fn get_bytes(&self, path: &str) -> Result<Vec<u8>, DriverError> {
            let bytes = self
                .byte_responses
                .get(path)
                .cloned()
                .ok_or_else(|| DriverError::Device(format!("no canned bytes for {path}")))?;
            // Faithfully double the real client's port contract: `get_bytes`
            // yields a validated JPEG, rejecting a non-image body.
            if bytes.starts_with(&[0xFF, 0xD8]) {
                Ok(bytes)
            } else {
                Err(DriverError::Device(
                    "snapshot response was not a JPEG image".to_owned(),
                ))
            }
        }
    }

    fn driver_with(pairs: &[(&str, &str)]) -> DahuaDriver {
        DahuaDriver::new(
            Arc::new(FakeHttpClient::new(pairs)),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        )
    }

    /// An [`HttpDeviceClient`] that records every requested path verbatim (query
    /// included) and answers each request from a caller-supplied closure, so a
    /// test can assert the exact url-encoded write query and drive per-item
    /// success/failure by inspecting the path.
    /// Response callback driving per-path success/failure in tests.
    type BoxedResponder = Box<dyn Fn(&str) -> Result<String, DriverError> + Send + Sync>;

    struct RecordingHttpClient {
        requests: Mutex<Vec<String>>,
        responder: BoxedResponder,
    }

    impl RecordingHttpClient {
        fn new(
            responder: impl Fn(&str) -> Result<String, DriverError> + Send + Sync + 'static,
        ) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responder: Box::new(responder),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().expect("requests mutex").clone()
        }
    }

    #[async_trait]
    impl HttpDeviceClient for RecordingHttpClient {
        async fn get_text(&self, path: &str) -> Result<String, DriverError> {
            self.requests
                .lock()
                .expect("requests mutex")
                .push(path.to_owned());
            (self.responder)(path)
        }
    }

    /// Build a driver over a [`RecordingHttpClient`], returning both so the test
    /// can inspect the recorded requests.
    fn recording_driver(
        responder: impl Fn(&str) -> Result<String, DriverError> + Send + Sync + 'static,
    ) -> (DahuaDriver, Arc<RecordingHttpClient>) {
        let client = Arc::new(RecordingHttpClient::new(responder));
        let driver = DahuaDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );
        (driver, client)
    }

    fn new_user(name: &str, password: &str) -> NewUser {
        NewUser {
            name: name.to_owned(),
            password: Secret::new(password),
            group: None,
            authorities: Vec::new(),
        }
    }

    #[test]
    fn parse_system_info_combines_three_cgis() {
        let info = parse_system_info(
            SAMPLE_SYSTEM_INFO,
            SAMPLE_DEVICE_TYPE,
            SAMPLE_SOFTWARE_VERSION,
        );
        assert_eq!(info.model.as_deref(), Some("DHI-XVR4116HS-S2"));
        assert_eq!(info.firmware.as_deref(), Some("3.218.0000002.1"));
        assert_eq!(info.serial.as_deref(), Some("3J04ABCPAZ12345"));
        assert_eq!(info.channels, None);
        assert_eq!(info.device_time, None);
    }

    #[test]
    fn parse_users_groups_by_index_and_orders_authorities() {
        let users = parse_users(SAMPLE_USERS);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "admin");
        assert_eq!(users[0].group.as_deref(), Some("admin"));
        assert_eq!(users[0].authorities, ["System", "Monitor_01"]);
        assert_eq!(users[1].name, "viewer");
        assert_eq!(users[1].group.as_deref(), Some("user"));
        assert_eq!(users[1].authorities, ["Monitor_01"]);
    }

    #[test]
    fn parse_disks_maps_direct_bytes_and_filters_empty_slots() {
        let disks = parse_disks(SAMPLE_STORAGE);
        // The zero-total third detail is dropped.
        assert_eq!(disks.len(), 2);

        assert_eq!(disks[0].name, "/dev/sda0");
        assert_eq!(disks[0].total_bytes, 1_000_204_886_016);
        assert_eq!(disks[0].used_bytes, 500_000_000_000);
        assert_eq!(disks[0].state, DiskState::Ok);
        assert!(!disks[0].is_error);

        assert_eq!(disks[1].name, "/dev/sdb0");
        assert_eq!(disks[1].state, DiskState::Error);
        assert!(disks[1].is_error);
    }

    #[test]
    fn parse_channels_maps_channel_titles_to_one_based_channels() {
        let text = "\
table.ChannelTitle[0].Name=Entrance\r
table.ChannelTitle[1].Name=\r
table.ChannelTitle[2].Name=Register 3\r
table.ChannelTitle[2].EncodeBlend=true\r
";
        let channels = parse_channels(text);
        assert_eq!(channels.len(), 3);
        assert_eq!(channels[0].index, 1);
        assert_eq!(channels[0].name.as_deref(), Some("Entrance"));
        // Blank title -> nameless channel; the non-`.Name` sibling is ignored.
        assert_eq!(channels[1].index, 2);
        assert_eq!(channels[1].name, None);
        assert_eq!(channels[2].index, 3);
        assert_eq!(channels[2].name.as_deref(), Some("Register 3"));
    }

    #[test]
    fn apply_ptz_support_marks_supported_and_unsupported_from_enable_flag() {
        let text = "\
table.PTZProtocol[0].Address=1\r
table.PTZProtocol[0].Enable=true\r
table.PTZProtocol[1].Address=2\r
table.PTZProtocol[1].Enable=false\r
";
        let mut channels = vec![
            Channel {
                index: 1,
                name: None,
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
            Channel {
                index: 2,
                name: None,
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
            Channel {
                index: 3,
                name: None,
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
        ];
        apply_ptz_support(&mut channels, text);
        assert_eq!(channels[0].ptz_support, PtzSupport::Supported);
        assert_eq!(channels[1].ptz_support, PtzSupport::Unsupported);
        // No PTZProtocol[2] entry in the config body -> stays Unknown, not
        // downgraded to Unsupported.
        assert_eq!(channels[2].ptz_support, PtzSupport::Unknown);
    }

    #[tokio::test]
    async fn list_channels_enriches_with_ptz_support_from_the_protocol_config() {
        let driver = driver_with(&[
            (
                PATH_CHANNEL_TITLE,
                "table.ChannelTitle[0].Name=Entrance\r\ntable.ChannelTitle[1].Name=Back\r\n",
            ),
            (
                PATH_PTZ_PROTOCOL,
                "table.PTZProtocol[0].Enable=true\r\ntable.PTZProtocol[1].Enable=false\r\n",
            ),
        ]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels[0].ptz_support, PtzSupport::Supported);
        assert_eq!(channels[1].ptz_support, PtzSupport::Unsupported);
    }

    #[tokio::test]
    async fn list_channels_leaves_ptz_support_unknown_when_the_protocol_query_fails() {
        let driver = driver_with(&[(
            PATH_CHANNEL_TITLE,
            "table.ChannelTitle[0].Name=Entrance\r\n",
        )]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels[0].ptz_support, PtzSupport::Unknown);
    }

    #[test]
    fn channel_title_index_reads_only_the_name_leaf() {
        assert_eq!(channel_title_index("table.ChannelTitle[4].Name"), Some(4));
        assert_eq!(
            channel_title_index("table.ChannelTitle[4].EncodeBlend"),
            None
        );
        assert_eq!(channel_title_index("table.Other[0].Name"), None);
    }

    fn vector(pan: f32, tilt: f32, zoom: f32) -> PtzVector {
        PtzVector { pan, tilt, zoom }
    }

    #[test]
    fn dahua_move_code_maps_axes_zoom_and_clamps_speed() {
        // An idle vector is a no-op.
        assert_eq!(dahua_move_code(vector(0.0, 0.0, 0.0)), None);
        // A single axis maps to a cardinal code; speed scales onto 1..=8.
        assert_eq!(
            dahua_move_code(vector(-0.5, 0.0, 0.0)),
            Some(("Left".to_owned(), 4))
        );
        // Pan+tilt combine horizontal-then-vertical, at full speed.
        assert_eq!(
            dahua_move_code(vector(1.0, 1.0, 0.0)),
            Some(("RightUp".to_owned(), 8))
        );
        // Zoom wins when it is the largest-magnitude axis.
        assert_eq!(
            dahua_move_code(vector(0.2, 0.0, 0.9)),
            Some(("ZoomTele".to_owned(), 7))
        );
        assert_eq!(
            dahua_move_code(vector(0.0, -0.3, -0.4)),
            Some(("ZoomWide".to_owned(), 3))
        );
    }

    #[tokio::test]
    async fn ptz_move_issues_start_with_direction_channel_and_speed() {
        let (driver, client) = recording_driver(|_path| Ok("OK".to_owned()));
        driver
            .ptz(2, PtzCommand::Move(vector(1.0, 0.0, 0.0)))
            .await
            .expect("ptz move");
        let requests = client.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].contains("action=start&channel=2&code=Right"),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains("arg2=8"), "{}", requests[0]);
    }

    #[tokio::test]
    async fn ptz_stop_halts_every_axis_even_if_one_errors() {
        // One code errors; Stop must still attempt all six.
        let (driver, client) = recording_driver(|path| {
            if path.contains("code=Left") {
                Err(DriverError::Device("busy".to_owned()))
            } else {
                Ok("OK".to_owned())
            }
        });
        driver.ptz(1, PtzCommand::Stop).await.expect("ptz stop");
        let requests = client.requests();
        assert_eq!(requests.len(), 6);
        assert!(requests.iter().all(|path| path.contains("action=stop")));
    }

    #[tokio::test]
    async fn ptz_preset_recalls_by_id() {
        let (driver, client) = recording_driver(|_path| Ok("OK".to_owned()));
        driver
            .ptz(3, PtzCommand::Preset { id: 5 })
            .await
            .expect("ptz preset");
        assert!(
            client.requests()[0].contains("code=GotoPreset&arg1=0&arg2=5"),
            "{}",
            client.requests()[0]
        );
    }

    #[tokio::test]
    async fn ptz_idle_move_is_a_no_op() {
        let (driver, client) = recording_driver(|_path| Ok("OK".to_owned()));
        driver
            .ptz(1, PtzCommand::Move(vector(0.0, 0.0, 0.0)))
            .await
            .expect("idle ptz");
        assert!(client.requests().is_empty());
    }

    #[test]
    fn parse_disks_infers_read_only_state_from_type() {
        let text = "\
list.info[0].Detail[0].TotalBytes=100\r
list.info[0].Detail[0].UsedBytes=10\r
list.info[0].Detail[0].IsError=false\r
list.info[0].Detail[0].Type=ReadOnly\r
";
        let disks = parse_disks(text);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].state, DiskState::ReadOnly);
        // A read-only-but-healthy disk is not flagged as an error.
        assert!(!disks[0].is_error);
    }

    #[test]
    fn parse_disks_falls_back_to_synthetic_name_when_path_absent() {
        let text = "\
list.info[1].Detail[0].TotalBytes=100\r
list.info[1].Detail[0].UsedBytes=10\r
list.info[1].Detail[0].IsError=false\r
";
        let disks = parse_disks(text);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].name, "disk1-0");
    }

    #[tokio::test]
    async fn get_info_maps_the_three_identity_cgis() {
        let driver = driver_with(&[
            (PATH_SYSTEM_INFO, SAMPLE_SYSTEM_INFO),
            (PATH_DEVICE_TYPE, SAMPLE_DEVICE_TYPE),
            (PATH_SOFTWARE_VERSION, SAMPLE_SOFTWARE_VERSION),
        ]);
        let info = driver.get_info().await.expect("get_info");
        assert_eq!(info.model.as_deref(), Some("DHI-XVR4116HS-S2"));
        assert_eq!(info.serial.as_deref(), Some("3J04ABCPAZ12345"));
    }

    #[tokio::test]
    async fn list_users_maps_the_user_cgi() {
        let driver = driver_with(&[(PATH_USERS, SAMPLE_USERS)]);
        let users = driver.list_users().await.expect("list_users");
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "admin");
    }

    #[tokio::test]
    async fn disk_status_maps_the_storage_cgi() {
        let driver = driver_with(&[(PATH_STORAGE, SAMPLE_STORAGE)]);
        let disks = driver.disk_status().await.expect("disk_status");
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].total_bytes, 1_000_204_886_016);
    }

    #[tokio::test]
    async fn recording_status_walks_the_mediafilefind_loop() {
        // Drive the full factory → findFile → findNextFile → close/destroy cycle
        // by dispatching on the request's `action`. The first findNextFile
        // returns a short page (< page size), so the loop ends after one batch.
        let (driver, client) = recording_driver(|path| {
            if path.contains("action=factory.create") {
                Ok("result=1465967\r\n".to_owned())
            } else if path.contains("action=findFile") {
                Ok("OK\r\n".to_owned())
            } else if path.contains("action=findNextFile") {
                Ok("found=2\r\n\
items[0].Channel=1\r\n\
items[0].StartTime=2026-07-01 08:00:00\r\n\
items[0].EndTime=2026-07-01 09:00:00\r\n\
items[1].StartTime=2026-07-01 09:00:00\r\n\
items[1].EndTime=2026-07-01 10:30:00\r\n"
                    .to_owned())
            } else {
                Ok(String::new()) // close / destroy
            }
        });

        let status = driver.recording_status().await.expect("recording status");
        assert_eq!(status.spans.len(), 2);
        assert_eq!(
            status.oldest,
            Some(parse_dahua_time("2026-07-01 08:00:00").unwrap())
        );
        assert_eq!(
            status.newest,
            Some(parse_dahua_time("2026-07-01 10:30:00").unwrap())
        );
        // Newest span is far in the past → not currently recording.
        assert!(!status.active);

        // The handle is always released: a close and a destroy were issued.
        let reqs = client.requests();
        assert!(reqs.iter().any(|r| r.contains("action=close")));
        assert!(reqs.iter().any(|r| r.contains("action=destroy")));
        assert!(driver.capabilities().recording);
    }

    /// `search_recordings` reuses the exact `collect_channel_recording`
    /// primitive `recording_status` loops over per channel — one cycle,
    /// keyed to the caller's channel, attributed back to it (which the
    /// device-wide aggregate cannot do since it merges every channel).
    #[tokio::test]
    async fn search_recordings_attributes_spans_to_the_requested_channel() {
        let (driver, client) = recording_driver(|path| {
            if path.contains("action=factory.create") {
                Ok("result=1465967\r\n".to_owned())
            } else if path.contains("action=findFile") {
                Ok("OK\r\n".to_owned())
            } else if path.contains("action=findNextFile") {
                Ok("found=1\r\n\
items[0].Channel=3\r\n\
items[0].StartTime=2026-07-01 08:00:00\r\n\
items[0].EndTime=2026-07-01 09:00:00\r\n"
                    .to_owned())
            } else {
                Ok(String::new())
            }
        });

        let request = ExportRequest {
            channel: 3,
            start: "2026-07-01T00:00:00Z".parse().expect("start"),
            end: "2026-07-02T00:00:00Z".parse().expect("end"),
        };
        let spans = driver.search_recordings(&request).await.expect("search");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].channel, Some(3));

        // Only the requested channel's cycle ran — one factory.create, not one
        // per channel like the device-wide recording_status walk.
        let reqs = client.requests();
        assert_eq!(
            reqs.iter()
                .filter(|r| r.contains("action=factory.create"))
                .count(),
            1
        );
        assert!(driver.capabilities().recording_search);
    }

    #[tokio::test]
    async fn page_media_find_stops_at_the_page_cap_when_the_device_never_signals_exhaustion() {
        // Every findNextFile reply reports a full page (`found` ==
        // MEDIA_FIND_PAGE) with exactly one item, so nothing but the
        // iteration cap can end the loop — a device that never signals
        // exhaustion must not be paged forever.
        let (driver, client) = recording_driver(|path| {
            if path.contains("action=findNextFile") {
                Ok(format!(
                    "found={MEDIA_FIND_PAGE}\r\n\
items[0].StartTime=2026-01-01 00:00:00\r\n\
items[0].EndTime=2026-01-01 00:00:01\r\n"
                ))
            } else {
                Ok("OK\r\n".to_owned())
            }
        });
        let start = parse_dahua_time("2026-01-01 00:00:00").unwrap();
        let end = parse_dahua_time("2026-01-02 00:00:00").unwrap();

        let spans = driver
            .page_media_find(1, 1, start, end)
            .await
            .expect("page_media_find");
        assert_eq!(spans.len(), MEDIA_FIND_MAX_PAGES as usize);

        let page_requests = client
            .requests()
            .iter()
            .filter(|r| r.contains("action=findNextFile"))
            .count();
        assert_eq!(page_requests, MEDIA_FIND_MAX_PAGES as usize);
    }

    #[tokio::test]
    async fn page_media_find_caps_total_spans_even_when_one_page_reports_far_more_items_than_requested()
     {
        // A single reply claims a "full" page (`found` == MEDIA_FIND_PAGE, so
        // the short-page-exhausted check alone would keep paging) but stuffs
        // far more than MEDIA_FIND_PAGE items into it. The span cap — not the
        // page cap — must end the loop, and the accumulated result must never
        // exceed it even though the parsed page itself did.
        let oversized_items: String = (0..MEDIA_FIND_MAX_SPANS + 50)
            .map(|i| {
                format!(
                    "items[{i}].StartTime=2026-01-01 00:00:00\r\nitems[{i}].EndTime=2026-01-01 00:00:01\r\n"
                )
            })
            .collect();
        let body = format!("found={MEDIA_FIND_PAGE}\r\n{oversized_items}");
        let (driver, client) = recording_driver(move |path| {
            if path.contains("action=findNextFile") {
                Ok(body.clone())
            } else {
                Ok("OK\r\n".to_owned())
            }
        });
        let start = parse_dahua_time("2026-01-01 00:00:00").unwrap();
        let end = parse_dahua_time("2026-01-02 00:00:00").unwrap();

        let spans = driver
            .page_media_find(1, 1, start, end)
            .await
            .expect("page_media_find");
        assert_eq!(spans.len(), MEDIA_FIND_MAX_SPANS);

        // The oversized first page alone exceeds the cap, so a single page
        // (not 200 of them) ends the loop.
        let page_requests = client
            .requests()
            .iter()
            .filter(|r| r.contains("action=findNextFile"))
            .count();
        assert_eq!(page_requests, 1);
    }

    #[tokio::test]
    async fn recording_status_aggregates_across_channels_so_one_idle_channel_does_not_blank_the_device()
     {
        // Channel 1's archive is old (would report inactive alone); channel 2
        // has a span ending seconds ago. The aggregate must be active and
        // must report channel 2's span, not just channel 1's.
        let now = Utc::now();
        let recent_end = now - chrono::Duration::seconds(30);
        let recent_start = recent_end - chrono::Duration::hours(1);
        let old_start = parse_dahua_time("2020-01-01 00:00:00").unwrap();
        let old_end = parse_dahua_time("2020-01-01 01:00:00").unwrap();

        let current_channel = Arc::new(std::sync::atomic::AtomicU16::new(0));
        let cc = current_channel.clone();
        let (driver, _client) = recording_driver(move |path| {
            if path == PATH_CHANNEL_TITLE {
                Ok(
                    "table.ChannelTitle[0].Name=Front\r\ntable.ChannelTitle[1].Name=Back\r\n"
                        .to_owned(),
                )
            } else if path.contains("action=factory.create") {
                Ok("result=1\r\n".to_owned())
            } else if path.contains("action=findFile") {
                if path.contains("condition.Channel=1&") {
                    cc.store(1, std::sync::atomic::Ordering::SeqCst);
                } else if path.contains("condition.Channel=2&") {
                    cc.store(2, std::sync::atomic::Ordering::SeqCst);
                }
                Ok("OK\r\n".to_owned())
            } else if path.contains("action=findNextFile") {
                match cc.load(std::sync::atomic::Ordering::SeqCst) {
                    1 => Ok(format!(
                        "found=1\r\nitems[0].StartTime={}\r\nitems[0].EndTime={}\r\n",
                        old_start.format("%Y-%m-%d %H:%M:%S"),
                        old_end.format("%Y-%m-%d %H:%M:%S"),
                    )),
                    2 => Ok(format!(
                        "found=1\r\nitems[0].StartTime={}\r\nitems[0].EndTime={}\r\n",
                        recent_start.format("%Y-%m-%d %H:%M:%S"),
                        recent_end.format("%Y-%m-%d %H:%M:%S"),
                    )),
                    _ => Ok("found=0\r\n".to_owned()),
                }
            } else {
                Ok(String::new()) // close / destroy
            }
        });

        // The device wire format is second-precision (`%Y-%m-%d %H:%M:%S`), so
        // the driver can only ever parse back second-precision timestamps — the
        // expected values must go through the same truncation, not compare
        // against the raw `Utc::now()`-derived values (which carry nanoseconds).
        let recent_end_wire =
            parse_dahua_time(&recent_end.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap();

        let status = driver.recording_status().await.expect("recording status");
        assert!(status.active);
        assert_eq!(status.spans.len(), 2);
        assert_eq!(status.oldest, Some(old_start));
        assert_eq!(status.newest, Some(recent_end_wire));
    }

    #[tokio::test]
    async fn recording_status_skips_a_channel_that_errors_but_keeps_aggregating_the_rest() {
        // Channel 1's `findFile` is rejected with a device-level error (not a
        // transport failure); channel 2 still succeeds and its span still
        // surfaces in the aggregate.
        let now = Utc::now();
        let recent_end = now - chrono::Duration::seconds(30);
        let recent_start = recent_end - chrono::Duration::hours(1);
        let (driver, _client) = recording_driver(move |path| {
            if path == PATH_CHANNEL_TITLE {
                Ok(
                    "table.ChannelTitle[0].Name=Front\r\ntable.ChannelTitle[1].Name=Back\r\n"
                        .to_owned(),
                )
            } else if path.contains("action=factory.create") {
                Ok("result=1\r\n".to_owned())
            } else if path.contains("action=findFile") {
                if path.contains("condition.Channel=1&") {
                    return Err(DriverError::Device("channel busy".to_owned()));
                }
                Ok("OK\r\n".to_owned())
            } else if path.contains("action=findNextFile") {
                Ok(format!(
                    "found=1\r\nitems[0].StartTime={}\r\nitems[0].EndTime={}\r\n",
                    recent_start.format("%Y-%m-%d %H:%M:%S"),
                    recent_end.format("%Y-%m-%d %H:%M:%S"),
                ))
            } else {
                Ok(String::new()) // close / destroy
            }
        });

        let status = driver
            .recording_status()
            .await
            .expect("recording status despite one bad channel");
        assert_eq!(status.spans.len(), 1);
        assert!(status.active);
    }

    #[tokio::test]
    async fn recording_status_aborts_immediately_on_a_transport_error_from_any_channel() {
        let (driver, client) = recording_driver(|path| {
            if path == PATH_CHANNEL_TITLE {
                Ok(
                    "table.ChannelTitle[0].Name=Front\r\ntable.ChannelTitle[1].Name=Back\r\n"
                        .to_owned(),
                )
            } else if path.contains("action=factory.create") {
                Ok("result=1\r\n".to_owned())
            } else if path.contains("action=findFile") {
                Err(DriverError::Transport("connection refused".to_owned()))
            } else {
                Ok(String::new())
            }
        });

        let result = driver.recording_status().await;
        assert!(matches!(result, Err(DriverError::Transport(_))));
        // The transport error on channel 1 aborts the walk before channel 2
        // is ever attempted.
        let requests = client.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.contains("action=findFile"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn snapshot_requests_the_channel_and_returns_the_jpeg_bytes() {
        // A minimal but well-formed JPEG (SOI … EOI).
        let jpeg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0xFF, 0xD9];
        let path = "/cgi-bin/snapshot.cgi?channel=2";
        let driver = DahuaDriver::new(
            Arc::new(FakeHttpClient::new(&[]).with_bytes(&[(path, jpeg)])),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );

        let snap = driver.snapshot(Some(2)).await.expect("snapshot");
        let SnapshotRef::Bytes(bytes) = &snap else {
            panic!("expected inline bytes, got {snap:?}");
        };
        assert_eq!(bytes.as_slice(), jpeg);
        // Debug reports the size, never the driver password.
        assert!(!format!("{snap:?}").contains("secretpw"));
    }

    #[tokio::test]
    async fn snapshot_defaults_to_channel_one() {
        let jpeg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xD9];
        let driver = DahuaDriver::new(
            Arc::new(
                FakeHttpClient::new(&[]).with_bytes(&[("/cgi-bin/snapshot.cgi?channel=1", jpeg)]),
            ),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );

        let snap = driver.snapshot(None).await.expect("snapshot");
        assert!(matches!(snap, SnapshotRef::Bytes(bytes) if bytes == jpeg));
    }

    #[tokio::test]
    async fn snapshot_rejects_a_non_jpeg_body() {
        let html: &[u8] = b"<html>401 Unauthorized</html>";
        let driver = DahuaDriver::new(
            Arc::new(
                FakeHttpClient::new(&[]).with_bytes(&[("/cgi-bin/snapshot.cgi?channel=1", html)]),
            ),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );

        let error = driver
            .snapshot(None)
            .await
            .expect_err("a non-JPEG body must be rejected");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[test]
    fn response_reports_ok_requires_an_exact_match_not_a_substring() {
        assert!(response_reports_ok("OK"));
        assert!(response_reports_ok("OK\r\n"));
        assert!(response_reports_ok("  OK  \r\n"));
        // A body that merely contains "OK" — a status line, a token, or an
        // unrelated HTML page — must not be misread as success.
        assert!(!response_reports_ok("200 OK"));
        assert!(!response_reports_ok("<html>TOKEN=abc123 OK</html>"));
        assert!(!response_reports_ok("NOTOK"));
        assert!(!response_reports_ok(""));
    }

    #[tokio::test]
    async fn create_users_rejects_a_200_ok_html_body_as_failure_not_success() {
        // A proxy/reboot HTML page (or any body that merely contains "OK")
        // must not be misread as a successful create.
        let (driver, _client) =
            recording_driver(
                |_path| Ok("<html><body>200 OK - proxy page</body></html>".to_owned()),
            );
        let outcome = driver
            .create_users(&[new_user("cam", "pw")])
            .await
            .expect("create_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "cam");
    }

    #[tokio::test]
    async fn write_ops_on_empty_input_are_no_ops() {
        let driver = driver_with(&[]);
        let created = driver.create_users(&[]).await.expect("create_users");
        assert!(created.succeeded.is_empty() && created.failed.is_empty());
        let deleted = driver.delete_users(&[]).await.expect("delete_users");
        assert!(deleted.succeeded.is_empty() && deleted.failed.is_empty());
    }

    #[tokio::test]
    async fn stream_uri_builds_a_dahua_rtsp_url_without_leaking_the_password() {
        let driver = driver_with(&[]);
        let uri = driver.stream_uri(Some(3)).await.expect("stream_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("device.test:554/cam/realmonitor"));
        assert!(uri.uri.contains("channel=3"));
        assert!(uri.uri.contains("subtype=0"));
        // The URL embeds credentials, but Debug must redact them.
        assert!(!format!("{uri:?}").contains("secretpw"));
    }

    #[tokio::test]
    async fn substream_uri_builds_a_dahua_rtsp_url_with_subtype_1() {
        let driver = driver_with(&[]);
        let uri = driver.substream_uri(Some(3)).await.expect("substream_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("device.test:554/cam/realmonitor"));
        assert!(uri.uri.contains("channel=3"));
        assert!(uri.uri.contains("subtype=1"));
        // The URL embeds credentials, but Debug must redact them.
        assert!(!format!("{uri:?}").contains("secretpw"));
    }

    #[tokio::test]
    async fn playback_uri_builds_a_time_ranged_dahua_url() {
        let driver = driver_with(&[]);
        let request = ExportRequest {
            channel: 2,
            start: parse_dahua_time("2026-07-01 08:00:00").unwrap(),
            end: parse_dahua_time("2026-07-01 09:00:00").unwrap(),
        };
        let uri = driver.playback_uri(&request).await.expect("playback_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("/cam/playback?channel=2"));
        assert!(uri.uri.contains("starttime=2026_07_01_08_00_00"));
        assert!(uri.uri.contains("endtime=2026_07_01_09_00_00"));
        assert!(!format!("{uri:?}").contains("secretpw"));
        assert!(driver.capabilities().export);
    }

    #[tokio::test]
    async fn stream_uri_percent_encodes_userinfo_so_special_characters_cannot_break_the_url() {
        let driver = DahuaDriver::new(
            Arc::new(FakeHttpClient::new(&[])),
            DeviceEndpoint::new("device.test", 80),
            "ad min",
            Secret::new("p@ss:w/o?rd#1"),
        );
        let uri = driver.stream_uri(Some(1)).await.expect("stream_uri");
        // The raw '@'/':'/'/'/'?'/'#' and whitespace never appear unescaped in
        // the userinfo — only the deliberate `user:password@host` structure.
        assert!(
            uri.uri
                .starts_with("rtsp://ad%20min:p%40ss%3Aw%2Fo%3Frd%231@device.test:554/"),
            "uri: {}",
            uri.uri
        );
        // Debug must redact the whole userinfo. Before percent-encoding, a
        // password containing '@' would mis-split the redaction's first-'@'
        // search and leak a fragment of it; that must not happen now.
        let debug = format!("{uri:?}");
        assert!(!debug.contains("p@ss"));
        assert!(!debug.contains("ad min"));
        assert!(!debug.contains("p%40ss"));
    }

    #[tokio::test]
    async fn playback_uri_percent_encodes_userinfo_so_special_characters_cannot_break_the_url() {
        let driver = DahuaDriver::new(
            Arc::new(FakeHttpClient::new(&[])),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("a:b@c"),
        );
        let request = ExportRequest {
            channel: 1,
            start: parse_dahua_time("2026-07-01 08:00:00").unwrap(),
            end: parse_dahua_time("2026-07-01 09:00:00").unwrap(),
        };
        let uri = driver.playback_uri(&request).await.expect("playback_uri");
        assert!(
            uri.uri
                .starts_with("rtsp://admin:a%3Ab%40c@device.test:554/"),
            "uri: {}",
            uri.uri
        );
        assert!(!format!("{uri:?}").contains("a:b@c"));
    }

    #[tokio::test]
    async fn capabilities_reflect_the_implemented_read_plane() {
        let driver = driver_with(&[]);
        let capabilities = driver.capabilities();
        assert!(capabilities.info);
        assert!(capabilities.users);
        assert!(capabilities.disk);
        assert!(capabilities.streaming);
        assert!(capabilities.snapshot);
        assert!(capabilities.recording);
    }

    #[tokio::test]
    async fn create_users_builds_url_encoded_adduser_query() {
        let (driver, client) = recording_driver(|_path| Ok("OK\r\n".to_owned()));
        let user = NewUser {
            name: "guard1".to_owned(),
            // A password whose reserved characters must not survive as query syntax.
            password: Secret::new("a&b=c"),
            group: Some("admin".to_owned()),
            authorities: vec!["Monitor".to_owned(), "Playback".to_owned()],
        };
        let outcome = driver
            .create_users(std::slice::from_ref(&user))
            .await
            .expect("create_users");
        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());

        let requests = client.requests();
        assert_eq!(requests.len(), 1);
        let path = &requests[0];
        assert!(
            path.starts_with("/cgi-bin/userManager.cgi?action=addUser"),
            "path: {path}"
        );
        assert!(path.contains("&user.Name=guard1"), "path: {path}");
        assert!(path.contains("&user.Group=admin"), "path: {path}");
        assert!(path.contains("&user.Sharable=false"), "path: {path}");
        assert!(path.contains("&user.Reserved=false"), "path: {path}");
        // Authority tokens are comma-joined (alphanumeric tokens pass through).
        assert!(
            path.contains("&user.AuthorityList=Monitor,Playback"),
            "path: {path}"
        );
        // The password's `&` and `=` are percent-encoded, never structural.
        assert!(path.contains("&user.Password=a%26b%3Dc"), "path: {path}");
        assert!(!path.contains("a&b=c"), "raw password leaked: {path}");
    }

    #[tokio::test]
    async fn create_users_reports_partial_success() {
        // The first account is accepted (`OK`); the second is rejected with a
        // device error body, which must land as a per-item failure, not a top-level
        // error.
        let (driver, client) = recording_driver(|path| {
            if path.contains("user.Name=ok") {
                Ok("OK\r\n".to_owned())
            } else {
                Ok("Error: user already exists\r\n".to_owned())
            }
        });
        let users = [new_user("ok", "p1"), new_user("dupe", "p2")];
        let outcome = driver.create_users(&users).await.expect("create_users");

        assert_eq!(outcome.succeeded, ["ok"]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "dupe");
        assert!(matches!(
            &outcome.failed[0].1,
            DriverError::Device(message) if message == "Error: user already exists"
        ));
        // The default group is applied to accounts that carry none.
        assert!(
            client
                .requests()
                .iter()
                .all(|path| path.contains("&user.Group=user"))
        );
    }

    #[tokio::test]
    async fn create_users_records_non_transport_error_as_failed_entry() {
        // A per-request `Device` error must not abort the loop: both users are
        // attempted and both recorded as failures.
        let (driver, client) = recording_driver(|_path| {
            Err(DriverError::Device("device returned HTTP 400".to_owned()))
        });
        let users = [new_user("a", "p"), new_user("b", "p")];
        let outcome = driver
            .create_users(&users)
            .await
            .expect("device error is per-item, not fatal");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 2);
        assert_eq!(client.requests().len(), 2);
    }

    #[tokio::test]
    async fn create_users_aborts_on_transport_error() {
        // A transport failure stops all work: the loop returns a top-level error
        // and does not attempt the remaining users.
        let (driver, client) =
            recording_driver(|_path| Err(DriverError::Transport("connection refused".to_owned())));
        let users = [new_user("a", "p"), new_user("b", "p")];
        let result = driver.create_users(&users).await;
        assert!(matches!(result, Err(DriverError::Transport(_))));
        assert_eq!(client.requests().len(), 1);
    }

    #[tokio::test]
    async fn bulk_user_writes_keep_committed_prefix_after_transport_abort() {
        for operation in ["create", "delete", "update"] {
            let (driver, client) = recording_driver(|path| {
                if path.contains("user.Name=second") || path.contains("name=second") {
                    Err(DriverError::Transport("link down".to_owned()))
                } else {
                    Ok("OK\r\n".to_owned())
                }
            });
            let names = ["first".to_owned(), "second".to_owned(), "third".to_owned()];
            let outcome = match operation {
                "create" => {
                    driver
                        .create_users(&names.map(|name| new_user(&name, "pw")))
                        .await
                }
                "delete" => driver.delete_users(&names).await,
                "update" => {
                    driver
                        .update_users(&names.map(|name| UserUpdate {
                            name,
                            password: None,
                            group: Some("operator".to_owned()),
                            authorities: None,
                        }))
                        .await
                }
                _ => Err(DriverError::Unsupported),
            }
            .expect("confirmed write survives fatal link error");
            assert_eq!(outcome.succeeded, ["first"], "{operation}");
            assert_eq!(outcome.failed.len(), 2, "{operation}");
            assert_eq!(outcome.failed[0].0, "second");
            assert!(matches!(outcome.failed[0].1, DriverError::Transport(_)));
            assert_eq!(outcome.failed[1].0, "third");
            assert!(
                matches!(&outcome.failed[1].1, DriverError::Unreachable(reason) if reason == "not attempted after device link failure")
            );
            assert_eq!(
                client.requests().len(),
                2,
                "{operation}: tail must not be attempted"
            );
        }
    }

    #[tokio::test]
    async fn bulk_user_writes_without_acknowledgement_keep_top_level_transport_error() {
        for operation in ["create", "delete", "update"] {
            let (driver, client) =
                recording_driver(|_| Err(DriverError::Transport("link down".to_owned())));
            let names = ["first".to_owned(), "second".to_owned()];
            let result = match operation {
                "create" => {
                    driver
                        .create_users(&names.map(|name| new_user(&name, "pw")))
                        .await
                }
                "delete" => driver.delete_users(&names).await,
                "update" => {
                    driver
                        .update_users(&names.map(|name| UserUpdate {
                            name,
                            password: None,
                            group: Some("operator".to_owned()),
                            authorities: None,
                        }))
                        .await
                }
                _ => Err(DriverError::Unsupported),
            };
            assert!(
                matches!(result, Err(DriverError::Transport(_))),
                "{operation}"
            );
            assert_eq!(client.requests().len(), 1, "{operation}");
        }
    }

    #[tokio::test]
    async fn delete_users_builds_path_and_maps_ok() {
        let (driver, client) = recording_driver(|path| {
            if path.contains("name=missing") {
                Ok("Error: no such user\r\n".to_owned())
            } else {
                Ok("OK\r\n".to_owned())
            }
        });
        let outcome = driver
            .delete_users(&["guard 1".to_owned(), "missing".to_owned()])
            .await
            .expect("delete_users");

        assert_eq!(outcome.succeeded, ["guard 1"]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "missing");

        let requests = client.requests();
        assert!(
            requests[0].starts_with("/cgi-bin/userManager.cgi?action=deleteUser&name="),
            "path: {}",
            requests[0]
        );
        // The space in the username is percent-encoded in the delete path.
        assert!(
            requests[0].contains("name=guard%201"),
            "path: {}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn create_users_never_leaks_plaintext_password() {
        let (driver, client) = recording_driver(|_path| Ok("OK".to_owned()));
        let secret_pw = "t0p&secret=x";
        let user = NewUser {
            name: "cam".to_owned(),
            password: Secret::new(secret_pw),
            group: None,
            authorities: Vec::new(),
        };
        // The request DTO's Debug must redact the plaintext.
        assert!(!format!("{user:?}").contains(secret_pw));

        driver
            .create_users(std::slice::from_ref(&user))
            .await
            .expect("create_users");

        let path = client.requests().remove(0);
        // Only the percent-encoded form may appear in the URL; never the plaintext.
        assert!(
            !path.contains(secret_pw),
            "plaintext password leaked: {path}"
        );
        assert!(
            path.contains("t0p%26secret%3Dx"),
            "encoded password missing: {path}"
        );
    }

    #[tokio::test]
    async fn update_users_builds_url_encoded_modifyuser_query() {
        let (driver, client) = recording_driver(|_path| Ok("OK\r\n".to_owned()));
        let update = UserUpdate {
            name: "guard1".to_owned(),
            // A password whose reserved characters must not survive as query syntax.
            password: Some(Secret::new("a&b=c")),
            group: Some("admin".to_owned()),
            authorities: Some(vec!["Monitor".to_owned(), "Playback".to_owned()]),
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());

        let requests = client.requests();
        assert_eq!(requests.len(), 1);
        let path = &requests[0];
        assert!(
            path.starts_with("/cgi-bin/userManager.cgi?action=modifyUser"),
            "path: {path}"
        );
        assert!(path.contains("&user.Name=guard1"), "path: {path}");
        assert!(path.contains("&user.Group=admin"), "path: {path}");
        assert!(
            path.contains("&user.AuthorityList=Monitor,Playback"),
            "path: {path}"
        );
        // The password's `&` and `=` are percent-encoded, never structural.
        assert!(path.contains("&user.Password=a%26b%3Dc"), "path: {path}");
        assert!(!path.contains("a&b=c"), "raw password leaked: {path}");
        assert!(driver.capabilities().user_update);
    }

    #[tokio::test]
    async fn update_users_sends_only_changed_facets() {
        let (driver, client) = recording_driver(|_path| Ok("OK\r\n".to_owned()));
        // A password-only edit: no group, no authorities. An alphanumeric password
        // keeps this test focused on which facets appear (encoding is covered by
        // `update_users_builds_url_encoded_modifyuser_query`).
        let update = UserUpdate {
            name: "guard1".to_owned(),
            password: Some(Secret::new("resetpw")),
            group: None,
            authorities: None,
        };
        driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");

        let path = client.requests().remove(0);
        assert!(path.contains("&user.Name=guard1"), "path: {path}");
        assert!(path.contains("&user.Password=resetpw"), "path: {path}");
        // Facets left untouched are absent from the query entirely.
        assert!(!path.contains("user.Group="), "path: {path}");
        assert!(!path.contains("user.AuthorityList="), "path: {path}");
    }

    #[tokio::test]
    async fn update_users_reports_partial_success() {
        // The first edit is accepted (`OK`); the second is rejected with a device
        // error body, which lands as a per-item failure, not a top-level error.
        let (driver, _client) = recording_driver(|path| {
            if path.contains("user.Name=ok") {
                Ok("OK\r\n".to_owned())
            } else {
                Ok("Error: no such user\r\n".to_owned())
            }
        });
        let updates = [
            UserUpdate {
                name: "ok".to_owned(),
                password: None,
                group: Some("admin".to_owned()),
                authorities: None,
            },
            UserUpdate {
                name: "ghost".to_owned(),
                password: None,
                group: Some("user".to_owned()),
                authorities: None,
            },
        ];
        let outcome = driver.update_users(&updates).await.expect("update_users");
        assert_eq!(outcome.succeeded, ["ok"]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "ghost");
        assert!(matches!(
            &outcome.failed[0].1,
            DriverError::Device(message) if message == "Error: no such user"
        ));
    }
}
