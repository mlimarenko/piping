//! REST surface for the CCTV device-management plane (`/api/v1/cctv/...`).
//!
//! This is the CCTV (video-surveillance) connection type's own plane — keyed by
//! connection-target id exactly like SSH/RDP/VNC, and separate from the NetBox
//! inventory `devices` CRUD (`/api/v1/inventory/devices`). The typed connection
//! profile it resolves through carries the `cctv` protocol slug (a DB CHECK-backed
//! vocabulary, consistent with the `cctv` RBAC resource and this REST namespace).
//! It resolves a target to a concrete
//! recorder/camera and runs one normalized driver operation: static
//! `capabilities` (the control shape the UI renders from), identity/user/disk/
//! recording/channel reads, per-channel snapshot, recorded-footage export, and
//! device controls. Every operation is independently gated on the dedicated
//! per-target device vocabulary: `cctv:view`, `cctv:recordings`, `cctv:export`,
//! `device_users:connection_target`, `cctv:settings`, and `cctv:ptz`. Broad
//! connection-target read access never implies CCTV access.
//!
//! Secret values are never returned: responses mirror the vendor-neutral domain
//! DTOs, which carry only non-secret readouts. Timestamps are emitted as RFC3339
//! strings; snapshot and export answer with binary bodies (`image/jpeg`,
//! `video/*`) — never a credential-bearing URI.

use axum::{
    Extension, Json,
    body::Body,
    extract::State,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::access_control_helpers::{
    build_device_management_service, build_policy_evaluation_service,
};
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::error_mapping::build_error_response;
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::require_bulk_items;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::{application::AuditRecorder, domain::AuditEvent};
use crate::contexts::device_management::application::{
    ClipStream, DeviceManagementService, DeviceResolveError, DeviceServiceError, LiveStreamOptions,
    PreviewAction, PreviewItem, WriteOp, WritePreview,
};
use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, Channel, ClipEncoding, DeviceInfo, DiskState, DiskStatus,
    DriverError, ExportRequest, NewUser, PtzCommand, PtzSupport, PtzVector, RecordSpan,
    RecordingStatus, Secret, SnapshotRef, UserAccount, UserUpdate,
};
use crate::contexts::shared::domain::reason_codes::{self, ConnectionReasonCode};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

const CONNECTION_FAILURE_MESSAGE: &str = "Unable to connect to the device";

/// Typed, credential-free details accepted by the CCTV audit emitter.
///
/// Keeping request DTOs and domain write commands out of this type prevents
/// device passwords or login names from being forwarded through an unstructured
/// metadata value. Account-write events carry counts only.
///
pub(crate) enum CctvAuditDetails<'a> {
    Ptz {
        channel: u16,
        action: &'static str,
    },
    Export {
        channel: u16,
        start: &'a str,
        end: &'a str,
    },
    DeviceUsers {
        outcome: &'a DeviceUsersBulkOutcomeResponse,
    },
    FleetUsers {
        succeeded_count: usize,
        failed_count: usize,
    },
}

fn cctv_audit_metadata(
    request_context: &RequestContext,
    target_id: Uuid,
    operation: &'static str,
    details: CctvAuditDetails<'_>,
) -> serde_json::Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("operation".to_string(), json!(operation));
    metadata.insert("device_id".to_string(), json!(target_id));
    metadata.insert(
        "auth_method".to_string(),
        json!(request_context.auth_method),
    );
    metadata.insert(
        "correlation_id".to_string(),
        json!(request_context.correlation_id),
    );

    match details {
        CctvAuditDetails::Ptz { channel, action } => {
            metadata.insert("result".to_string(), json!("success"));
            metadata.insert("channel".to_string(), json!(channel));
            metadata.insert("ptz_action".to_string(), json!(action));
        }
        CctvAuditDetails::Export {
            channel,
            start,
            end,
        } => {
            metadata.insert("result".to_string(), json!("success"));
            metadata.insert("channel".to_string(), json!(channel));
            metadata.insert("start".to_string(), json!(start));
            metadata.insert("end".to_string(), json!(end));
        }
        CctvAuditDetails::DeviceUsers { outcome } => {
            let result = if outcome.all_succeeded {
                "success"
            } else if outcome.succeeded.is_empty() {
                "failed"
            } else {
                "partial_success"
            };
            metadata.insert("result".to_string(), json!(result));
            metadata.insert(
                "succeeded_count".to_string(),
                json!(outcome.succeeded.len()),
            );
            metadata.insert("failed_count".to_string(), json!(outcome.failed.len()));
        }
        CctvAuditDetails::FleetUsers {
            succeeded_count,
            failed_count,
        } => {
            metadata.insert(
                "result".to_string(),
                json!(if failed_count == 0 {
                    "success"
                } else {
                    "partial_success"
                }),
            );
            metadata.insert("succeeded_count".to_string(), json!(succeeded_count));
            metadata.insert("failed_count".to_string(), json!(failed_count));
        }
    }

    serde_json::Value::Object(metadata)
}

/// Record a CCTV action that has already succeeded on the customer device.
///
/// This deliberately returns unit. Once the device mutation or export has
/// succeeded, an audit-store failure must not turn the response into a 5xx and
/// invite a retry of an action that already happened. The failure is logged
/// loudly for reconciliation instead.
///
pub(crate) async fn record_cctv_operation<R>(
    recorder: &R,
    request_context: &RequestContext,
    event_type: &'static str,
    operation: &'static str,
    target_id: Uuid,
    details: CctvAuditDetails<'_>,
) where
    R: AuditRecorder + ?Sized,
{
    let outcome = match request_context.require_authenticated_user_id() {
        Ok(actor_id) => {
            recorder
                .record_event(AuditEvent {
                    id: Uuid::now_v7(),
                    event_type: event_type.to_string(),
                    actor_id: Some(actor_id),
                    actor_kind: request_context.audit_actor_kind().to_string(),
                    target_type: Some("cctv_device".to_string()),
                    target_id: Some(target_id),
                    metadata: cctv_audit_metadata(request_context, target_id, operation, details),
                    created_at: chrono::Utc::now(),
                })
                .await
        }
        Err(error) => Err(error),
    };

    if let Err(error) = outcome {
        log_error_event(
            "cctv.audit.record_failed",
            "cctv_audit_record_failed",
            error.to_string(),
            EventFields::default()
                .with_operation("cctv_audit_record")
                .with_status("failed")
                .with_resource("cctv_device", target_id.to_string())
                .with_context("event_type", event_type.to_string())
                .with_context("cctv_operation", operation.to_string()),
        );
    }
}

/// The caller's independent `cctv:*` verdicts for this target.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
pub struct DeviceCctvPermissionsResponse {
    pub view: bool,
    pub recordings: bool,
    pub export: bool,
    pub users: bool,
    pub settings: bool,
    pub ptz: bool,
}

impl DeviceCctvPermissionsResponse {
    fn has_any(self) -> bool {
        self.view || self.recordings || self.export || self.users || self.settings || self.ptz
    }
}

/// The device's static capability set plus the caller's write permissions.
///
/// This is the control shape the UI renders from: which panels/actions a family
/// supports (a static, per-driver constant), and whether the caller is permitted
/// to use the mutating ones (PTZ, user provisioning).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceCapabilitiesResponse {
    /// Device identity/firmware readout is available.
    pub info: bool,
    /// User account listing/provisioning is available.
    pub users: bool,
    /// Disk/storage health readout is available.
    pub disk: bool,
    /// Recording state/retention readout is available.
    pub recording: bool,
    /// Still-image capture is available.
    pub snapshot: bool,
    /// Live stream URI resolution is available.
    pub streaming: bool,
    /// Channel/input enumeration is available.
    pub channels: bool,
    /// Pan/tilt/zoom control is available.
    pub ptz: bool,
    /// Recorded-footage export is available.
    pub export: bool,
    /// Editing an existing user account is available.
    pub user_update: bool,
    /// Enumerating the device's own authority/permission-token catalog is
    /// available, so the UI can offer a picker instead of a free-text field.
    pub authority_catalog: bool,
    /// Per-channel, time-windowed recorded-footage search is available (an
    /// archive timeline), distinct from the coarse device-wide `recording`
    /// readout above.
    pub recording_search: bool,
    /// The caller's per-target RBAC verdicts for the mutating controls.
    pub permissions: DeviceCctvPermissionsResponse,
}

impl DeviceCapabilitiesResponse {
    fn new(capabilities: Capabilities, permissions: DeviceCctvPermissionsResponse) -> Self {
        let Capabilities {
            info,
            users,
            disk,
            recording,
            snapshot,
            streaming,
            channels,
            ptz,
            export,
            user_update,
            authority_catalog,
            recording_search,
        } = capabilities;
        Self {
            info,
            users,
            disk,
            recording,
            snapshot,
            streaming,
            channels,
            ptz,
            export,
            user_update,
            authority_catalog,
            recording_search,
            permissions,
        }
    }
}

/// Per-channel PTZ support established by read-only capability discovery.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DevicePtzSupportResponse {
    /// A structural device response confirms PTZ support.
    Supported,
    /// A successful structural discovery confirms PTZ is absent.
    Unsupported,
    /// Discovery could not prove either outcome.
    Unknown,
}

impl From<PtzSupport> for DevicePtzSupportResponse {
    fn from(value: PtzSupport) -> Self {
        match value {
            PtzSupport::Supported => Self::Supported,
            PtzSupport::Unsupported => Self::Unsupported,
            PtzSupport::Unknown => Self::Unknown,
        }
    }
}

/// One camera/input channel on the device.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceChannelResponse {
    /// Device-native 1-based channel number.
    pub index: u16,
    /// Human title the device assigns the channel, if any.
    pub name: Option<String>,
    /// Whether the channel is enabled/connected, if the device reports it.
    pub enabled: Option<bool>,
    /// Whether read-only discovery confirms PTZ support for this exact channel.
    pub ptz_support: DevicePtzSupportResponse,
    /// Whether the channel is currently recording, if the device reports it.
    pub recording: Option<bool>,
}

impl From<Channel> for DeviceChannelResponse {
    fn from(value: Channel) -> Self {
        Self {
            index: value.index,
            name: value.name,
            enabled: value.enabled,
            ptz_support: value.ptz_support.into(),
            recording: value.recording,
        }
    }
}

/// The device's camera/input channels.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceChannelsResponse {
    /// Channels, in the device's own order.
    pub channels: Vec<DeviceChannelResponse>,
}

/// A pan/tilt/zoom control command to issue against a channel.
///
/// `kind` selects the action: `move` uses the `pan`/`tilt`/`zoom` velocities
/// (each `-1.0..=1.0`, 0 = no motion), `stop` halts a continuous move, and
/// `preset` recalls the stored `preset` id.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PtzCommandRequest {
    /// Which action to perform.
    pub kind: PtzKind,
    /// 1-based channel to control (defaults to channel 1).
    #[serde(default = "default_channel")]
    pub channel: u16,
    /// Horizontal velocity for `move` (negative = left).
    #[serde(default)]
    pub pan: f32,
    /// Vertical velocity for `move` (negative = down).
    #[serde(default)]
    pub tilt: f32,
    /// Zoom velocity for `move` (negative = wide).
    #[serde(default)]
    pub zoom: f32,
    /// Preset id for `preset`.
    #[serde(default)]
    pub preset: u16,
}

/// The PTZ action a command request selects.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PtzKind {
    /// Start/continue a continuous move at the given velocities.
    Move,
    /// Stop an in-progress continuous move.
    Stop,
    /// Recall a stored preset by id.
    Preset,
}

/// Default channel when a request omits one.
fn default_channel() -> u16 {
    1
}

impl PtzCommandRequest {
    /// Convert the flat request into the typed domain command (clamping the
    /// move velocities into their valid range).
    fn to_command(&self) -> PtzCommand {
        match self.kind {
            PtzKind::Move => PtzCommand::Move(
                PtzVector {
                    pan: self.pan,
                    tilt: self.tilt,
                    zoom: self.zoom,
                }
                .clamped(),
            ),
            PtzKind::Stop => PtzCommand::Stop,
            PtzKind::Preset => PtzCommand::Preset { id: self.preset },
        }
    }

    fn audit_action(&self) -> &'static str {
        match self.kind {
            PtzKind::Move => "move",
            PtzKind::Stop => "stop",
            PtzKind::Preset => "preset",
        }
    }
}

/// Time window + channel selector for a recorded-footage export.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeviceExportQuery {
    /// 1-based channel to export (defaults to channel 1).
    #[serde(default = "default_channel")]
    pub channel: u16,
    /// Inclusive window start (RFC3339).
    pub start: String,
    /// Exclusive window end (RFC3339).
    pub end: String,
    /// Re-encode the video so any browser can play it.
    ///
    /// Only meaningful for playback. These recorders commonly write H.265,
    /// which most browsers refuse to decode — a faithful copy is then
    /// unwatchable. Transcoding costs real CPU, so it is never implicit: the
    /// caller asks for it after the browser has actually failed to play the
    /// stream. Export never transcodes; a downloaded clip keeps the device's
    /// own encoding.
    pub transcode: Option<bool>,
}

/// Device identity and clock, as read from the device.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceInfoResponse {
    /// Model string, if the device reports one.
    pub model: Option<String>,
    /// Firmware version/build, if reported.
    pub firmware: Option<String>,
    /// Serial number, if reported.
    pub serial: Option<String>,
    /// Number of channels (cameras/inputs), if reported.
    pub channels: Option<u16>,
    /// Device-local time as an RFC3339 string, if reported.
    pub device_time: Option<String>,
}

impl From<DeviceInfo> for DeviceInfoResponse {
    fn from(info: DeviceInfo) -> Self {
        Self {
            model: info.model,
            firmware: info.firmware,
            serial: info.serial,
            channels: info.channels,
            device_time: info.device_time.map(|time| time.to_rfc3339()),
        }
    }
}

/// One user account present on the device.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceUserResponse {
    /// Login name.
    pub name: String,
    /// Group/role the account belongs to, if the device exposes one.
    pub group: Option<String>,
    /// Granted authority tokens, verbatim from the device (family-specific).
    pub authorities: Vec<String>,
}

impl From<UserAccount> for DeviceUserResponse {
    fn from(user: UserAccount) -> Self {
        Self {
            name: user.name,
            group: user.group,
            authorities: user.authorities,
        }
    }
}

/// The device's user accounts.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceUsersResponse {
    /// Existing user accounts.
    pub users: Vec<DeviceUserResponse>,
}

/// Authority/permission tokens accepted by the device for user accounts.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceAuthoritiesResponse {
    /// Verbatim tokens from the device's structural authority catalog.
    pub authorities: Vec<String>,
}

/// Normalized disk health state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceDiskStateResponse {
    /// Healthy and writable.
    Ok,
    /// Present but read-only.
    ReadOnly,
    /// Errored/failed.
    Error,
    /// Device did not report a state we recognize.
    Unknown,
}

impl From<DiskState> for DeviceDiskStateResponse {
    fn from(state: DiskState) -> Self {
        match state {
            DiskState::Ok => Self::Ok,
            DiskState::ReadOnly => Self::ReadOnly,
            DiskState::Error => Self::Error,
            DiskState::Unknown => Self::Unknown,
        }
    }
}

/// Health of one storage device/partition.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceDiskEntryResponse {
    /// Device/partition identifier as reported.
    pub name: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Used capacity in bytes.
    pub used_bytes: u64,
    /// Normalized health state.
    pub state: DeviceDiskStateResponse,
    /// Whether the device flagged this disk as errored.
    pub is_error: bool,
}

impl From<DiskStatus> for DeviceDiskEntryResponse {
    fn from(disk: DiskStatus) -> Self {
        Self {
            name: disk.name,
            total_bytes: disk.total_bytes,
            used_bytes: disk.used_bytes,
            state: disk.state.into(),
            is_error: disk.is_error,
        }
    }
}

/// The device's disk/storage health.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceDiskResponse {
    /// Per-disk health entries.
    pub disks: Vec<DeviceDiskEntryResponse>,
}

/// A contiguous span of recorded footage available on the device.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceRecordSpanResponse {
    /// Start of the span as an RFC3339 string.
    pub start: String,
    /// End of the span as an RFC3339 string.
    pub end: String,
    /// The channel this span belongs to, when the source is per-channel.
    /// `None` on the device-wide `/recording` read for a family whose search
    /// fans out across every channel; always present from `/recordings`.
    pub channel: Option<u16>,
}

impl From<RecordSpan> for DeviceRecordSpanResponse {
    fn from(span: RecordSpan) -> Self {
        Self {
            start: span.start.to_rfc3339(),
            end: span.end.to_rfc3339(),
            channel: span.channel,
        }
    }
}

/// Recording state and available footage window.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceRecordingResponse {
    /// Whether recording is currently active.
    pub active: bool,
    /// Oldest retained footage timestamp (RFC3339), if any.
    pub oldest: Option<String>,
    /// Newest retained footage timestamp (RFC3339), if any.
    pub newest: Option<String>,
    /// Individual recorded spans, when the device enumerates them.
    pub spans: Vec<DeviceRecordSpanResponse>,
}

impl From<RecordingStatus> for DeviceRecordingResponse {
    fn from(status: RecordingStatus) -> Self {
        Self {
            active: status.active,
            oldest: status.oldest.map(|time| time.to_rfc3339()),
            newest: status.newest.map(|time| time.to_rfc3339()),
            spans: status
                .spans
                .into_iter()
                .map(DeviceRecordSpanResponse::from)
                .collect(),
        }
    }
}

/// Which user-management mutation a preview request describes.
///
/// The discriminant selects which of the request's `users` / `usernames`
/// collections is used to build the write operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceUserWriteOpKind {
    /// Provision the accounts in `users`.
    Create,
    /// Remove the accounts named in `usernames`.
    Delete,
}

/// A user account to provision on the device.
///
/// `password` is **write-only**: it is accepted on the request but never echoed
/// in any response. It is optional so the same shape can drive a dry-run
/// preview, which never uses the password — a create maps a missing password
/// to an empty string and lets the device decide whether to accept it.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct DeviceNewUserRequest {
    /// Login name to create.
    pub name: String,
    /// Plaintext password to set (write-only; never returned).
    #[serde(default)]
    pub password: Option<String>,
    /// Group/role to assign, if the family uses groups.
    #[serde(default)]
    pub group: Option<String>,
    /// Authority tokens to grant, verbatim from the device's own catalog.
    #[serde(default)]
    pub authorities: Vec<String>,
}

impl From<DeviceNewUserRequest> for NewUser {
    fn from(request: DeviceNewUserRequest) -> Self {
        Self {
            name: request.name,
            password: Secret::new(request.password.unwrap_or_default()),
            group: request.group,
            authorities: request.authorities,
        }
    }
}

/// Dry-run request: describe a user-management write to classify against the
/// device's current users without mutating it.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct PreviewDeviceUsersRequest {
    /// Whether the previewed write would create or delete accounts.
    pub op: DeviceUserWriteOpKind,
    /// Accounts to provision (used when `op` is `create`).
    #[serde(default)]
    pub users: Vec<DeviceNewUserRequest>,
    /// Login names to remove (used when `op` is `delete`).
    #[serde(default)]
    pub usernames: Vec<String>,
}

impl From<PreviewDeviceUsersRequest> for WriteOp {
    fn from(request: PreviewDeviceUsersRequest) -> Self {
        match request.op {
            DeviceUserWriteOpKind::Create => {
                Self::CreateUsers(request.users.into_iter().map(NewUser::from).collect())
            }
            DeviceUserWriteOpKind::Delete => Self::DeleteUsers(request.usernames),
        }
    }
}

/// Provision request: the accounts to create on the device.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateDeviceUsersRequest {
    /// Accounts to provision.
    #[schema(min_items = 1)]
    pub users: Vec<DeviceNewUserRequest>,
}

/// Removal request: the login names to delete from the device.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct DeleteDeviceUsersRequest {
    /// Login names to remove.
    #[schema(min_items = 1)]
    pub usernames: Vec<String>,
}

/// An edit to one existing user account on the device.
///
/// `name` identifies the target account (never changed). Each remaining field is
/// optional: omit it to leave that facet untouched, set it to change it. Like
/// `DeviceNewUserRequest`, `password` is **write-only** — accepted on the
/// request but never echoed. An update that changes nothing is rejected.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UserUpdateRequest {
    /// Login name of the account to edit (identity; never modified).
    pub name: String,
    /// New plaintext password to set, if changing it (write-only; never returned).
    #[serde(default)]
    pub password: Option<String>,
    /// New group/role to assign, if changing it.
    #[serde(default)]
    pub group: Option<String>,
    /// Replacement authority tokens to grant, verbatim, if changing them.
    #[serde(default)]
    pub authorities: Option<Vec<String>>,
}

impl From<UserUpdateRequest> for UserUpdate {
    fn from(request: UserUpdateRequest) -> Self {
        Self {
            name: request.name,
            password: request.password.map(Secret::new),
            group: request.group,
            authorities: request.authorities,
        }
    }
}

/// Edit request: the accounts to update on the device.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateDeviceUsersRequest {
    /// Accounts to edit.
    #[schema(min_items = 1)]
    pub users: Vec<UserUpdateRequest>,
}

impl PreviewDeviceUsersRequest {
    /// A preview must name the collection the selected `op` consumes.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        match self.op {
            DeviceUserWriteOpKind::Create => {
                require_bulk_items(&self.users)?;
            }
            DeviceUserWriteOpKind::Delete => {
                require_bulk_items(&self.usernames)?;
            }
        }
        Ok(())
    }
}

impl CreateDeviceUsersRequest {
    /// A create must name at least one account.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.users)?;
        Ok(())
    }
}

impl DeleteDeviceUsersRequest {
    /// A delete must name at least one login.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.usernames)?;
        Ok(())
    }
}

impl UpdateDeviceUsersRequest {
    /// An update must name at least one account.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.users)?;
        Ok(())
    }
}

/// What a preview determined would happen to one requested name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceUserPreviewActionResponse {
    /// A requested create whose name is absent — it would be created.
    Create,
    /// A requested create whose name already exists — it would be skipped.
    Skip,
    /// A requested delete whose name is present — it would be removed.
    Delete,
    /// A requested delete whose name is absent — nothing to remove.
    Missing,
}

impl From<PreviewAction> for DeviceUserPreviewActionResponse {
    fn from(action: PreviewAction) -> Self {
        match action {
            PreviewAction::Create => Self::Create,
            PreviewAction::Skip => Self::Skip,
            PreviewAction::Delete => Self::Delete,
            PreviewAction::Missing => Self::Missing,
        }
    }
}

/// The preview verdict for one requested name.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeviceUserPreviewItemResponse {
    /// The requested login name, verbatim.
    pub name: String,
    /// What would happen to this name on a subsequent write.
    pub action: DeviceUserPreviewActionResponse,
    /// A short, human-readable explanation of the action.
    pub reason: String,
}

impl From<PreviewItem> for DeviceUserPreviewItemResponse {
    fn from(item: PreviewItem) -> Self {
        Self {
            name: item.name,
            action: item.action.into(),
            reason: item.reason,
        }
    }
}

/// Dry-run result: per-name verdicts plus whether the caller may write.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeviceUsersPreviewResponse {
    /// One verdict per requested name, in request order.
    pub per_item: Vec<DeviceUserPreviewItemResponse>,
    /// Whether a subsequent write would be authorized for this caller and target.
    /// Preview and apply use the same `device_users:connection_target` permission.
    pub writes_enabled: bool,
}

impl DeviceUsersPreviewResponse {
    pub(crate) fn new(preview: WritePreview, writes_enabled: bool) -> Self {
        Self {
            per_item: preview
                .per_item
                .into_iter()
                .map(DeviceUserPreviewItemResponse::from)
                .collect(),
            writes_enabled,
        }
    }
}

/// One account a bulk write failed for.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeviceUserBulkFailureResponse {
    /// Login name the operation failed for.
    pub name: String,
    /// Protocol-neutral machine-readable failure reason.
    pub reason_code: ConnectionReasonCode,
    /// Safe user-facing failure message; device details are never exposed.
    pub message: &'static str,
}

/// Per-account outcome of a bulk provision/removal, preserving partial success.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeviceUsersBulkOutcomeResponse {
    /// Login names the operation succeeded for.
    pub succeeded: Vec<String>,
    /// Login names that failed, each paired with the reason.
    pub failed: Vec<DeviceUserBulkFailureResponse>,
    /// Whether every requested account succeeded.
    pub all_succeeded: bool,
}

impl From<BulkOutcome> for DeviceUsersBulkOutcomeResponse {
    fn from(outcome: BulkOutcome) -> Self {
        let all_succeeded = outcome.all_succeeded();
        Self {
            succeeded: outcome.succeeded,
            failed: outcome
                .failed
                .into_iter()
                .map(|(name, error)| DeviceUserBulkFailureResponse {
                    name,
                    reason_code: connection_reason_for_driver_error(&error),
                    message: CONNECTION_FAILURE_MESSAGE,
                })
                .collect(),
            all_succeeded,
        }
    }
}

/// Read the device's static feature manifest and all caller `cctv:*` verdicts.
///
/// Available when at least one CCTV permission is granted for this target.
/// Resolving builds the driver but does not contact the device.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/capabilities",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_cctv_capabilities",
    responses(
        (status = 200, description = "Device capability set", body = DeviceCapabilitiesResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cctv_capabilities(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let id = match parse_target_id(&target_id) {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    let permissions = match cctv_permissions(&state, &request_context, id).await {
        Ok(permissions) => permissions,
        Err(error) => return handle_app_error(error),
    };
    if !permissions.has_any() {
        return handle_app_error(AppError::Forbidden(
            "Missing CCTV permission for this target".to_owned(),
        ));
    }
    let service = build_device_management_service(&state);
    match service.capabilities(id).await {
        Ok(caps) => (
            StatusCode::OK,
            Json(DeviceCapabilitiesResponse::new(caps, permissions)),
        )
            .into_response(),
        Err(error) => map_service_error(error),
    }
}

/// Enumerate the device's camera/input channels.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/channels",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_cctv_channels",
    responses(
        (status = 200, description = "Device channels", body = DeviceChannelsResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cctv_channels(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::CctvView,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.list_channels(id).await {
        Ok(channels) => {
            let response = DeviceChannelsResponse {
                channels: channels
                    .into_iter()
                    .map(DeviceChannelResponse::from)
                    .collect(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => map_service_error(error),
    }
}

/// Read device identity, firmware, and clock.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/info",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_info",
    responses(
        (status = 200, description = "Device identity and firmware", body = DeviceInfoResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_info(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::CctvView,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.get_info(id).await {
        Ok(info) => (StatusCode::OK, Json(DeviceInfoResponse::from(info))).into_response(),
        Err(error) => map_service_error(error),
    }
}

/// List the device's user accounts.
#[utoipa::path(
    get,
    path = "/api/v1/device-fleet/{target_id}/users",
    tag = "devices",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_users",
    responses(
        (status = 200, description = "Device user accounts", body = DeviceUsersResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.list_users(id).await {
        Ok(users) => {
            let response = DeviceUsersResponse {
                users: users.into_iter().map(DeviceUserResponse::from).collect(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => map_service_error(error),
    }
}

/// List authority/permission tokens accepted by the device for user accounts.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/authorities",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_authorities",
    responses(
        (status = 200, description = "Device authority catalog", body = DeviceAuthoritiesResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_authorities(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.list_authorities(id).await {
        Ok(authorities) => (
            StatusCode::OK,
            Json(DeviceAuthoritiesResponse { authorities }),
        )
            .into_response(),
        Err(error) => map_service_error(error),
    }
}

/// Read the device's disk/storage health.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/disk",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_disk",
    responses(
        (status = 200, description = "Device disk health", body = DeviceDiskResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_disk(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::CctvSettings,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.disk_status(id).await {
        Ok(disks) => {
            let response = DeviceDiskResponse {
                disks: disks
                    .into_iter()
                    .map(DeviceDiskEntryResponse::from)
                    .collect(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => map_service_error(error),
    }
}

/// Read the device's recording state and available footage window.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/recording",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_recording",
    responses(
        (status = 200, description = "Device recording state", body = DeviceRecordingResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_recording(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::CctvRecordings,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.recording_status(id).await {
        Ok(status) => (StatusCode::OK, Json(DeviceRecordingResponse::from(status))).into_response(),
        Err(error) => map_service_error(error),
    }
}

/// Recorded spans returned by a per-channel search.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceRecordingSearchResponse {
    /// Matching recorded spans, each attributed to the requested channel.
    pub spans: Vec<DeviceRecordSpanResponse>,
}

/// Search recorded footage for one channel over a time window.
///
/// Requires `cctv:recordings`. Distinct from `/recording`, which reads a
/// coarse device-wide window: this answers "what footage exists on channel N
/// between these two timestamps" — what an archive timeline needs to render
/// honest per-camera gaps, and what a playback/export request can consult
/// first to avoid paying a device session for an empty window.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/recordings",
    tag = "cctv",
    params(
        DeviceExportQuery,
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "search_device_recordings",
    responses(
        (status = 200, description = "Matching recorded spans", body = DeviceRecordingSearchResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn search_device_recordings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    Query(query): Query<DeviceExportQuery>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        auth_context,
        &headers,
        &target_id,
        Action::CctvRecordings,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    let request = match parse_search_request(&query) {
        Ok(request) => request,
        Err(error) => return handle_app_error(error),
    };
    match service.search_recordings(id, request).await {
        Ok(spans) => (
            StatusCode::OK,
            Json(DeviceRecordingSearchResponse {
                spans: spans
                    .into_iter()
                    .map(DeviceRecordSpanResponse::from)
                    .collect(),
            }),
        )
            .into_response(),
        Err(error) => map_service_error(error),
    }
}

/// Query shape for the live MJPEG stream endpoint.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct DeviceStreamQuery {
    /// Camera/input channel to stream. Omitted uses the device's default channel.
    pub channel: Option<u16>,
    /// Output frame rate (clamped to 1..=15, default 6).
    pub fps: Option<u16>,
    /// Output width ceiling (clamped to 320..=1920, default 1280; never upscaled).
    pub max_width: Option<u16>,
}

/// Restream a channel's live video as multipart MJPEG.
///
/// Requires `cctv:view` for this target. The response is
/// `multipart/x-mixed-replace`; closing it tears the server-side transcode down.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/stream",
    tag = "cctv",
    params(
        DeviceStreamQuery,
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_stream",
    responses(
        (status = 200, description = "Live MJPEG multipart stream", content_type = "multipart/x-mixed-replace", body = Vec<u8>),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 429, description = "All live-stream slots are busy", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_stream(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    Query(query): Query<DeviceStreamQuery>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        Some(auth_context),
        &headers,
        &target_id,
        Action::CctvView,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    let options = LiveStreamOptions::clamped(query.fps, query.max_width);
    match service.live_mjpeg(id, query.channel, options).await {
        Ok(stream) => {
            let content_type = format!(
                "multipart/x-mixed-replace; boundary={boundary}",
                boundary = stream.boundary,
            );
            let Ok(content_type) = HeaderValue::from_str(&content_type) else {
                return handle_app_error(AppError::Internal(
                    "invalid stream content type".to_string(),
                ));
            };
            (
                StatusCode::OK,
                [(CONTENT_TYPE, content_type)],
                Body::from_stream(stream.bytes),
            )
                .into_response()
        }
        Err(error) => map_service_error(error),
    }
}

/// Optional channel selector for a snapshot request.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeviceSnapshotQuery {
    /// Camera/input channel to capture. Omitted uses the device's default channel.
    pub channel: Option<u16>,
}

/// Capture a still JPEG image from the device.
///
/// Requires `cctv:view` for this target. Every family returns inline JPEG bytes
/// with `image/jpeg` content type — the native families capture directly, and
/// generic ONVIF resolves the snapshot URI and fetches it server-side behind an
/// SSRF fence, so a credential-bearing URI never reaches the client.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/snapshot",
    tag = "cctv",
    params(
        DeviceSnapshotQuery,
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_device_snapshot",
    responses(
        (status = 200, description = "Captured still image", content_type = "image/jpeg", body = Vec<u8>),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_device_snapshot(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    Query(query): Query<DeviceSnapshotQuery>,
) -> impl IntoResponse {
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        Some(auth_context),
        &headers,
        &target_id,
        Action::CctvView,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.snapshot(id, query.channel).await {
        Ok(SnapshotRef::Bytes(bytes)) => match jpeg_response(bytes) {
            Ok(response) => response,
            Err(error) => handle_app_error(error),
        },
        // Defensive (2026-07-19): no shipped driver returns a bare URI — ONVIF,
        // the last producer, now fetches server-side. The `Uri` variant + this
        // arm are kept for forward compatibility: a future driver that surfaces a
        // URI must add its own SSRF-fenced proxy-fetch (see
        // `HttpDeviceClient::get_image_from_url`) rather than leak a
        // credential-bearing reference to the client, so this fails closed.
        Ok(SnapshotRef::Uri(_)) => handle_app_error(AppError::Validation(
            "snapshot-by-URI is not supported for this device".to_string(),
        )),
        Err(error) => map_service_error(error),
    }
}

/// Build a binary `image/jpeg` response from raw still bytes, mirroring the SSH
/// file-download binary path (explicit content type and length).
fn jpeg_response(bytes: Vec<u8>) -> Result<Response, AppError> {
    let length = bytes.len();
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).map_err(|error| {
            AppError::External(format!("Invalid snapshot size header: {error}"))
        })?,
    );
    Ok(response)
}

/// Issue a pan/tilt/zoom control command against a channel.
///
/// Requires `cctv:ptz` for this target and confirmed channel-level PTZ support.
#[utoipa::path(
    post,
    path = "/api/v1/cctv/{target_id}/ptz",
    tag = "cctv",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    request_body = PtzCommandRequest,
    operation_id = "cctv_ptz",
    responses(
        (status = 204, description = "PTZ command accepted"),
        (status = 403, description = "Caller lacks cctv:ptz on this target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn cctv_ptz(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    EnvelopeJson(body): EnvelopeJson<PtzCommandRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let (service, id) =
        match prepare_for_cctv_permission(&state, &request_context, &target_id, Action::CctvPtz)
            .await
        {
            Ok(prepared) => prepared,
            Err(response) => return *response,
        };
    if let Err(error) = service.ptz(id, body.channel, body.to_command()).await {
        return map_service_error(error);
    }
    record_cctv_operation(
        state.services.audit.as_ref(),
        &request_context,
        "cctv.ptz.commanded",
        "cctv_ptz",
        id,
        CctvAuditDetails::Ptz {
            channel: body.channel,
            action: body.audit_action(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

/// Export recorded footage from a channel over a time window.
///
/// Requires `cctv:export` for this target. The clip is streamed back as a typed
/// binary download and never exposes a credential-bearing playback URI.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/export",
    tag = "cctv",
    params(
        DeviceExportQuery,
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_cctv_export",
    responses(
        (status = 200, description = "Exported clip", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cctv_export(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    Query(query): Query<DeviceExportQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let (service, id) =
        match prepare_for_cctv_permission(&state, &request_context, &target_id, Action::CctvExport)
            .await
        {
            Ok(prepared) => prepared,
            Err(response) => return *response,
        };
    let request = match parse_export_request(&query) {
        Ok(request) => request,
        Err(error) => return handle_app_error(error),
    };
    let audit_channel = request.channel;
    let audit_start = request.start.to_rfc3339();
    let audit_end = request.end.to_rfc3339();
    let clip = match service.export_clip(id, request, ClipEncoding::Copy).await {
        Ok(clip) => clip,
        Err(error) => return map_service_error(error),
    };
    let response = match clip_response(clip, ClipDelivery::Download) {
        Ok(response) => response,
        Err(error) => return handle_app_error(error),
    };
    record_cctv_operation(
        state.services.audit.as_ref(),
        &request_context,
        "cctv.recording.exported",
        "get_cctv_export",
        id,
        CctvAuditDetails::Export {
            channel: audit_channel,
            start: &audit_start,
            end: &audit_end,
        },
    )
    .await;
    response
}

/// Play recorded footage for a channel and time window in the browser.
///
/// Requires `cctv:recordings` for this target — watching history is a read of
/// the same class as the recording metadata beside it, while taking a copy away
/// is the separate `cctv:export` grant on `/export`. The bytes are the same
/// fragmented MP4; only the disposition and the permission differ, so an
/// operator can be allowed to review an incident without being able to export
/// footage out of the system.
#[utoipa::path(
    get,
    path = "/api/v1/cctv/{target_id}/playback",
    tag = "cctv",
    params(
        DeviceExportQuery,
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "get_cctv_playback",
    responses(
        (status = 200, description = "Recorded footage stream", content_type = "video/mp4", body = Vec<u8>),
        (status = 403, description = "Caller lacks cctv:recordings for this target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cctv_playback(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    Query(query): Query<DeviceExportQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let (service, id) = match prepare_for_cctv_permission(
        &state,
        &request_context,
        &target_id,
        Action::CctvRecordings,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    let request = match parse_export_request(&query) {
        Ok(request) => request,
        Err(error) => return handle_app_error(error),
    };
    let encoding = playback_encoding(query.transcode);
    match service.export_clip(id, request, encoding).await {
        Ok(clip) => match clip_response(clip, ClipDelivery::Watch) {
            Ok(response) => response,
            Err(error) => handle_app_error(error),
        },
        Err(error) => map_service_error(error),
    }
}

/// Parse the export query's RFC3339 window into a typed [`ExportRequest`].
/// Longest export window an export will attempt. The RTSP-playback path runs at
/// realtime (a clip takes about as long to export as it is to watch) while the
/// device-native file download (Dahua `loadfile`) runs several times faster; the
/// cap only guards against a mistaken multi-hour request tying up an export slot.
/// The default admits hour-long archives with headroom; raise it for an operator
/// who needs longer via `PIPING_CCTV__MAX_EXPORT_WINDOW_SECS`.
const DEFAULT_MAX_EXPORT_WINDOW_SECONDS: i64 = 2 * 60 * 60;

fn configured_export_window_seconds(value: Option<&str>) -> i64 {
    value
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_EXPORT_WINDOW_SECONDS)
}

pub(crate) fn max_export_window_seconds() -> i64 {
    static VALUE: std::sync::LazyLock<i64> = std::sync::LazyLock::new(|| {
        let configured = std::env::var("PIPING_CCTV__MAX_EXPORT_WINDOW_SECS").ok();
        configured_export_window_seconds(configured.as_deref())
    });
    *VALUE
}

fn playback_encoding(transcode: Option<bool>) -> ClipEncoding {
    if transcode.unwrap_or(false) {
        ClipEncoding::BrowserCompatible
    } else {
        ClipEncoding::Copy
    }
}

fn parse_export_request(query: &DeviceExportQuery) -> Result<ExportRequest, AppError> {
    let (start, end) = parse_window(query)?;
    let max_window = max_export_window_seconds();
    if (end - start).num_seconds() > max_window {
        return Err(AppError::Validation(format!(
            "export window exceeds the {max_window}s maximum; request a shorter range"
        )));
    }
    Ok(ExportRequest {
        channel: query.channel,
        start,
        end,
    })
}

/// Longest window a recording SEARCH may cover.
///
/// Unlike export/playback, a search moves no media bytes and opens no device
/// session — it is a metadata read an archive timeline can call with a wide
/// range. The cap exists only to bound a device-controlled reply, not to
/// protect a media slot, so it is far more generous than
/// [`max_export_window_seconds`].
const MAX_SEARCH_WINDOW_SECONDS: i64 = 30 * 24 * 60 * 60;

/// Parse the search query's RFC3339 window, capped at
/// [`MAX_SEARCH_WINDOW_SECONDS`] rather than the export/playback limit.
fn parse_search_request(query: &DeviceExportQuery) -> Result<ExportRequest, AppError> {
    let (start, end) = parse_window(query)?;
    if (end - start).num_seconds() > MAX_SEARCH_WINDOW_SECONDS {
        return Err(AppError::Validation(format!(
            "search window exceeds the {MAX_SEARCH_WINDOW_SECONDS}s maximum; request a shorter range"
        )));
    }
    Ok(ExportRequest {
        channel: query.channel,
        start,
        end,
    })
}

/// Parse and validate the shared start/end shape both window kinds check.
fn parse_window(
    query: &DeviceExportQuery,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>), AppError> {
    let start = parse_rfc3339(&query.start, "start")?;
    let end = parse_rfc3339(&query.end, "end")?;
    if end <= start {
        return Err(AppError::Validation(
            "window end must be after start".to_owned(),
        ));
    }
    Ok((start, end))
}

/// Parse one RFC3339 timestamp, naming the field in the error.
fn parse_rfc3339(value: &str, field: &str) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|error| AppError::Validation(format!("invalid {field} timestamp: {error}")))
}

/// How a recorded clip is delivered to the caller.
///
/// The bytes are identical either way — this only decides whether the browser
/// plays the stream in place or saves it. The two are separate REST operations
/// because they answer to different permissions: watching history is
/// `cctv:recordings`, taking a copy away is `cctv:export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipDelivery {
    /// Play in the page (`inline`).
    Watch,
    /// Save to disk (`attachment`).
    Download,
}

impl ClipDelivery {
    /// The `Content-Disposition` type this delivery uses.
    fn disposition(self) -> &'static str {
        match self {
            Self::Watch => "inline",
            Self::Download => "attachment",
        }
    }
}

/// Build a STREAMING response from an exported clip: the MP4 bytes are piped
/// straight from ffmpeg to the client (chunked, no `Content-Length`), so an
/// hour-long window never buffers the whole clip in memory.
///
/// The MP4 is fragmented (`frag_keyframe+empty_moov`), which is what makes the
/// same stream progressively playable in a `<video>` as it arrives rather than
/// only after a full download.
fn clip_response(clip: ClipStream, delivery: ClipDelivery) -> Result<Response, AppError> {
    let ClipStream {
        content_type,
        filename,
        bytes,
    } = clip;
    let mut response = Response::new(Body::from_stream(bytes));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .map_err(|error| AppError::External(format!("Invalid clip content type: {error}")))?,
    );
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "{}; filename=\"{}\"",
            delivery.disposition(),
            sanitize_filename(&filename)
        ))
        .map_err(|error| AppError::External(format!("Invalid clip filename: {error}")))?,
    );
    Ok(response)
}

/// Keep a download filename to a safe ASCII subset so it cannot break the
/// `Content-Disposition` header or suggest a path. Purely structural.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "clip.bin".to_owned()
    } else {
        cleaned
    }
}

/// Dry-run a user-management write against the device without mutating it.
///
/// A read-only dry run gated on `device_users:connection_target`; the same permission authorizes a
/// subsequent apply, so `writes_enabled` is true for every successful preview.
#[utoipa::path(
    post,
    path = "/api/v1/device-fleet/{target_id}/users:preview",
    tag = "devices",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "preview_device_users",
    request_body = PreviewDeviceUsersRequest,
    responses(
        (status = 200, description = "Dry-run preview of a user-management write", body = DeviceUsersPreviewResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_device_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    EnvelopeJson(body): EnvelopeJson<PreviewDeviceUsersRequest>,
) -> impl IntoResponse {
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let (service, id) = match prepared_cctv(
        &state,
        &request_id,
        Some(auth_context),
        &headers,
        &target_id,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    match service.preview_users(id, WriteOp::from(body)).await {
        Ok(preview) => (
            StatusCode::OK,
            Json(DeviceUsersPreviewResponse::new(preview, true)),
        )
            .into_response(),
        Err(error) => map_service_error(error),
    }
}

/// Provision user accounts on the device.
///
/// Requires `device_users:connection_target` for this target. Returns a
/// per-account outcome so partial success is visible; a per-account device
/// rejection is reported inside the outcome, not as an error status.
#[utoipa::path(
    post,
    path = "/api/v1/device-fleet/{target_id}/users:create",
    tag = "devices",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "create_device_users",
    request_body = CreateDeviceUsersRequest,
    responses(
        (status = 200, description = "Per-account provisioning outcome", body = DeviceUsersBulkOutcomeResponse),
        (status = 403, description = "Caller lacks device_users:connection_target on this target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_device_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    EnvelopeJson(body): EnvelopeJson<CreateDeviceUsersRequest>,
) -> impl IntoResponse {
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let (service, id) = match prepare_for_cctv_permission(
        &state,
        &request_context,
        &target_id,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    let users: Vec<NewUser> = body.users.into_iter().map(NewUser::from).collect();
    let outcome = match service.create_users(id, &users).await {
        Ok(outcome) => outcome,
        Err(error) => return map_service_error(error),
    };
    let response = DeviceUsersBulkOutcomeResponse::from(outcome);
    record_cctv_operation(
        state.services.audit.as_ref(),
        &request_context,
        "device.users.created",
        "create_device_users",
        id,
        CctvAuditDetails::DeviceUsers { outcome: &response },
    )
    .await;
    (StatusCode::OK, Json(response)).into_response()
}

/// Edit existing user accounts on the device (password/group/authorities).
///
/// Requires `device_users:connection_target` for this target. Each account is
/// identified by `name`; only the facets a request supplies are changed. Returns
/// a per-account outcome so partial success is visible; a per-account device
/// rejection is reported inside the outcome, not as an error status. An update
/// that changes nothing (or names a blank account) is a 400.
#[utoipa::path(
    put,
    path = "/api/v1/device-fleet/{target_id}/users",
    tag = "devices",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "update_device_users",
    request_body = UpdateDeviceUsersRequest,
    responses(
        (status = 200, description = "Per-account edit outcome", body = DeviceUsersBulkOutcomeResponse),
        (status = 403, description = "Caller lacks device_users:connection_target on this target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_device_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    EnvelopeJson(body): EnvelopeJson<UpdateDeviceUsersRequest>,
) -> impl IntoResponse {
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let (service, id) = match prepare_for_cctv_permission(
        &state,
        &request_context,
        &target_id,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    let updates: Vec<UserUpdate> = body.users.into_iter().map(UserUpdate::from).collect();
    let outcome = match service.update_users(id, &updates).await {
        Ok(outcome) => outcome,
        Err(error) => return map_service_error(error),
    };
    let response = DeviceUsersBulkOutcomeResponse::from(outcome);
    record_cctv_operation(
        state.services.audit.as_ref(),
        &request_context,
        "device.users.updated",
        "update_device_users",
        id,
        CctvAuditDetails::DeviceUsers { outcome: &response },
    )
    .await;
    (StatusCode::OK, Json(response)).into_response()
}

/// Remove user accounts from the device by login name.
///
/// Requires `device_users:connection_target` for this target. Returns a
/// per-account outcome.
#[utoipa::path(
    post,
    path = "/api/v1/device-fleet/{target_id}/users:delete",
    tag = "devices",
    params(
        ("target_id" = String, Path, description = "Connection target id"),
    ),
    operation_id = "delete_device_users",
    request_body = DeleteDeviceUsersRequest,
    responses(
        (status = 200, description = "Per-account removal outcome", body = DeviceUsersBulkOutcomeResponse),
        (status = 403, description = "Caller lacks device_users:connection_target on this target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_device_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    EnvelopeJson(body): EnvelopeJson<DeleteDeviceUsersRequest>,
) -> impl IntoResponse {
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let (service, id) = match prepare_for_cctv_permission(
        &state,
        &request_context,
        &target_id,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return *response,
    };
    let outcome = match service.delete_users(id, &body.usernames).await {
        Ok(outcome) => outcome,
        Err(error) => return map_service_error(error),
    };
    let response = DeviceUsersBulkOutcomeResponse::from(outcome);
    record_cctv_operation(
        state.services.audit.as_ref(),
        &request_context,
        "device.users.deleted",
        "delete_device_users",
        id,
        CctvAuditDetails::DeviceUsers { outcome: &response },
    )
    .await;
    (StatusCode::OK, Json(response)).into_response()
}

/// Authorize one per-target device permission before device or media I/O.
/// Handler prologue in one call: build the request context from the axum
/// extractors, authorize `action` against this specific target, and resolve the
/// device-management service. Every device handler opened with the same two
/// statements; keeping them together means a change to how a device request is
/// authorized lands once instead of seventeen times.
async fn prepared_cctv(
    state: &AppState,
    request_id: &RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: &HeaderMap,
    target_id: &str,
    action: Action,
) -> Result<(DeviceManagementService, Uuid), Box<Response>> {
    let request_context = build_request_context(request_id, auth_context, headers);
    prepare_for_cctv_permission(state, &request_context, target_id, action).await
}

async fn prepare_for_cctv_permission(
    state: &AppState,
    request_context: &RequestContext,
    target_id: &str,
    action: Action,
) -> Result<(DeviceManagementService, Uuid), Box<Response>> {
    prepare_with(state, request_context, target_id, action).await
}

/// Shared handler prologue: parse the target id, authorize `action` against the
/// specific connection target, and build the device-management service. On any
/// failure it returns the mapped error response so callers can early-return it.
async fn prepare_with(
    state: &AppState,
    request_context: &RequestContext,
    target_id: &str,
    action: Action,
) -> Result<(DeviceManagementService, Uuid), Box<Response>> {
    let id = parse_target_id(target_id).map_err(|error| Box::new(handle_app_error(error)))?;
    ensure_target_scoped_access(state, request_context, id, action)
        .await
        .map_err(|error| Box::new(handle_app_error(error)))?;
    Ok((build_device_management_service(state), id))
}

/// The caller's RBAC verdict for one CCTV write action on this target: `true`
/// when the write endpoint would authorize it, `false` on an RBAC denial. Any
/// non-authorization failure (e.g. the attribute lookup) propagates as an error
/// rather than masquerading as "not permitted".
///
/// `pub(crate)` so the fleet preview endpoint reports the same per-target
/// verdict inside each target's preview.
pub(crate) async fn cctv_permission_verdict(
    state: &AppState,
    request_context: &RequestContext,
    target_id: Uuid,
    action: Action,
) -> Result<bool, AppError> {
    match ensure_target_scoped_access(state, request_context, target_id, action).await {
        Ok(()) => Ok(true),
        Err(AppError::Forbidden(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

async fn cctv_permissions(
    state: &AppState,
    request_context: &RequestContext,
    target_id: Uuid,
) -> Result<DeviceCctvPermissionsResponse, AppError> {
    Ok(DeviceCctvPermissionsResponse {
        view: cctv_permission_verdict(state, request_context, target_id, Action::CctvView).await?,
        recordings: cctv_permission_verdict(
            state,
            request_context,
            target_id,
            Action::CctvRecordings,
        )
        .await?,
        export: cctv_permission_verdict(state, request_context, target_id, Action::CctvExport)
            .await?,
        users: cctv_permission_verdict(state, request_context, target_id, Action::DeviceUsers)
            .await?,
        settings: cctv_permission_verdict(state, request_context, target_id, Action::CctvSettings)
            .await?,
        ptz: cctv_permission_verdict(state, request_context, target_id, Action::CctvPtz).await?,
    })
}

fn parse_target_id(value: &str) -> Result<Uuid, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "target_id cannot be empty".to_string(),
        ));
    }
    Uuid::parse_str(trimmed)
        .map_err(|error| AppError::Validation(format!("Invalid target_id: {error}")))
}

/// The resource type a per-target permission check evaluates: the CCTV
/// namespace for `cctv:*` actions and the neutral connection target for the
/// device plane (e.g. [`Action::DeviceUsers`]). This is the single place the
/// action-to-resource mapping for per-target gates lives.
fn permission_resource_type(action: &Action) -> ResourceType {
    if action.is_cctv_permission() {
        ResourceType::Cctv
    } else {
        ResourceType::ConnectionTarget
    }
}

/// Per-target permission checks evaluate `cctv:*` actions against
/// `ResourceType::Cctv` and device-plane actions (such as [`Action::DeviceUsers`])
/// against `ResourceType::ConnectionTarget`; the target id and connection-target
/// attributes preserve per-target patterns and ABAC constraints.
///
/// `pub(crate)` so the fleet endpoints (`device_fleet`) reuse the exact same
/// per-target gate, running it once per selected target to partition the fleet
/// into permitted vs `not_permitted` without changing this decision anywhere.
pub(crate) async fn ensure_target_scoped_access(
    state: &AppState,
    request_context: &RequestContext,
    target_id: Uuid,
    action: Action,
) -> Result<(), AppError> {
    let resource_type = permission_resource_type(&action);
    let resource_id = format!("{}:{target_id}", resource_type.as_str());
    let attributes = connection_target_authz_attributes_for(state, target_id, None).await?;
    build_policy_evaluation_service(state)
        .ensure_access_with_attributes(
            request_context,
            action,
            resource_type,
            Some(resource_id.as_str()),
            Some(attributes),
        )
        .await
}

/// Convert a device-management failure to a safe canonical REST response.
///
/// The domain variants carry diagnostics for structured server logs, but the
/// response deliberately contains only protocol-neutral reason codes and a
/// generic message. Classification is exhaustive and never inspects text.
fn map_service_error(error: DeviceServiceError) -> Response {
    let diagnostic = error.to_string();
    let mapping = match error {
        DeviceServiceError::Resolve(resolve) => return map_resolve_error(resolve),
        DeviceServiceError::Driver(driver) => driver_error_mapping(driver),
        DeviceServiceError::InvalidRequest(_) => ConnectionErrorMapping::new(
            StatusCode::BAD_REQUEST,
            ConnectionReasonCode::ConnectionUnsupported,
        ),
        DeviceServiceError::LiveBusy => ConnectionErrorMapping::new(
            StatusCode::TOO_MANY_REQUESTS,
            ConnectionReasonCode::ConnectionBusy,
        ),
    };
    log_device_error(mapping.status, &diagnostic);
    mapping.into_response()
}

fn map_resolve_error(error: DeviceResolveError) -> Response {
    let diagnostic = error.to_string();
    let mapping = match error {
        DeviceResolveError::NotFound(_) => {
            return handle_app_error(AppError::NotFound("Connection target not found".to_owned()));
        }
        DeviceResolveError::NoCredentials(_) => ConnectionErrorMapping::new(
            StatusCode::BAD_REQUEST,
            ConnectionReasonCode::ConnectionCredentialsRequired,
        ),
        DeviceResolveError::UnknownFamily(_) | DeviceResolveError::NoAddress(_) => {
            ConnectionErrorMapping::new(
                StatusCode::BAD_REQUEST,
                ConnectionReasonCode::ConnectionUnsupported,
            )
        }
        DeviceResolveError::Internal(_) => ConnectionErrorMapping::new(
            StatusCode::BAD_GATEWAY,
            ConnectionReasonCode::ConnectionFailed,
        ),
    };
    log_device_error(mapping.status, &diagnostic);
    mapping.into_response()
}

#[derive(Debug, Clone, Copy)]
struct ConnectionErrorMapping {
    status: StatusCode,
    reason: ConnectionReasonCode,
}

impl ConnectionErrorMapping {
    const fn new(status: StatusCode, reason: ConnectionReasonCode) -> Self {
        Self { status, reason }
    }

    fn into_response(self) -> Response {
        build_error_response(
            self.status,
            reason_codes::CONNECTION_FAILED,
            CONNECTION_FAILURE_MESSAGE,
            self.reason.as_str(),
        )
    }
}

fn connection_reason_for_driver_error(error: &DriverError) -> ConnectionReasonCode {
    match error {
        DriverError::Auth => ConnectionReasonCode::ConnectionAuthenticationFailed,
        DriverError::Timeout(_) => ConnectionReasonCode::ConnectionTimeout,
        DriverError::Unreachable(_) | DriverError::Transport(_) => {
            ConnectionReasonCode::ConnectionUnreachable
        }
        DriverError::Busy(_) => ConnectionReasonCode::ConnectionBusy,
        DriverError::Unsupported => ConnectionReasonCode::ConnectionUnsupported,
        DriverError::ClockSkew | DriverError::Protocol(_) | DriverError::Device(_) => {
            ConnectionReasonCode::ConnectionFailed
        }
    }
}

fn driver_error_mapping(error: DriverError) -> ConnectionErrorMapping {
    let reason = connection_reason_for_driver_error(&error);
    let status = match reason {
        ConnectionReasonCode::ConnectionAuthenticationFailed
        | ConnectionReasonCode::ConnectionUnsupported
        | ConnectionReasonCode::ConnectionCredentialsRequired => StatusCode::BAD_REQUEST,
        ConnectionReasonCode::ConnectionTimeout => StatusCode::GATEWAY_TIMEOUT,
        ConnectionReasonCode::ConnectionUnreachable | ConnectionReasonCode::ConnectionFailed => {
            StatusCode::BAD_GATEWAY
        }
        ConnectionReasonCode::ConnectionBusy => StatusCode::TOO_MANY_REQUESTS,
    };
    ConnectionErrorMapping::new(status, reason)
}

fn log_device_error(status: StatusCode, diagnostic: &str) {
    if status.is_server_error() {
        log_device_operation_failure(status, diagnostic);
    } else {
        log_device_operation_rejection(status, diagnostic);
    }
}

fn log_device_operation_failure(status: StatusCode, diagnostic: &str) {
    tracing::error!(
        target: "piping_backend::api::cctv",
        %status,
        error = diagnostic,
        "CCTV device operation failed",
    );
}

fn log_device_operation_rejection(status: StatusCode, diagnostic: &str) {
    tracing::info!(
        target: "piping_backend::api::cctv",
        %status,
        error = diagnostic,
        "CCTV device operation was rejected",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc as SharedArc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use async_trait::async_trait;

    use super::*;
    use crate::contexts::shared::request_context::RequestAuth;

    #[derive(Clone, Default)]
    struct CctvAuditTestRecorder {
        events: SharedArc<Mutex<Vec<AuditEvent>>>,
    }

    #[async_trait]
    impl AuditRecorder for CctvAuditTestRecorder {
        async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("test recorder mutex should not be poisoned")
                .push(event);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingCctvAuditRecorder {
        attempted: AtomicBool,
    }

    #[async_trait]
    impl AuditRecorder for FailingCctvAuditRecorder {
        async fn record_event(&self, _event: AuditEvent) -> Result<(), AppError> {
            self.attempted.store(true, Ordering::SeqCst);
            Err(AppError::Internal("audit store is down".to_string()))
        }
    }

    #[derive(sqlx::FromRow)]
    struct PersistedCctvAuditRow {
        event_type: String,
        actor_id: Option<Uuid>,
        actor_kind: String,
        target_type: Option<String>,
        target_id: Option<Uuid>,
        metadata: serde_json::Value,
    }

    #[test]
    fn device_user_write_requests_reject_empty_batches_with_a_400() {
        use axum::http::StatusCode;

        use crate::api::rest::error_mapping::map_app_error_to_http;

        let create = serde_json::from_value::<CreateDeviceUsersRequest>(serde_json::json!({
            "users": [],
        }))
        .expect("deserialize create");
        let update = serde_json::from_value::<UpdateDeviceUsersRequest>(serde_json::json!({
            "users": [],
        }))
        .expect("deserialize update");
        let delete = serde_json::from_value::<DeleteDeviceUsersRequest>(serde_json::json!({
            "usernames": [],
        }))
        .expect("deserialize delete");
        let preview_create = serde_json::from_value::<PreviewDeviceUsersRequest>(
            serde_json::json!({"op": "create", "users": []}),
        )
        .expect("deserialize preview create");
        let preview_delete = serde_json::from_value::<PreviewDeviceUsersRequest>(
            serde_json::json!({"op": "delete", "usernames": []}),
        )
        .expect("deserialize preview delete");

        for validated in [
            create.validate(),
            update.validate(),
            delete.validate(),
            preview_create.validate(),
            preview_delete.validate(),
        ] {
            let error = validated.expect_err("an empty device-user batch must be rejected");
            assert_eq!(
                map_app_error_to_http(error).status(),
                StatusCode::BAD_REQUEST,
                "an empty device-user batch must answer 400, not 200",
            );
        }
    }

    #[tokio::test]
    async fn cctv_audit_is_complete_credential_free_and_fail_open() {
        let recorder = CctvAuditTestRecorder::default();
        let actor_id = Uuid::now_v7();
        let target_id = Uuid::now_v7();
        let mut request_context = RequestContext::new(
            Some(actor_id),
            "api_token".to_string(),
            None,
            "cctv-audit-request".to_string(),
            RequestAuth::default(),
        );
        request_context.api_token_id = Some("pat-audit-id".to_string());
        let user_outcome = DeviceUsersBulkOutcomeResponse::from(BulkOutcome {
            succeeded: vec!["operator".to_string()],
            failed: vec![("reviewer".to_string(), DriverError::Auth)],
        });

        record_cctv_operation(
            &recorder,
            &request_context,
            "cctv.ptz.commanded",
            "cctv_ptz",
            target_id,
            CctvAuditDetails::Ptz {
                channel: 4,
                action: "preset",
            },
        )
        .await;
        record_cctv_operation(
            &recorder,
            &request_context,
            "cctv.recording.exported",
            "get_cctv_export",
            target_id,
            CctvAuditDetails::Export {
                channel: 7,
                start: "2026-08-11T00:00:00+00:00",
                end: "2026-08-11T00:01:00+00:00",
            },
        )
        .await;
        record_cctv_operation(
            &recorder,
            &request_context,
            "device.users.created",
            "create_device_users",
            target_id,
            CctvAuditDetails::DeviceUsers {
                outcome: &user_outcome,
            },
        )
        .await;
        record_cctv_operation(
            &recorder,
            &request_context,
            "device.users.updated",
            "update_device_users",
            target_id,
            CctvAuditDetails::DeviceUsers {
                outcome: &user_outcome,
            },
        )
        .await;
        record_cctv_operation(
            &recorder,
            &request_context,
            "device.users.deleted",
            "delete_device_users",
            target_id,
            CctvAuditDetails::DeviceUsers {
                outcome: &user_outcome,
            },
        )
        .await;

        {
            let events = recorder
                .events
                .lock()
                .expect("test recorder mutex should not be poisoned");
            assert_eq!(events.len(), 5);
            // The behavioral event-type proof: every handler operation emits the
            // one canonical event type persisted for it.
            let expected_event_types = [
                "cctv.ptz.commanded",
                "cctv.recording.exported",
                "device.users.created",
                "device.users.updated",
                "device.users.deleted",
            ];
            for (event, expected) in events.iter().zip(expected_event_types) {
                assert_eq!(event.event_type, expected);
                assert_eq!(event.actor_id, Some(actor_id));
                assert_eq!(event.actor_kind, "api_token");
                assert_eq!(event.target_type.as_deref(), Some("cctv_device"));
                assert_eq!(event.target_id, Some(target_id));
                // The redaction proof: no credential-bearing key or value
                // survives serialization, case-insensitively, in ANY recorded
                // metadata. `auth_method` is the one legitimate label and this
                // suite drives `api_token`, whose name contains the forbidden
                // substring `token`; the label is not credential material, so it
                // is dropped from the scanned view only (the exact-metadata
                // assertions below still pin it verbatim).
                let mut scanned = event.metadata.clone();
                if let Some(object) = scanned.as_object_mut() {
                    object.remove("auth_method");
                }
                let serialized = scanned.to_string().to_ascii_lowercase();
                for forbidden in ["password", "secret", "token", "credential"] {
                    assert!(
                        !serialized.contains(forbidden),
                        "audit metadata for {expected} leaked `{forbidden}`: {serialized}"
                    );
                }
            }
            assert_eq!(
                events[0].metadata,
                json!({
                    "operation": "cctv_ptz",
                    "device_id": target_id,
                    "auth_method": "api_token",
                    "correlation_id": "cctv-audit-request",
                    "result": "success",
                    "channel": 4,
                    "ptz_action": "preset",
                })
            );
            assert_eq!(
                events[1].metadata,
                json!({
                    "operation": "get_cctv_export",
                    "device_id": target_id,
                    "auth_method": "api_token",
                    "correlation_id": "cctv-audit-request",
                    "result": "success",
                    "channel": 7,
                    "start": "2026-08-11T00:00:00+00:00",
                    "end": "2026-08-11T00:01:00+00:00",
                })
            );
            assert_eq!(events[3].metadata["succeeded_count"], json!(1));
            assert_eq!(events[3].metadata["failed_count"], json!(1));
            assert!(!events[3].metadata.to_string().contains("operator"));
            assert_eq!(
                events[3].metadata,
                json!({
                    "operation": "update_device_users",
                    "device_id": target_id,
                    "auth_method": "api_token",
                    "correlation_id": "cctv-audit-request",
                    "result": "partial_success",
                    "succeeded_count": 1,
                    "failed_count": 1,
                })
            );
            for (index, operation) in [(2, "create_device_users"), (4, "delete_device_users")] {
                let mut expected = events[3].metadata.clone();
                expected["operation"] = json!(operation);
                assert_eq!(events[index].metadata, expected);
            }
        }

        let failing_recorder = FailingCctvAuditRecorder::default();
        let (): () = record_cctv_operation(
            &failing_recorder,
            &request_context,
            "cctv.ptz.commanded",
            "cctv_ptz",
            target_id,
            CctvAuditDetails::Ptz {
                channel: 1,
                action: "stop",
            },
        )
        .await;
        assert!(failing_recorder.attempted.load(Ordering::SeqCst));
    }

    #[tokio::test]
    #[ignore = "requires a disposable PostgreSQL database"]
    async fn all_cctv_audit_variants_persist_actor_target_and_redacted_metadata()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use std::str::FromStr;

        use crate::contexts::audit::application::AuditService;
        use crate::contexts::audit::infrastructure::SqlxAuditRepository;

        let url = std::env::var("PIPING_DATABASE__URL")?;
        let options = sqlx::postgres::PgConnectOptions::from_str(&url)?;
        assert_eq!(std::env::var("PIPING_TEST_DISPOSABLE_DB")?.as_str(), "1");
        assert!(
            options
                .get_database()
                .is_some_and(|name| name == "pwa10_coverage" || name.starts_with("pwa63_")),
            "persistence test must only write to a disposable PWA coverage database"
        );
        let db = sqlx::PgPool::connect_with(options).await?;
        crate::storage::database::migrations::run_migrations(&db).await?;
        let actor = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO identity_user (id, username, email, status, auth_source) \
             VALUES ($1, $2, $3, 'active', 'local')",
        )
        .bind(actor)
        .bind(format!("cctv-audit-{actor}"))
        .bind(format!("cctv-audit-{actor}@test.local"))
        .execute(&db)
        .await?;

        let recorder = AuditService::new(SharedArc::new(SqlxAuditRepository::new(db.clone())));
        let context = RequestContext::new(
            Some(actor),
            "user".to_string(),
            None,
            "cctv-audit-persistence".to_string(),
            RequestAuth::default(),
        );
        let target = Uuid::now_v7();
        let outcome = DeviceUsersBulkOutcomeResponse::from(BulkOutcome {
            succeeded: vec!["operator".to_string()],
            failed: vec![("reviewer".to_string(), DriverError::Auth)],
        });
        record_cctv_operation(
            &recorder,
            &context,
            "cctv.ptz.commanded",
            "cctv_ptz",
            target,
            CctvAuditDetails::Ptz {
                channel: 4,
                action: "preset",
            },
        )
        .await;
        record_cctv_operation(
            &recorder,
            &context,
            "cctv.recording.exported",
            "get_cctv_export",
            target,
            CctvAuditDetails::Export {
                channel: 7,
                start: "2026-08-11T00:00:00+00:00",
                end: "2026-08-11T00:01:00+00:00",
            },
        )
        .await;
        for (event, operation) in [
            ("device.users.created", "create_device_users"),
            ("device.users.updated", "update_device_users"),
            ("device.users.deleted", "delete_device_users"),
        ] {
            record_cctv_operation(
                &recorder,
                &context,
                event,
                operation,
                target,
                CctvAuditDetails::DeviceUsers { outcome: &outcome },
            )
            .await;
        }

        let rows: Vec<PersistedCctvAuditRow> = sqlx::query_as(
            "SELECT event_type, actor_id, actor_kind, target_type, target_id, metadata \
                 FROM audit_event WHERE target_id = $1 AND target_type = 'cctv_device' ORDER BY id",
        )
        .bind(target)
        .fetch_all(&db)
        .await?;
        assert_eq!(
            rows.len(),
            5,
            "every operation must persist, including with an actor FK"
        );
        for (row, (expected, operation)) in rows.iter().zip([
            ("cctv.ptz.commanded", "cctv_ptz"),
            ("cctv.recording.exported", "get_cctv_export"),
            ("device.users.created", "create_device_users"),
            ("device.users.updated", "update_device_users"),
            ("device.users.deleted", "delete_device_users"),
        ]) {
            assert_eq!(row.event_type, expected);
            assert_eq!(row.actor_id, Some(actor));
            assert_eq!(row.actor_kind, "user");
            assert_eq!(row.target_type.as_deref(), Some("cctv_device"));
            assert_eq!(row.target_id, Some(target));
            assert_eq!(row.metadata["device_id"], target.to_string());
            assert_eq!(row.metadata["auth_method"], "user");
            assert_eq!(row.metadata["correlation_id"], "cctv-audit-persistence");
            assert_eq!(row.metadata["operation"], operation);
            match expected {
                "cctv.ptz.commanded" => {
                    assert_eq!(row.metadata["result"], "success");
                    assert_eq!(row.metadata["channel"], 4);
                    assert_eq!(row.metadata["ptz_action"], "preset");
                }
                "cctv.recording.exported" => {
                    assert_eq!(row.metadata["result"], "success");
                    assert_eq!(row.metadata["channel"], 7);
                    assert_eq!(row.metadata["start"], "2026-08-11T00:00:00+00:00");
                    assert_eq!(row.metadata["end"], "2026-08-11T00:01:00+00:00");
                }
                _ => {
                    assert_eq!(row.metadata["result"], "partial_success");
                    assert_eq!(row.metadata["succeeded_count"], json!(1));
                    assert_eq!(row.metadata["failed_count"], json!(1));
                    assert!(!row.metadata.to_string().contains("operator"));
                }
            }
            let mut scanned = row.metadata.clone();
            scanned
                .as_object_mut()
                .expect("metadata map")
                .remove("auth_method");
            for forbidden in ["password", "secret", "token", "credential"] {
                assert!(!scanned.to_string().to_ascii_lowercase().contains(forbidden));
            }
        }
        sqlx::query("DELETE FROM audit_event WHERE target_id = $1 AND target_type = 'cctv_device'")
            .bind(target)
            .execute(&db)
            .await?;
        sqlx::query("DELETE FROM identity_user WHERE id = $1")
            .bind(actor)
            .execute(&db)
            .await?;
        Ok(())
    }

    fn no_cctv_permissions() -> DeviceCctvPermissionsResponse {
        DeviceCctvPermissionsResponse {
            view: false,
            recordings: false,
            export: false,
            users: false,
            settings: false,
            ptz: false,
        }
    }

    fn export_query(start: &str, end: &str) -> DeviceExportQuery {
        DeviceExportQuery {
            channel: 1,
            start: start.to_owned(),
            end: end.to_owned(),
            transcode: None,
        }
    }

    fn empty_clip(filename: &str) -> ClipStream {
        ClipStream {
            content_type: "video/mp4".to_owned(),
            filename: filename.to_owned(),
            bytes: Box::pin(futures_util::stream::empty()),
        }
    }

    fn utc_timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(value)
            .expect("fixed test timestamp should parse")
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn capabilities_response_preserves_driver_manifest_and_permissions() {
        let permissions = DeviceCctvPermissionsResponse {
            view: true,
            recordings: false,
            export: true,
            users: false,
            settings: true,
            ptz: false,
        };
        let response = DeviceCapabilitiesResponse::new(
            Capabilities {
                info: true,
                users: false,
                disk: true,
                recording: false,
                snapshot: true,
                streaming: false,
                channels: true,
                ptz: false,
                export: true,
                user_update: false,
                authority_catalog: true,
                recording_search: false,
            },
            permissions,
        );

        assert_driver_capabilities(&response);
        assert_cctv_permissions(&response.permissions);
    }

    fn assert_driver_capabilities(response: &DeviceCapabilitiesResponse) {
        assert!(response.info);
        assert!(!response.users);
        assert!(response.disk);
        assert!(!response.recording);
        assert!(response.snapshot);
        assert!(!response.streaming);
        assert!(response.channels);
        assert!(!response.ptz);
        assert!(response.export);
        assert!(!response.user_update);
        assert!(response.authority_catalog);
        assert!(!response.recording_search);
    }

    fn assert_cctv_permissions(permissions: &DeviceCctvPermissionsResponse) {
        assert!(permissions.view);
        assert!(!permissions.recordings);
        assert!(permissions.export);
        assert!(!permissions.users);
        assert!(permissions.settings);
        assert!(!permissions.ptz);
    }

    #[test]
    fn channel_response_preserves_optional_fields_and_ptz_discovery_verdicts() {
        let responses: Vec<DeviceChannelResponse> = [
            Channel {
                index: 1,
                name: Some("gate".to_owned()),
                enabled: Some(true),
                ptz_support: PtzSupport::Supported,
                recording: Some(false),
            },
            Channel {
                index: 2,
                name: None,
                enabled: Some(false),
                ptz_support: PtzSupport::Unsupported,
                recording: Some(true),
            },
            Channel {
                index: u16::MAX,
                name: Some(String::new()),
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
        ]
        .into_iter()
        .map(DeviceChannelResponse::from)
        .collect();

        assert_supported_channel(&responses[0]);
        assert_unsupported_channel(&responses[1]);
        assert_unknown_channel(&responses[2]);
    }

    fn assert_supported_channel(response: &DeviceChannelResponse) {
        assert_eq!(response.index, 1);
        assert_eq!(response.name.as_deref(), Some("gate"));
        assert_eq!(response.enabled, Some(true));
        assert_eq!(response.recording, Some(false));
        assert!(matches!(
            response.ptz_support,
            DevicePtzSupportResponse::Supported
        ));
    }

    fn assert_unsupported_channel(response: &DeviceChannelResponse) {
        assert_eq!(response.index, 2);
        assert!(response.name.is_none());
        assert_eq!(response.enabled, Some(false));
        assert_eq!(response.recording, Some(true));
        assert!(matches!(
            response.ptz_support,
            DevicePtzSupportResponse::Unsupported
        ));
    }

    fn assert_unknown_channel(response: &DeviceChannelResponse) {
        assert_eq!(response.index, u16::MAX);
        assert_eq!(response.name.as_deref(), Some(""));
        assert_eq!(response.enabled, None);
        assert_eq!(response.recording, None);
        assert!(matches!(
            response.ptz_support,
            DevicePtzSupportResponse::Unknown
        ));
    }

    #[test]
    fn identity_and_user_responses_preserve_device_reported_values() {
        let device_time = utc_timestamp("2026-08-09T14:15:16Z");
        let info = DeviceInfoResponse::from(DeviceInfo {
            model: Some(String::new()),
            firmware: None,
            serial: Some("serial-7".to_owned()),
            channels: Some(u16::MAX),
            device_time: Some(device_time),
        });
        let user = DeviceUserResponse::from(UserAccount {
            name: "operator".to_owned(),
            group: None,
            authorities: vec!["Monitor".to_owned(), "Replay".to_owned()],
        });

        assert_eq!(info.model.as_deref(), Some(""));
        assert!(info.firmware.is_none());
        assert_eq!(info.serial.as_deref(), Some("serial-7"));
        assert_eq!(info.channels, Some(u16::MAX));
        assert_eq!(
            info.device_time.as_deref(),
            Some("2026-08-09T14:15:16+00:00")
        );
        assert_eq!(user.name, "operator");
        assert!(user.group.is_none());
        assert_eq!(user.authorities, ["Monitor", "Replay"]);
    }

    #[test]
    fn disk_response_preserves_capacity_bounds_and_every_health_state() {
        let responses: Vec<DeviceDiskEntryResponse> = [
            DiskState::Ok,
            DiskState::ReadOnly,
            DiskState::Error,
            DiskState::Unknown,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, state)| {
            DeviceDiskEntryResponse::from(DiskStatus {
                name: format!("disk-{index}"),
                total_bytes: u64::MAX,
                used_bytes: index as u64,
                state,
                is_error: matches!(state, DiskState::Error),
            })
        })
        .collect();

        assert!(matches!(responses[0].state, DeviceDiskStateResponse::Ok));
        assert!(matches!(
            responses[1].state,
            DeviceDiskStateResponse::ReadOnly
        ));
        assert!(matches!(responses[2].state, DeviceDiskStateResponse::Error));
        assert!(matches!(
            responses[3].state,
            DeviceDiskStateResponse::Unknown
        ));
        for (index, response) in responses.iter().enumerate() {
            assert_eq!(response.name, format!("disk-{index}"));
            assert_eq!(response.total_bytes, u64::MAX);
            assert_eq!(response.used_bytes, index as u64);
            assert_eq!(response.is_error, index == 2);
        }
    }

    #[test]
    fn recording_response_preserves_empty_and_family_attributed_spans() {
        let oldest = utc_timestamp("2026-08-01T00:00:00Z");
        let boundary = utc_timestamp("2026-08-02T00:00:00Z");
        let newest = utc_timestamp("2026-08-03T00:00:00Z");
        let populated = DeviceRecordingResponse::from(RecordingStatus {
            active: true,
            oldest: Some(oldest),
            newest: Some(newest),
            spans: vec![
                RecordSpan {
                    start: oldest,
                    end: boundary,
                    channel: None,
                },
                RecordSpan {
                    start: boundary,
                    end: newest,
                    channel: Some(u16::MAX),
                },
            ],
        });
        let empty = DeviceRecordingResponse::from(RecordingStatus {
            active: false,
            oldest: None,
            newest: None,
            spans: Vec::new(),
        });

        assert!(populated.active);
        assert_eq!(
            populated.oldest.as_deref(),
            Some("2026-08-01T00:00:00+00:00")
        );
        assert_eq!(
            populated.newest.as_deref(),
            Some("2026-08-03T00:00:00+00:00")
        );
        assert_eq!(populated.spans.len(), 2);
        assert_eq!(populated.spans[0].start, "2026-08-01T00:00:00+00:00");
        assert_eq!(populated.spans[0].end, "2026-08-02T00:00:00+00:00");
        assert_eq!(populated.spans[0].channel, None);
        assert_eq!(populated.spans[1].start, "2026-08-02T00:00:00+00:00");
        assert_eq!(populated.spans[1].end, "2026-08-03T00:00:00+00:00");
        assert_eq!(populated.spans[1].channel, Some(u16::MAX));
        assert!(!empty.active);
        assert!(empty.oldest.is_none());
        assert!(empty.newest.is_none());
        assert!(empty.spans.is_empty());
    }

    #[tokio::test]
    async fn jpeg_response_preserves_bytes_and_reports_exact_length() {
        for bytes in [Vec::new(), vec![0xff, 0xd8, 0xff, 0xd9]] {
            let response = jpeg_response(bytes.clone()).expect("the JPEG headers should be valid");
            assert_eq!(response.headers()[CONTENT_TYPE], "image/jpeg");
            assert_eq!(response.headers()[CONTENT_LENGTH], bytes.len().to_string());
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("the JPEG body should be readable");
            assert_eq!(body.as_ref(), bytes.as_slice());
        }
    }

    #[test]
    fn export_window_configuration_rejects_empty_nonpositive_and_overflow_values() {
        for invalid in [
            None,
            Some(""),
            Some("   "),
            Some("0"),
            Some("-1"),
            Some("9223372036854775808"),
            Some("not-a-number"),
        ] {
            assert_eq!(
                configured_export_window_seconds(invalid),
                DEFAULT_MAX_EXPORT_WINDOW_SECONDS
            );
        }
        assert_eq!(configured_export_window_seconds(Some(" 17 ")), 17);
        assert_eq!(
            configured_export_window_seconds(Some("9223372036854775807")),
            i64::MAX
        );
    }

    #[test]
    fn max_export_window_is_stable_during_concurrent_first_reads() {
        let configured = std::env::var("PIPING_CCTV__MAX_EXPORT_WINDOW_SECS").ok();
        let expected = configured_export_window_seconds(configured.as_deref());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(9));
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    max_export_window_seconds()
                })
            })
            .collect();

        barrier.wait();
        for reader in readers {
            assert_eq!(
                reader.join().expect("reader thread should not panic"),
                expected
            );
        }
    }

    #[test]
    fn archive_window_errors_identify_the_malformed_field() {
        for (query, expected_message) in [
            (
                export_query("", "2026-08-09T12:00:00Z"),
                "invalid start timestamp",
            ),
            (
                export_query("2026-08-09T11:00:00Z", "not-a-timestamp"),
                "invalid end timestamp",
            ),
        ] {
            let error = parse_export_request(&query).expect_err("malformed timestamp must fail");
            let AppError::Validation(message) = error else {
                panic!("expected validation error, got {error:?}");
            };
            assert!(
                message.contains(expected_message),
                "unexpected message: {message}"
            );
        }
    }

    #[test]
    fn archive_windows_reject_equal_end_and_one_second_past_each_cap() {
        let start = utc_timestamp("2026-01-01T00:00:00Z");
        let equal = export_query(&start.to_rfc3339(), &start.to_rfc3339());
        assert!(parse_export_request(&equal).is_err());
        assert!(parse_search_request(&equal).is_err());

        let export_end = start + chrono::Duration::seconds(max_export_window_seconds() + 1);
        let export_past_cap = export_query(&start.to_rfc3339(), &export_end.to_rfc3339());
        assert!(parse_export_request(&export_past_cap).is_err());

        let search_end = start + chrono::Duration::seconds(MAX_SEARCH_WINDOW_SECONDS + 1);
        let search_past_cap = export_query(&start.to_rfc3339(), &search_end.to_rfc3339());
        assert!(parse_search_request(&search_past_cap).is_err());
    }

    #[test]
    fn playback_encoding_requires_an_explicit_true_request() {
        assert_eq!(playback_encoding(None), ClipEncoding::Copy);
        assert_eq!(playback_encoding(Some(false)), ClipEncoding::Copy);
        assert_eq!(
            playback_encoding(Some(true)),
            ClipEncoding::BrowserCompatible
        );
    }

    #[test]
    fn clip_response_rejects_a_malformed_device_content_type() {
        let clip = ClipStream {
            content_type: "video/mp4\r\nx-device-secret: exposed".to_owned(),
            filename: "clip.mp4".to_owned(),
            bytes: Box::pin(futures_util::stream::empty()),
        };

        assert!(matches!(
            clip_response(clip, ClipDelivery::Watch),
            Err(AppError::External(_))
        ));
    }

    #[test]
    fn empty_clip_filename_still_forms_a_valid_header() {
        let response = clip_response(empty_clip(""), ClipDelivery::Download)
            .expect("fallback filename should form a valid header");
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_DISPOSITION],
            "attachment; filename=\"clip.bin\""
        );
    }

    #[test]
    fn capability_gate_accepts_every_individual_cctv_permission() {
        let none = no_cctv_permissions();
        assert!(!none.has_any());

        let individual_grants = [
            DeviceCctvPermissionsResponse { view: true, ..none },
            DeviceCctvPermissionsResponse {
                recordings: true,
                ..none
            },
            DeviceCctvPermissionsResponse {
                export: true,
                ..none
            },
            DeviceCctvPermissionsResponse {
                users: true,
                ..none
            },
            DeviceCctvPermissionsResponse {
                settings: true,
                ..none
            },
            DeviceCctvPermissionsResponse { ptz: true, ..none },
        ];

        assert!(individual_grants.into_iter().all(|grant| grant.has_any()));
    }

    #[test]
    fn ptz_request_selects_command_and_normalizes_move_velocities() {
        let move_request = PtzCommandRequest {
            kind: PtzKind::Move,
            channel: 4,
            pan: -4.0,
            tilt: 0.25,
            zoom: f32::NAN,
            preset: 23,
        };
        assert_eq!(
            move_request.to_command(),
            PtzCommand::Move(PtzVector {
                pan: -1.0,
                tilt: 0.25,
                zoom: 0.0,
            })
        );

        let stop_request = PtzCommandRequest {
            kind: PtzKind::Stop,
            ..move_request
        };
        assert_eq!(stop_request.to_command(), PtzCommand::Stop);

        let preset_request = PtzCommandRequest {
            kind: PtzKind::Preset,
            ..move_request
        };
        assert_eq!(preset_request.to_command(), PtzCommand::Preset { id: 23 });
    }

    #[test]
    fn export_request_accepts_exact_limit_and_normalizes_offset() {
        let expected_start = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed start timestamp should parse")
            .with_timezone(&chrono::Utc);
        let expected_end = expected_start + chrono::Duration::seconds(max_export_window_seconds());
        let query = DeviceExportQuery {
            channel: 7,
            start: " 2026-01-01T03:00:00+03:00 ".to_owned(),
            end: expected_end.to_rfc3339(),
            transcode: None,
        };

        let request = parse_export_request(&query)
            .expect("the exact export limit must remain an accepted boundary");
        assert_eq!(request.channel, 7);
        assert_eq!(request.start, expected_start);
        assert_eq!(request.end, expected_end);
    }

    #[test]
    fn search_request_accepts_exact_limit() {
        let start = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed start timestamp should parse")
            .with_timezone(&chrono::Utc);
        let end = start + chrono::Duration::seconds(MAX_SEARCH_WINDOW_SECONDS);
        let query = export_query(&start.to_rfc3339(), &end.to_rfc3339());

        let request = parse_search_request(&query)
            .expect("the exact search limit must remain an accepted boundary");
        assert_eq!(request.start, start);
        assert_eq!(request.end, end);
    }

    #[test]
    fn clip_delivery_headers_are_safe_and_distinguish_watch_from_download() {
        let unsafe_filename = "../../incident\r\n7.mp4";

        for (delivery, expected_disposition) in [
            (
                ClipDelivery::Watch,
                "inline; filename=\".._.._incident__7.mp4\"",
            ),
            (
                ClipDelivery::Download,
                "attachment; filename=\".._.._incident__7.mp4\"",
            ),
        ] {
            let response = clip_response(empty_clip(unsafe_filename), delivery)
                .expect("sanitized clip metadata should produce valid headers");
            assert_eq!(response.headers()[CONTENT_TYPE], "video/mp4");
            assert_eq!(
                response.headers()[axum::http::header::CONTENT_DISPOSITION],
                expected_disposition
            );
            assert!(response.headers().get(CONTENT_LENGTH).is_none());
        }
    }

    #[tokio::test]
    async fn live_capacity_exhaustion_maps_to_retryable_safe_response() {
        let response = map_service_error(DeviceServiceError::LiveBusy);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the CCTV error body should be readable");
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).expect("the CCTV error body should be JSON");
        assert_eq!(payload["reason_code"], reason_codes::CONNECTION_BUSY);
        assert_eq!(payload["code"], reason_codes::CONNECTION_FAILED);
        assert_eq!(payload["message"], CONNECTION_FAILURE_MESSAGE);
    }

    /// A search moves no media bytes and opens no device session, so its
    /// window cap must be far wider than export/playback's — an archive
    /// timeline calling it with a whole month must not be refused the way an
    /// export request pinning a media slot correctly is.
    #[test]
    fn search_request_accepts_a_window_export_would_reject() {
        let query = export_query("2026-01-01T00:00:00Z", "2026-01-15T00:00:00Z");

        assert!(parse_export_request(&query).is_err());
        assert!(parse_search_request(&query).is_ok());
    }

    #[test]
    fn search_request_still_rejects_an_inverted_window() {
        let query = export_query("2026-07-25T05:00:00Z", "2026-07-25T04:00:00Z");
        assert!(parse_search_request(&query).is_err());
    }

    #[test]
    fn search_request_rejects_a_window_past_its_own_cap() {
        let query = export_query("2020-01-01T00:00:00Z", "2026-01-01T00:00:00Z");
        assert!(parse_search_request(&query).is_err());
    }

    #[test]
    fn missing_target_is_not_found() {
        let response = map_service_error(DeviceServiceError::Resolve(
            DeviceResolveError::NotFound(Uuid::nil()),
        ));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn configuration_gaps_are_bad_requests() {
        for resolve in [
            DeviceResolveError::UnknownFamily(Uuid::nil()),
            DeviceResolveError::NoAddress(Uuid::nil()),
            DeviceResolveError::NoCredentials(Uuid::nil()),
        ] {
            assert_eq!(
                map_service_error(DeviceServiceError::Resolve(resolve)).status(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[test]
    fn resolve_internal_is_bad_gateway() {
        let response = map_service_error(DeviceServiceError::Resolve(
            DeviceResolveError::Internal("boom".to_string()),
        ));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn unsupported_and_auth_are_bad_requests() {
        assert_eq!(
            map_service_error(DeviceServiceError::Driver(DriverError::Unsupported)).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            map_service_error(DeviceServiceError::Driver(DriverError::Auth)).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn upstream_device_faults_are_bad_gateway_not_internal() {
        for driver in [
            DriverError::Transport("device read failed".to_string()),
            DriverError::Unreachable("connect failed".to_string()),
            DriverError::ClockSkew,
            DriverError::Protocol("short frame".to_string()),
            DriverError::Device("busy".to_string()),
        ] {
            assert_eq!(
                map_service_error(DeviceServiceError::Driver(driver)).status(),
                StatusCode::BAD_GATEWAY,
                "device-side driver faults must map to 502"
            );
        }
    }

    #[tokio::test]
    async fn typed_device_errors_map_to_exact_safe_connection_envelopes() {
        let cases = [
            (
                DriverError::Auth,
                StatusCode::BAD_REQUEST,
                reason_codes::CONNECTION_AUTHENTICATION_FAILED,
            ),
            (
                DriverError::Timeout("secret timeout detail".to_owned()),
                StatusCode::GATEWAY_TIMEOUT,
                reason_codes::CONNECTION_TIMEOUT,
            ),
            (
                DriverError::Unreachable("secret unreachable detail".to_owned()),
                StatusCode::BAD_GATEWAY,
                reason_codes::CONNECTION_UNREACHABLE,
            ),
            (
                DriverError::Busy("secret capacity detail".to_owned()),
                StatusCode::TOO_MANY_REQUESTS,
                reason_codes::CONNECTION_BUSY,
            ),
            (
                DriverError::Unsupported,
                StatusCode::BAD_REQUEST,
                reason_codes::CONNECTION_UNSUPPORTED,
            ),
            (
                DriverError::Device("secret upstream detail".to_owned()),
                StatusCode::BAD_GATEWAY,
                reason_codes::CONNECTION_FAILED,
            ),
        ];

        for (driver_error, expected_status, expected_reason_code) in cases {
            let response = map_service_error(DeviceServiceError::Driver(driver_error));
            assert_eq!(response.status(), expected_status);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("the CCTV error body should be readable");
            let payload: serde_json::Value =
                serde_json::from_slice(&bytes).expect("the CCTV error body should be JSON");
            assert_eq!(payload["reason_code"], expected_reason_code);
            assert_eq!(payload["code"], "CONNECTION_FAILED");
            assert_eq!(payload["message"], "Unable to connect to the device");
            assert!(payload.get("trace_id").is_none());
            let serialized =
                String::from_utf8(bytes.to_vec()).expect("the JSON payload should be UTF-8");
            assert!(!serialized.contains("secret"));
        }
    }

    #[tokio::test]
    async fn missing_and_rejected_credentials_have_distinct_reason_codes() {
        let missing = map_service_error(DeviceServiceError::Resolve(
            DeviceResolveError::NoCredentials(Uuid::nil()),
        ));
        let rejected = map_service_error(DeviceServiceError::Driver(DriverError::Auth));

        for (response, expected_reason_code) in [
            (missing, reason_codes::CONNECTION_CREDENTIALS_REQUIRED),
            (rejected, reason_codes::CONNECTION_AUTHENTICATION_FAILED),
        ] {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("the CCTV error body should be readable");
            let payload: serde_json::Value =
                serde_json::from_slice(&bytes).expect("the CCTV error body should be JSON");
            assert_eq!(payload["reason_code"], expected_reason_code);
        }
    }

    #[test]
    fn blank_target_id_is_rejected() {
        assert!(matches!(
            parse_target_id("   "),
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            parse_target_id("not-a-uuid"),
            Err(AppError::Validation(_))
        ));
        assert!(parse_target_id(&Uuid::nil().to_string()).is_ok());
    }

    #[test]
    fn invalid_write_request_maps_to_safe_unsupported_response() {
        let response =
            map_service_error(DeviceServiceError::InvalidRequest("blank name".to_owned()));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn new_user_request_maps_password_into_secret() {
        let user = NewUser::from(DeviceNewUserRequest {
            name: "guard1".to_string(),
            password: Some("s3cr3t".to_string()),
            group: Some("user".to_string()),
            authorities: vec!["Monitor".to_string()],
        });
        assert_eq!(user.name, "guard1");
        assert_eq!(user.password.expose(), "s3cr3t");
        assert_eq!(user.group.as_deref(), Some("user"));
        assert_eq!(user.authorities, ["Monitor"]);
    }

    #[test]
    fn new_user_request_defaults_missing_password_to_empty() {
        let user = NewUser::from(DeviceNewUserRequest {
            name: "guard1".to_string(),
            password: None,
            group: None,
            authorities: Vec::new(),
        });
        assert_eq!(user.password.expose(), "");
    }

    #[test]
    fn user_update_request_maps_supplied_facets_into_secret_backed_domain() {
        let update = UserUpdate::from(UserUpdateRequest {
            name: "guard1".to_string(),
            password: Some("new-s3cr3t".to_string()),
            group: Some("admin".to_string()),
            authorities: Some(vec!["Monitor".to_string()]),
        });
        assert_eq!(update.name, "guard1");
        assert_eq!(
            update.password.as_ref().map(Secret::expose),
            Some("new-s3cr3t")
        );
        assert_eq!(update.group.as_deref(), Some("admin"));
        assert_eq!(
            update.authorities.as_deref(),
            Some(["Monitor".to_string()].as_slice())
        );
        assert!(update.has_changes());
    }

    #[test]
    fn user_update_request_leaves_omitted_facets_as_none() {
        // Only the password is supplied; group/authorities stay untouched.
        let update = UserUpdate::from(UserUpdateRequest {
            name: "guard1".to_string(),
            password: Some("reset".to_string()),
            group: None,
            authorities: None,
        });
        assert!(update.password.is_some());
        assert!(update.group.is_none());
        assert!(update.authorities.is_none());
        assert!(update.has_changes());

        // An identity-only request carries no change.
        let vacuous = UserUpdate::from(UserUpdateRequest {
            name: "guard1".to_string(),
            password: None,
            group: None,
            authorities: None,
        });
        assert!(!vacuous.has_changes());
    }

    #[test]
    fn update_request_dto_does_not_serialize_a_password() {
        // `UserUpdateRequest` is write-only: it must not derive Serialize, so the
        // permissions verdict is the only place `user_update` surfaces. Assert the
        // verdict struct carries the new flag and never a password.
        let permissions = DeviceCctvPermissionsResponse {
            view: true,
            recordings: true,
            export: false,
            users: true,
            settings: false,
            ptz: true,
        };
        let json = serde_json::to_string(&permissions).expect("serialize permissions");
        assert!(json.contains("\"users\":true"));
        assert!(
            !json.contains("password"),
            "permissions must not carry a password field: {json}"
        );
    }

    #[test]
    fn preview_request_builds_create_write_op_from_users() {
        let op = WriteOp::from(PreviewDeviceUsersRequest {
            op: DeviceUserWriteOpKind::Create,
            users: vec![DeviceNewUserRequest {
                name: "fresh".to_string(),
                password: None,
                group: None,
                authorities: Vec::new(),
            }],
            // The unused collection for this discriminant is ignored.
            usernames: vec!["ignored".to_string()],
        });
        match op {
            WriteOp::CreateUsers(users) => {
                assert_eq!(users.len(), 1);
                assert_eq!(users[0].name, "fresh");
            }
            other => panic!("expected CreateUsers, got {other:?}"),
        }
    }

    #[test]
    fn preview_request_builds_delete_write_op_from_usernames() {
        let op = WriteOp::from(PreviewDeviceUsersRequest {
            op: DeviceUserWriteOpKind::Delete,
            users: Vec::new(),
            usernames: vec!["present".to_string(), "absent".to_string()],
        });
        match op {
            WriteOp::DeleteUsers(names) => assert_eq!(names, ["present", "absent"]),
            other => panic!("expected DeleteUsers, got {other:?}"),
        }
    }

    #[test]
    fn preview_response_mirrors_domain_verdicts() {
        let response = DeviceUsersPreviewResponse::new(
            WritePreview {
                per_item: vec![
                    PreviewItem {
                        name: "a".to_string(),
                        action: PreviewAction::Create,
                        reason: "new".to_string(),
                    },
                    PreviewItem {
                        name: "b".to_string(),
                        action: PreviewAction::Skip,
                        reason: "exists".to_string(),
                    },
                ],
            },
            true,
        );
        assert!(response.writes_enabled);
        assert_eq!(response.per_item.len(), 2);
        assert!(matches!(
            response.per_item[0].action,
            DeviceUserPreviewActionResponse::Create
        ));
        assert!(matches!(
            response.per_item[1].action,
            DeviceUserPreviewActionResponse::Skip
        ));
    }

    #[test]
    fn bulk_outcome_response_reports_partial_failure() {
        let response = DeviceUsersBulkOutcomeResponse::from(BulkOutcome {
            succeeded: vec!["a".to_string()],
            failed: vec![("b".to_string(), DriverError::Unsupported)],
        });
        assert!(!response.all_succeeded);
        assert_eq!(response.succeeded, ["a"]);
        assert_eq!(response.failed.len(), 1);
        assert_eq!(response.failed[0].name, "b");
        assert_eq!(
            response.failed[0].reason_code,
            ConnectionReasonCode::ConnectionUnsupported
        );
        assert_eq!(
            response.failed[0].message,
            "Unable to connect to the device"
        );
    }

    #[tokio::test]
    async fn zero_success_device_user_outcome_audits_result_failed() {
        // A DVRIP batch whose session died before its first acknowledgement is
        // still a per-item outcome: the handler answers 200 and emits a `failed`
        // event (counts only), not a 502 with no event.
        let recorder = CctvAuditTestRecorder::default();
        let target_id = Uuid::now_v7();
        let context = RequestContext::new(
            Some(Uuid::now_v7()),
            "api_token".to_string(),
            None,
            "cctv-zero-success".to_string(),
            RequestAuth::default(),
        );
        let outcome = DeviceUsersBulkOutcomeResponse::from(BulkOutcome {
            succeeded: Vec::new(),
            failed: vec![
                (
                    "guard1".to_string(),
                    DriverError::Unreachable("link down".to_string()),
                ),
                (
                    "guard2".to_string(),
                    DriverError::Unreachable("not attempted after device link failure".to_string()),
                ),
            ],
        });
        assert!(!outcome.all_succeeded);
        assert!(outcome.succeeded.is_empty());

        record_cctv_operation(
            &recorder,
            &context,
            "device.users.created",
            "create_device_users",
            target_id,
            CctvAuditDetails::DeviceUsers { outcome: &outcome },
        )
        .await;

        let events = recorder.events.lock().expect("test recorder mutex");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "device.users.created");
        assert_eq!(events[0].target_id, Some(target_id));
        assert_eq!(events[0].metadata["result"], "failed");
        assert_eq!(events[0].metadata["succeeded_count"], 0);
        assert_eq!(events[0].metadata["failed_count"], 2);
        let serialized = events[0].metadata.to_string();
        for forbidden in ["guard1", "guard2", "link down"] {
            assert!(
                !serialized.contains(forbidden),
                "audit metadata must be count-only: {serialized}"
            );
        }
    }

    #[test]
    fn write_responses_never_serialize_a_password() {
        let outcome = DeviceUsersBulkOutcomeResponse::from(BulkOutcome {
            succeeded: vec!["guard1".to_string()],
            failed: vec![("guard2".to_string(), DriverError::Auth)],
        });
        let json = serde_json::to_string(&outcome).expect("serialize bulk outcome");
        assert!(
            !json.contains("password"),
            "bulk outcome must not carry a password field: {json}"
        );

        let preview = DeviceUsersPreviewResponse::new(
            WritePreview {
                per_item: vec![PreviewItem {
                    name: "guard1".to_string(),
                    action: PreviewAction::Create,
                    reason: "new".to_string(),
                }],
            },
            false,
        );
        let json = serde_json::to_string(&preview).expect("serialize preview");
        assert!(
            !json.contains("password"),
            "preview must not carry a password field: {json}"
        );
    }
}
