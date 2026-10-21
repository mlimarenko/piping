//! The vendor-neutral [`DeviceDriver`] contract and its normalized data model.
//!
//! A driver translates the small, normalized operations below into one device
//! family's native protocol. Every operation is request/response: there is no
//! session to attach to or reconnect. A concrete device advertises which
//! operations it actually supports via [`Capabilities`]; callers must treat an
//! unsupported operation ([`DriverError::Unsupported`]) as a normal outcome, not
//! a bug.

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which normalized operations a concrete device supports.
///
/// The management plane (`users`/`disk`/`recording`) is only fully available on
/// native drivers; a generic ONVIF device typically advertises `snapshot` and
/// `streaming` but not `disk`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Device identity/firmware readout ([`DeviceDriver::get_info`]).
    pub info: bool,
    /// User account listing and provisioning.
    pub users: bool,
    /// Disk/storage health readout.
    pub disk: bool,
    /// Recording state / retention readout.
    pub recording: bool,
    /// Still-image capture.
    pub snapshot: bool,
    /// Live stream URI resolution.
    pub streaming: bool,
    /// Channel/input enumeration ([`DeviceDriver::list_channels`]).
    pub channels: bool,
    /// Pan/tilt/zoom control ([`DeviceDriver::ptz`]).
    pub ptz: bool,
    /// Recorded-footage export over a time window ([`DeviceDriver::export_clip`]).
    pub export: bool,
    /// Editing an existing user account ([`DeviceDriver::update_users`]).
    pub user_update: bool,
    /// Enumerating the device's own authority/permission-token catalog
    /// ([`DeviceDriver::list_authorities`]), so callers can populate
    /// [`NewUser`]/[`UserUpdate`]`.authorities` without guessing valid tokens.
    pub authority_catalog: bool,
    /// Per-channel, time-windowed recorded-footage search
    /// ([`DeviceDriver::search_recordings`]), distinct from
    /// [`recording`](Self::recording): that read is a coarse, device-wide
    /// window, this one answers "what footage exists on channel N between
    /// these two timestamps" — what an archive timeline or a pre-export
    /// sanity check actually needs.
    pub recording_search: bool,
}

/// A failure from a driver operation.
///
/// Distinguishes device-reported problems from transport problems so callers
/// can decide retry/abort, and separates the *normal* "device can't do this"
/// case ([`DriverError::Unsupported`]) from real errors.
#[derive(Debug, Error)]
pub enum DriverError {
    /// The device/family does not implement this operation.
    #[error("operation not supported by this device")]
    Unsupported,
    /// Authentication was rejected by the device.
    #[error("authentication failed")]
    Auth,
    /// The device operation exceeded its explicit deadline.
    #[error("device operation timed out: {0}")]
    Timeout(String),
    /// The device could not be reached over its configured transport.
    #[error("device is unreachable: {0}")]
    Unreachable(String),
    /// The device clock is too far from ours for authenticated calls to succeed;
    /// synchronize time first.
    #[error("device clock skew; synchronize device time before authenticated calls")]
    ClockSkew,
    /// The underlying byte transport (via the proxy node) failed.
    #[error("transport error: {0}")]
    Transport(String),
    /// The device replied, but the response could not be parsed.
    #[error("malformed response from device: {0}")]
    Protocol(String),
    /// The device replied with an application-level error.
    #[error("device reported an error: {0}")]
    Device(String),
    /// A shared, capacity-bounded resource (a media-plane concurrency slot) is
    /// exhausted. Distinct from [`Transport`](Self::Transport) so the REST layer
    /// can answer `429 Too Many Requests` (retry later) rather than a `502`.
    #[error("all media-plane slots are busy: {0}")]
    Busy(String),
}

/// A plaintext secret (e.g. a device password) that never appears in logs.
///
/// `Debug` is redacted; the value is only reachable through [`Secret::expose`],
/// which makes every read site greppable. It is serializable because it must be
/// placed into native device payloads — never serialize a value containing a
/// `Secret` into a log or audit record.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wrap a plaintext secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Expose the underlying plaintext. Call sites are intentionally explicit.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Device identity and clock, as read from the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Model string, if the device reports one.
    pub model: Option<String>,
    /// Firmware version/build, if reported.
    pub firmware: Option<String>,
    /// Serial number, if reported.
    pub serial: Option<String>,
    /// Number of channels (cameras/inputs), if reported.
    pub channels: Option<u16>,
    /// Device-local time, used to detect clock skew before authenticated calls.
    pub device_time: Option<DateTime<Utc>>,
}

/// An existing user account on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAccount {
    /// Login name.
    pub name: String,
    /// Group/role the account belongs to, if the device exposes one.
    pub group: Option<String>,
    /// Granted authority tokens, verbatim from the device (family-specific).
    pub authorities: Vec<String>,
}

/// A user account to provision on the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewUser {
    /// Login name to create.
    pub name: String,
    /// Plaintext password to set (redacted in logs).
    pub password: Secret,
    /// Group/role to assign, if the family uses groups.
    pub group: Option<String>,
    /// Authority tokens to grant, verbatim (enumerate the device's own catalog
    /// rather than hard-coding tokens).
    pub authorities: Vec<String>,
}

/// An edit to an existing user account on the device.
///
/// `name` identifies the target account (it is never changed by an update). The
/// remaining fields are each optional: `None` leaves that facet untouched, so a
/// caller can reset only the password, only the group, only the authorities, or
/// any combination. An update carrying no change (all three `None`) is
/// meaningless and is rejected at the service boundary before any driver call.
///
/// `Debug` is safe to emit: the `password`'s inner [`Secret`] redacts itself, so
/// an `Option<Secret>` renders as `Some(Secret(***))`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserUpdate {
    /// Login name of the account to edit (identity; never modified).
    pub name: String,
    /// New plaintext password to set, if changing it (redacted in logs).
    pub password: Option<Secret>,
    /// New group/role to assign, if changing it.
    pub group: Option<String>,
    /// Replacement authority tokens to grant, verbatim, if changing them.
    pub authorities: Option<Vec<String>>,
}

impl UserUpdate {
    /// Whether this update carries at least one facet to change. A `false`
    /// result means the update is vacuous (identity-only) and should be
    /// rejected rather than silently succeeding with no device write.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.password.is_some() || self.group.is_some() || self.authorities.is_some()
    }
}

/// Health of one storage device/partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskStatus {
    /// Device/partition identifier as reported.
    pub name: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Used capacity in bytes.
    pub used_bytes: u64,
    /// Normalized health state.
    pub state: DiskState,
    /// Whether the device flagged this disk as errored.
    pub is_error: bool,
}

/// Normalized disk health state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskState {
    /// Healthy and writable.
    Ok,
    /// Present but read-only.
    ReadOnly,
    /// Errored/failed.
    Error,
    /// Device did not report a state we recognize.
    Unknown,
}

/// A contiguous span of recorded footage available on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordSpan {
    /// Start of the span.
    pub start: DateTime<Utc>,
    /// End of the span.
    pub end: DateTime<Utc>,
    /// The channel this span belongs to, when the source of the span is
    /// per-channel. `None` for [`RecordingStatus::spans`] on a family (Dahua)
    /// whose device-wide read fans out across every channel and cannot
    /// attribute a span back to one; always `Some` for
    /// [`DeviceDriver::search_recordings`], which is per-channel by
    /// construction.
    pub channel: Option<u16>,
}

/// Recording state and available footage window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingStatus {
    /// Whether recording is currently active.
    pub active: bool,
    /// Oldest footage timestamp, if any is retained.
    pub oldest: Option<DateTime<Utc>>,
    /// Newest footage timestamp, if any is retained.
    pub newest: Option<DateTime<Utc>>,
    /// Individual recorded spans, when the device enumerates them.
    pub spans: Vec<RecordSpan>,
}

/// A captured still image, either inline or by reference.
#[derive(Clone, Serialize, Deserialize)]
pub enum SnapshotRef {
    /// Inline JPEG bytes (e.g. DVRIP `OPSNAP`, vendor snapshot CGI).
    Bytes(Vec<u8>),
    /// A URI from which the still can be fetched.
    Uri(String),
}

impl std::fmt::Debug for SnapshotRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotRef::Bytes(bytes) => {
                write!(f, "SnapshotRef::Bytes({} bytes)", bytes.len())
            }
            SnapshotRef::Uri(_) => f.write_str("SnapshotRef::Uri(<redacted>)"),
        }
    }
}

/// Transport a resolved stream URI uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamTransport {
    /// RTSP (RTP/RTCP), the near-universal camera egress.
    Rtsp,
}

/// A resolved live-stream URI.
///
/// The URI usually embeds credentials — either as RTSP userinfo (`user:pass@`)
/// or, for the XMeye scheme, as `password=` tokens inside the path — so `Debug`
/// redacts them.
#[derive(Clone, Serialize, Deserialize)]
pub struct StreamUri {
    /// The stream URI, possibly containing embedded credentials.
    pub uri: String,
    /// The transport the URI uses.
    pub transport: StreamTransport,
}

impl std::fmt::Debug for StreamUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamUri")
            .field("uri", &redact_stream_uri(&self.uri))
            .field("transport", &self.transport)
            .finish()
    }
}

/// Credential key names whose values are masked when a stream URI is logged.
///
/// This is defensive log hygiene — never a routing or classification input —
/// so a small, explicit list is appropriate here.
const CREDENTIAL_KEYS: [&str; 7] = [
    "password", "pwd", "pass", "secret", "token", "user", "username",
];

/// Redact credentials from a stream URI for logging: drop `userinfo@`, then mask
/// the value of any credential-bearing `key=value` token (the XMeye scheme
/// carries `password=` inside the path). Scheme, host and non-secret parameters
/// are kept so the log line is still useful.
fn redact_stream_uri(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return "<redacted>".to_owned();
    };
    let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
    format!("{scheme}://{}", mask_credential_tokens(rest))
}

/// Mask credential-token values in `input`, preserving its `& ? ;` structure.
fn mask_credential_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut token_start = 0;
    for (idx, ch) in input.char_indices() {
        if matches!(ch, '&' | '?' | ';') {
            out.push_str(&mask_token(&input[token_start..idx]));
            out.push(ch);
            token_start = idx + ch.len_utf8();
        }
    }
    out.push_str(&mask_token(&input[token_start..]));
    out
}

/// Mask a single `key=value` token if its key names a credential; the key may be
/// prefixed by a path segment (e.g. `10.0.0.9:554/password`), which is preserved.
fn mask_token(token: &str) -> String {
    let Some((key, _value)) = token.split_once('=') else {
        return token.to_owned();
    };
    let key_name = key.rsplit('/').next().unwrap_or(key);
    if CREDENTIAL_KEYS
        .iter()
        .any(|k| key_name.eq_ignore_ascii_case(k))
    {
        let prefix = &key[..key.len() - key_name.len()];
        return format!("{prefix}{key_name}=***");
    }
    token.to_owned()
}

/// Per-channel PTZ support established by read-only capability discovery.
///
/// This is intentionally distinct from [`Capabilities::ptz`], which only says
/// the driver can encode PTZ commands. Control is safe only when this value is
/// [`Supported`](Self::Supported) for the selected channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtzSupport {
    /// A structural device response confirms this channel has PTZ configuration.
    Supported,
    /// A successful structural discovery confirms this channel has no PTZ configuration.
    Unsupported,
    /// Discovery could not prove either supported or unsupported.
    #[default]
    Unknown,
}

/// One camera/input channel on a recorder or multi-sensor device.
///
/// `index` is the device's own 1-based channel number — the value every native
/// per-channel operation (`snapshot`, `ptz`, `export_clip`, RTSP path) keys on.
/// `name`/`enabled`/`recording` are best-effort: a family that cannot cheaply
/// read them leaves them `None`, and the UI degrades to the bare channel number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Device-native 1-based channel number.
    pub index: u16,
    /// Human title the device assigns the channel, if it exposes one.
    pub name: Option<String>,
    /// Whether the channel is enabled/connected, if the device reports it.
    pub enabled: Option<bool>,
    /// Whether read-only discovery confirms PTZ support for this exact channel.
    pub ptz_support: PtzSupport,
    /// Whether the channel is currently recording, if the device reports it.
    pub recording: Option<bool>,
}

/// A normalized, vendor-neutral PTZ (pan/tilt/zoom) control command.
///
/// The three families express PTZ differently (Dahua `ptz.cgi?code=…`,
/// Hikvision ISAPI `PTZData` in `-100..=100`, DVRIP `OPPTZControl`); this is the
/// common denominator each driver maps onto. `Move` velocities are the
/// normalized `-1.0..=1.0` range (0 = no motion on that axis); `Stop` halts a
/// continuous move; `Preset` recalls a stored position by id.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PtzCommand {
    /// Start (or continue) a continuous move at the given normalized velocities.
    Move(PtzVector),
    /// Stop any in-progress continuous move.
    Stop,
    /// Recall a stored preset position by its device id.
    Preset {
        /// Device-native preset id.
        id: u16,
    },
}

/// Normalized PTZ velocities, each on `-1.0..=1.0` (0 = no motion on that axis).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PtzVector {
    /// Horizontal velocity: negative = left, positive = right.
    pub pan: f32,
    /// Vertical velocity: negative = down, positive = up.
    pub tilt: f32,
    /// Zoom velocity: negative = wide, positive = tele.
    pub zoom: f32,
}

impl PtzVector {
    /// Clamp every axis into the valid `-1.0..=1.0` range, mapping NaN to 0.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            pan: clamp_unit(self.pan),
            tilt: clamp_unit(self.tilt),
            zoom: clamp_unit(self.zoom),
        }
    }
}

/// Clamp a value into `-1.0..=1.0`, mapping NaN to 0.
fn clamp_unit(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

/// A request to export recorded footage from one channel over a time window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// Device-native 1-based channel to export.
    pub channel: u16,
    /// Inclusive start of the window.
    pub start: DateTime<Utc>,
    /// Exclusive end of the window.
    pub end: DateTime<Utc>,
}

/// Exported recorded footage, returned as inline bytes with a container hint.
///
/// Unlike [`SnapshotRef`], export never yields a bare URI: a playback URI would
/// embed device credentials (the same reason [`StreamUri`] is redacted), so the
/// bytes are always proxied server-side. `content_type`/`filename` let the REST
/// layer stream the clip as a typed download attachment.
#[derive(Clone, Serialize, Deserialize)]
pub struct ClipRef {
    /// The exported clip bytes (container per `content_type`).
    pub data: Vec<u8>,
    /// MIME type of the clip (e.g. `video/mp4`, `application/octet-stream`).
    pub content_type: String,
    /// Suggested download filename.
    pub filename: String,
}

impl std::fmt::Debug for ClipRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipRef")
            .field("bytes", &self.data.len())
            .field("content_type", &self.content_type)
            .field("filename", &self.filename)
            .finish()
    }
}

/// A raw device-file download stream for recorded footage (e.g. Dahua `.dav`).
///
/// This is the *fast* export path: a family that exposes a native file download
/// (Dahua `loadfile.cgi`) pulls the recorded window over HTTP several times
/// faster than realtime and with ~0 CPU, unlike an RTSP-playback copy that runs
/// at recording speed. The device container is not browser-playable, so the
/// media plane remuxes it to a fragmented MP4 with `-c copy` (no transcode).
/// Nothing is buffered — bytes flow device → ffmpeg stdin → response as they
/// arrive, so an hour-long clip stays O(chunk) in memory.
pub struct DownloadStream {
    /// The device container bytes, streamed (never buffered whole). Credential
    /// material stays in the driver's client and never rides this stream.
    pub bytes: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
    /// Suggested base filename (no extension); the media plane appends `.mp4`.
    pub filename: String,
    /// Optional remux hints for a stream the container prober cannot fully
    /// describe. `None` (the default for a real container like Dahua's `.dav`)
    /// leaves every decision to the prober.
    pub hints: RemuxHints,
}

/// Facts about a [`DownloadStream`] that the container prober cannot recover.
///
/// Deliberately a small typed struct rather than raw encoder arguments: a
/// passthrough of ffmpeg flags would let a driver reshape the media plane's
/// command line, which is neither reviewable nor testable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemuxHints {
    /// Source frame rate, when the byte stream itself does not carry one.
    ///
    /// A bare elementary stream (as DVRIP archive playback delivers, unlike a
    /// real container) has no timing, so a remux would silently assume 25 fps.
    /// Recorders in this family commonly run at 12, which would play back at
    /// roughly double speed.
    pub fps: Option<u8>,
}

/// How the media plane should package a recorded stream.
///
/// The cheap default copies the stream untouched. Transcoding exists only
/// because these recorders commonly write H.265, which most browsers cannot
/// decode — so a faithful copy is unplayable for the operator who asked to
/// watch. It costs real CPU, so it is never chosen implicitly: the caller asks
/// for it, and the concurrency ceiling bounds how many can run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClipEncoding {
    /// Repackage only (`-c copy`): exact, ~0 CPU, keeps the source codec.
    #[default]
    Copy,
    /// Re-encode the video to H.264 baseline so any browser can play it.
    BrowserCompatible,
}

impl std::fmt::Debug for DownloadStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadStream")
            .field("filename", &self.filename)
            .finish_non_exhaustive()
    }
}

/// Outcome of a bulk operation, preserving partial success.
///
/// Native user provisioning is often a per-item loop that can partially fail;
/// callers must be able to see exactly which items succeeded and why the rest
/// did not. A fatal link error interrupts the loop: the attempted item's
/// ambiguous failure and a static failure for each unattempted item are
/// recorded, and [`LinkFailurePolicy`] decides whether a batch with no
/// acknowledgement is that per-item outcome or a top-level `Err`. Audit
/// metadata must use counts, never these usernames or device error messages.
#[derive(Debug, Default)]
pub struct BulkOutcome {
    /// Usernames the operation succeeded for.
    pub succeeded: Vec<String>,
    /// Usernames that failed, paired with the reason.
    pub failed: Vec<(String, DriverError)>,
}

/// What a bulk loop does when a fatal link error interrupts it.
///
/// The native drivers disagree on whether a batch that acknowledged nothing is
/// a per-item result or a top-level error, so the distinction is explicit at
/// the call site rather than repeated in each driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFailurePolicy {
    /// Always return the per-item [`BulkOutcome`], even with nothing
    /// acknowledged. A single-session batch that logged in and then broke must
    /// still report the attempted item's failure and the unattempted tail, so
    /// the caller can audit each account and answer per-item.
    PreservePartialOutcome,
    /// Return `Err` when no write was acknowledged, and the partial
    /// [`BulkOutcome`] otherwise. A stateless batch whose link failed before
    /// any confirmation is a device error, not a successful empty response.
    ErrorWithoutAcknowledged,
}

impl BulkOutcome {
    /// Stop after a fatal link error without losing confirmed writes.
    ///
    /// The failed request might have reached the device; only an acknowledged
    /// response is added to `succeeded`. The attempted item carries `error`,
    /// and every remaining item is recorded as not attempted. `policy` decides
    /// the no-acknowledgement case; see [`LinkFailurePolicy`].
    ///
    /// # Errors
    ///
    /// Returns `error` instead of an outcome only under
    /// [`LinkFailurePolicy::ErrorWithoutAcknowledged`] when nothing was
    /// acknowledged.
    pub fn abort_after_link_failure(
        mut self,
        name: String,
        error: DriverError,
        remaining: impl IntoIterator<Item = String>,
        policy: LinkFailurePolicy,
    ) -> Result<Self, DriverError> {
        if policy == LinkFailurePolicy::ErrorWithoutAcknowledged && self.succeeded.is_empty() {
            return Err(error);
        }
        self.failed.push((name, error));
        self.failed.extend(remaining.into_iter().map(|name| {
            (
                name,
                DriverError::Unreachable("not attempted after device link failure".to_owned()),
            )
        }));
        Ok(self)
    }

    /// Whether every item succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.failed.is_empty()
    }
}

/// The normalized contract every device family driver implements.
///
/// Implementations are object-safe (`Box<dyn DeviceDriver>`) so heterogeneous
/// families can share one registry.
///
/// # Errors
///
/// Every method returns [`DriverError`]. [`DriverError::Unsupported`] is a
/// normal result for capabilities the device lacks; check [`Capabilities`]
/// first to avoid calling unsupported operations.
#[async_trait]
pub trait DeviceDriver: Send + Sync {
    /// The operations this device supports.
    fn capabilities(&self) -> Capabilities;

    /// Read device identity, firmware, and clock.
    ///
    /// Should be safe to call first: native drivers rely on the device clock to
    /// build authenticated requests without skew.
    async fn get_info(&self) -> Result<DeviceInfo, DriverError>;

    /// List existing user accounts.
    async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError>;

    /// Enumerate the device's own authority/permission-token catalog.
    ///
    /// The returned strings are the exact tokens the device accepts in
    /// [`NewUser::authorities`] / [`UserUpdate::authorities`] — callers should
    /// build a picker from this list rather than hard-coding or guessing
    /// tokens. The default returns [`DriverError::Unsupported`] so a family
    /// without a discoverable catalog keeps compiling; callers check
    /// [`Capabilities::authority_catalog`] first.
    async fn list_authorities(&self) -> Result<Vec<String>, DriverError> {
        Err(DriverError::Unsupported)
    }

    /// Provision users, returning a per-item [`BulkOutcome`].
    async fn create_users(&self, users: &[NewUser]) -> Result<BulkOutcome, DriverError>;

    /// Remove users by name, returning a per-item [`BulkOutcome`].
    async fn delete_users(&self, usernames: &[String]) -> Result<BulkOutcome, DriverError>;

    /// Edit existing users (password/group/authorities), returning a per-item
    /// [`BulkOutcome`].
    ///
    /// Each [`UserUpdate`] identifies an account by `name` and carries only the
    /// facets to change (a `None` field is left untouched). Like the other
    /// user-write methods this is a per-item loop that preserves partial success;
    /// callers check [`Capabilities::user_update`] first. The default returns
    /// [`DriverError::Unsupported`] so a family that cannot edit a user (e.g.
    /// generic ONVIF) keeps compiling without pretending to support it.
    async fn update_users(&self, updates: &[UserUpdate]) -> Result<BulkOutcome, DriverError> {
        let _ = updates;
        Err(DriverError::Unsupported)
    }

    /// Read disk/storage health.
    async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError>;

    /// Read recording state and available footage window.
    async fn recording_status(&self) -> Result<RecordingStatus, DriverError>;

    /// Search recorded footage for one channel over a time window.
    ///
    /// Unlike [`recording_status`](Self::recording_status) — a coarse,
    /// device-wide readout — this answers a specific question: what footage
    /// exists on THIS channel between THESE two timestamps. That is what an
    /// archive timeline needs to render honest per-camera gaps, and what an
    /// export/playback request can consult first to avoid paying a device
    /// session for a window with nothing in it.
    ///
    /// `request.channel`/`start`/`end` are read as the search window; no bytes
    /// are moved. The default returns [`DriverError::Unsupported`] so a family
    /// without a per-channel search primitive keeps compiling; callers check
    /// [`Capabilities::recording_search`] first.
    async fn search_recordings(
        &self,
        request: &ExportRequest,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        let _ = request;
        Err(DriverError::Unsupported)
    }

    /// Capture a still image from a channel (or the device's default channel).
    async fn snapshot(&self, channel: Option<u16>) -> Result<SnapshotRef, DriverError>;

    /// Resolve a live-stream URI for a channel (or the default channel).
    async fn stream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError>;

    /// Resolve a LIVE-stream URI for a channel's LOWER-RESOLUTION SUBSTREAM, if
    /// the family/device exposes one.
    ///
    /// This is the preferred source for the live (and, where offered, the
    /// still-fallback) plane: decode DPB scales with source pixel count, so a
    /// substream is materially cheaper to decode than
    /// [`stream_uri`](Self::stream_uri) (always the main/full-resolution
    /// stream). Unlike `stream_uri`, this URI is SPECULATIVE — the device may
    /// have no substream configured at all — so a failure to actually play it
    /// means "no substream available", never a hard error; callers MUST fall
    /// back to [`stream_uri`](Self::stream_uri) on playback failure. The
    /// default simply delegates to [`stream_uri`](Self::stream_uri), so a
    /// family without a distinct lower-resolution source keeps compiling and
    /// always resolves to the main stream.
    async fn substream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        self.stream_uri(channel).await
    }

    /// Resolve an RTSP PLAYBACK URI for a channel over a recorded time window.
    ///
    /// Unlike [`stream_uri`](Self::stream_uri) (live), this addresses the
    /// device's archive: the RTSP session replays footage between the request's
    /// `start`/`end`. The credential-bearing URI never leaves the backend — it
    /// is handed straight to the media plane, which decodes/copies it into a
    /// clip. The default returns [`DriverError::Unsupported`]; families whose
    /// playback is not a standard RTSP URL (e.g. DVRIP `OPPlayBack`) keep it.
    async fn playback_uri(&self, request: &ExportRequest) -> Result<StreamUri, DriverError> {
        let _ = request;
        Err(DriverError::Unsupported)
    }

    /// Stream recorded footage as a native device file download, if the family
    /// has one (the *fast* export path, see [`DownloadStream`]).
    ///
    /// Returns `Ok(None)` when the device has no fast file-download API; the
    /// service then falls back to [`playback_uri`](Self::playback_uri) + an
    /// RTSP-playback copy (realtime, slower, but universal). `Ok(Some(_))` yields
    /// the raw device container (e.g. Dahua `.dav`), which the media plane
    /// remuxes to MP4 with `-c copy`. The default is `Ok(None)` so every family
    /// keeps compiling; only families with a verified native download override it.
    async fn export_download(
        &self,
        request: &ExportRequest,
    ) -> Result<Option<DownloadStream>, DriverError> {
        let _ = request;
        Ok(None)
    }

    /// Enumerate the device's camera/input channels.
    ///
    /// The default returns [`DriverError::Unsupported`] so a family that has not
    /// implemented channel discovery keeps compiling; callers should check
    /// [`Capabilities::channels`] first and fall back to
    /// [`DeviceInfo::channels`] (a count) or a manual channel number.
    async fn list_channels(&self) -> Result<Vec<Channel>, DriverError> {
        Err(DriverError::Unsupported)
    }

    /// Issue a pan/tilt/zoom control command against a channel.
    ///
    /// A device-mutating control action: callers gate it exactly like user
    /// provisioning (the device-write lab flag) and check [`Capabilities::ptz`]
    /// first. The default returns [`DriverError::Unsupported`].
    async fn ptz(&self, channel: u16, command: PtzCommand) -> Result<(), DriverError> {
        let _ = (channel, command);
        Err(DriverError::Unsupported)
    }

    /// Export recorded footage from a channel over a time window.
    ///
    /// Returns the clip as inline bytes ([`ClipRef`]) — never a credential-bearing
    /// URI. A pure READ of stored footage. The default returns
    /// [`DriverError::Unsupported`]; callers check [`Capabilities::export`] first.
    async fn export_clip(&self, request: &ExportRequest) -> Result<ClipRef, DriverError> {
        let _ = request;
        Err(DriverError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;

    #[test]
    fn secret_debug_is_redacted_but_value_is_reachable() {
        let secret = Secret::new("hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn new_user_debug_does_not_leak_password() {
        let user = NewUser {
            name: "guard1".to_owned(),
            password: Secret::new("s3cr3t-pw"),
            group: Some("user".to_owned()),
            authorities: vec!["Monitor".to_owned()],
        };
        let rendered = format!("{user:?}");
        assert!(
            !rendered.contains("s3cr3t-pw"),
            "password leaked: {rendered}"
        );
        assert!(rendered.contains("Secret(***)"));
    }

    #[test]
    fn user_update_debug_does_not_leak_password() {
        let update = UserUpdate {
            name: "guard1".to_owned(),
            password: Some(Secret::new("new-s3cr3t")),
            group: Some("user".to_owned()),
            authorities: Some(vec!["Monitor".to_owned()]),
        };
        let rendered = format!("{update:?}");
        assert!(
            !rendered.contains("new-s3cr3t"),
            "password leaked: {rendered}"
        );
        assert!(rendered.contains("Secret(***)"));
    }

    #[test]
    fn user_update_has_changes_detects_a_vacuous_update() {
        let vacuous = UserUpdate {
            name: "guard1".to_owned(),
            password: None,
            group: None,
            authorities: None,
        };
        assert!(!vacuous.has_changes());
        // Any single facet makes it a real change.
        assert!(
            UserUpdate {
                password: Some(Secret::new("pw")),
                ..vacuous.clone()
            }
            .has_changes()
        );
        assert!(
            UserUpdate {
                group: Some("admin".to_owned()),
                ..vacuous.clone()
            }
            .has_changes()
        );
        assert!(
            UserUpdate {
                authorities: Some(vec!["Monitor".to_owned()]),
                ..vacuous
            }
            .has_changes()
        );
    }

    #[test]
    fn stream_uri_debug_redacts_embedded_credentials() {
        let rtsp = StreamUri {
            uri: "rtsp://admin:p%40ss@10.0.0.5:554/Streaming/Channels/101".to_owned(),
            transport: StreamTransport::Rtsp,
        };
        let rendered = format!("{rtsp:?}");
        assert!(
            !rendered.contains("p%40ss"),
            "credential leaked: {rendered}"
        );
        assert!(rendered.contains("rtsp://10.0.0.5:554/Streaming/Channels/101"));

        let xmeye = StreamUri {
            uri: "rtsp://10.0.0.9:554/user=admin&password=topsecret&channel=1&stream=0.sdp?"
                .to_owned(),
            transport: StreamTransport::Rtsp,
        };
        assert!(!format!("{xmeye:?}").contains("topsecret"));
    }

    #[test]
    fn snapshot_debug_reports_size_not_bytes() {
        let snap = SnapshotRef::Bytes(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        assert_eq!(format!("{snap:?}"), "SnapshotRef::Bytes(4 bytes)");
    }

    #[test]
    fn bulk_outcome_tracks_partial_success() {
        let outcome = BulkOutcome {
            succeeded: vec!["a".to_owned()],
            failed: vec![("b".to_owned(), DriverError::Unsupported)],
        };
        assert!(!outcome.all_succeeded());
        assert_eq!(outcome.succeeded, ["a"]);
    }

    #[test]
    fn ptz_vector_clamps_out_of_range_axes_and_maps_nan_to_zero() {
        let clamped = PtzVector {
            pan: 2.5,
            tilt: -9.0,
            zoom: f32::NAN,
        }
        .clamped();
        assert_eq!(clamped.pan, 1.0);
        assert_eq!(clamped.tilt, -1.0);
        assert_eq!(clamped.zoom, 0.0);
    }

    #[test]
    fn ptz_vector_leaves_in_range_axes_untouched() {
        let vector = PtzVector {
            pan: 0.5,
            tilt: -0.25,
            zoom: 0.0,
        };
        assert_eq!(vector.clamped(), vector);
    }

    #[test]
    fn clip_ref_debug_reports_size_not_bytes() {
        let clip = ClipRef {
            data: vec![0u8; 4096],
            content_type: "video/mp4".to_owned(),
            filename: "clip.mp4".to_owned(),
        };
        let rendered = format!("{clip:?}");
        assert!(rendered.contains("bytes: 4096"), "{rendered}");
        assert!(rendered.contains("video/mp4"), "{rendered}");
        // The clip payload itself is never spilled into Debug.
        assert!(!rendered.contains("[0, 0, 0"), "{rendered}");
    }

    /// A minimal [`DeviceDriver`] fake that implements only the
    /// non-defaulted methods, so the trait's default method bodies (under
    /// test here) are exercised unmodified.
    struct FakeDriver {
        stream: StreamUri,
    }

    #[async_trait]
    impl DeviceDriver for FakeDriver {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn create_users(&self, _users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn delete_users(&self, _usernames: &[String]) -> Result<BulkOutcome, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn snapshot(&self, _channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
            Err(DriverError::Unsupported)
        }

        async fn stream_uri(&self, _channel: Option<u16>) -> Result<StreamUri, DriverError> {
            Ok(self.stream.clone())
        }
    }

    #[tokio::test]
    async fn substream_uri_default_falls_back_to_stream_uri() {
        let driver = FakeDriver {
            stream: StreamUri {
                uri: "rtsp://10.0.0.5:554/Streaming/Channels/101".to_owned(),
                transport: StreamTransport::Rtsp,
            },
        };
        let substream = driver
            .substream_uri(Some(1))
            .await
            .expect("default substream_uri delegates to stream_uri");
        assert_eq!(substream.uri, driver.stream.uri);
        assert_eq!(substream.transport, driver.stream.transport);
    }

    #[tokio::test]
    async fn list_authorities_default_is_unsupported() {
        let driver = FakeDriver {
            stream: StreamUri {
                uri: "rtsp://10.0.0.5:554/Streaming/Channels/101".to_owned(),
                transport: StreamTransport::Rtsp,
            },
        };
        let err = driver
            .list_authorities()
            .await
            .expect_err("default list_authorities is unsupported");
        assert!(matches!(err, DriverError::Unsupported));
    }

    #[test]
    fn capabilities_default_leaves_authority_catalog_false() {
        assert!(!Capabilities::default().authority_catalog);
    }

    #[test]
    fn ptz_command_serializes_with_a_kind_tag() {
        let moved = serde_json::to_value(PtzCommand::Move(PtzVector {
            pan: 1.0,
            tilt: 0.0,
            zoom: -1.0,
        }))
        .expect("serialize");
        assert_eq!(moved["kind"], "move");
        assert_eq!(moved["pan"], 1.0);

        let preset = serde_json::to_value(PtzCommand::Preset { id: 3 }).expect("serialize");
        assert_eq!(preset["kind"], "preset");
        assert_eq!(preset["id"], 3);

        let stop = serde_json::to_value(PtzCommand::Stop).expect("serialize");
        assert_eq!(stop["kind"], "stop");
    }
}
