//! Fleet (multi-target) user-management REST surface for the device plane.
//!
//! Three POST endpoints fan one user-management request — preview / create /
//! delete — across many connection targets at once. They mirror the
//! single-device write endpoints in [`super::cctv`], but instead of a target
//! id in the path they take a `target_ids` list in the body and return a
//! **per-target** partition of the outcome.
//!
//! # Capability- and RBAC-aware partitioning
//!
//! A fleet request never fails the whole batch on one target. For each selected
//! target the handler runs the SAME per-target gate the single-device path uses
//! (`super::cctv::ensure_target_scoped_access`) — `device_users:connection_target`
//! for all three operations — but **captures** allow/deny per target instead of
//! returning
//! `403` on the first deny: denied targets are excluded from the device fan-out
//! and reported as
//! [`not_permitted`](FleetWriteTargetOutcomeResponse::NotPermitted).
//! The permitted subset then runs through [`FleetWriteService`], whose own
//! structural/transport partitioning surfaces as `done` / `ineligible` /
//! `unreachable`. The operator therefore sees a single response that reads
//! "42 selected → 31 eligible, 8 unsupported, 3 not permitted".
//!
//! # Secrets
//!
//! `password` is write-only: it is accepted on the request DTOs but never echoed
//! — none of the response DTOs derive `Serialize` with a password field.

use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::cctv::{
    CctvAuditDetails, DeviceNewUserRequest, DeviceUserWriteOpKind, DeviceUsersBulkOutcomeResponse,
    DeviceUsersPreviewResponse, PreviewDeviceUsersRequest, ensure_target_scoped_access,
    record_cctv_operation,
};
use crate::api::rest::routes::validation::require_bulk_items;
use crate::app::AppState;
use crate::contexts::access_control::domain::Action;
use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::device_management::application::{
    FleetPreview, FleetPreviewOutcome, FleetTargetOutcome, FleetWriteResult, FleetWriteService,
    WriteOp,
};
use crate::contexts::device_management::domain::NewUser;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use crate::api::access_control_helpers::build_device_management_service;

/// Defensive filler used only if the fleet service returned fewer per-target
/// entries than there were permitted targets — an invariant violation of
/// `FleetWriteService` (each permitted target yields exactly one entry, in
/// order) that must degrade to a reason string rather than panic a response.
const MISSING_RESULT_REASON: &str = "no fleet result was produced for this target";

/// Dry-run request: preview one user-management change across many targets.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct PreviewFleetUsersRequest {
    /// Connection targets to preview against.
    #[serde(deserialize_with = "deserialize_unique_target_ids")]
    #[schema(min_items = 1)]
    pub target_ids: Vec<Uuid>,
    /// Whether the previewed write would create or delete accounts.
    pub op: DeviceUserWriteOpKind,
    /// Accounts to provision (used when `op` is `create`).
    #[serde(default)]
    pub users: Vec<DeviceNewUserRequest>,
    /// Login names to remove (used when `op` is `delete`).
    #[serde(default)]
    pub usernames: Vec<String>,
}

/// Provision request: create the same accounts across many targets.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateFleetUsersRequest {
    /// Connection targets to provision on.
    #[serde(deserialize_with = "deserialize_unique_target_ids")]
    #[schema(min_items = 1)]
    pub target_ids: Vec<Uuid>,
    /// Accounts to provision on every permitted, eligible target.
    #[schema(min_items = 1)]
    pub users: Vec<DeviceNewUserRequest>,
}

/// Removal request: delete the same accounts (by login name) across many targets.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct DeleteFleetUsersRequest {
    /// Connection targets to remove accounts from.
    #[serde(deserialize_with = "deserialize_unique_target_ids")]
    #[schema(min_items = 1)]
    pub target_ids: Vec<Uuid>,
    /// Login names to remove from every permitted, eligible target.
    #[schema(min_items = 1)]
    pub usernames: Vec<String>,
}

impl CreateFleetUsersRequest {
    /// A fleet create must name at least one target and at least one account:
    /// an empty batch is a 400, never a no-op `200`.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.target_ids)?;
        require_bulk_items(&self.users)?;
        Ok(())
    }
}

impl DeleteFleetUsersRequest {
    /// A fleet delete must name at least one target and at least one login.
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.target_ids)?;
        require_bulk_items(&self.usernames)?;
        Ok(())
    }
}

impl PreviewFleetUsersRequest {
    /// A fleet preview must name at least one target and the collection the
    /// selected `op` consumes (`users` for create, `usernames` for delete).
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require_bulk_items(&self.target_ids)?;
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

/// One target's verdict in a fleet **preview** (read-only).
///
/// Externally tagged (`{"preview": {…}}`, `{"ineligible": {"reason": …}}`, or
/// the bare string `"not_permitted"`) — the same representation the underlying
/// preview outcome uses, plus the RBAC-only `not_permitted` case the service is
/// auth-agnostic about.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FleetPreviewTargetOutcomeResponse {
    /// Eligible; the per-name dry-run verdicts for this target.
    Preview(DeviceUsersPreviewResponse),
    /// Structurally excluded (no user-management plane, did not resolve, or a
    /// structurally invalid request) — the device was never contacted.
    Ineligible {
        /// A short, language-neutral explanation.
        reason: String,
    },
    /// The caller lacks `device_users:connection_target` on this target; it was excluded from the run.
    NotPermitted,
}

impl FleetPreviewTargetOutcomeResponse {
    /// Map a service preview outcome, pairing an eligible preview with the
    /// caller's per-target RBAC write verdict.
    fn from_outcome(outcome: FleetPreviewOutcome, writes_enabled: bool) -> Self {
        match outcome {
            FleetPreviewOutcome::Preview(preview) => {
                Self::Preview(DeviceUsersPreviewResponse::new(preview, writes_enabled))
            }
            FleetPreviewOutcome::Ineligible { reason } => Self::Ineligible { reason },
        }
    }
}

/// One `(target, verdict)` pair in a fleet preview response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FleetPreviewTargetResponse {
    /// The connection target this verdict is for.
    pub target_id: Uuid,
    /// What the preview determined for it.
    pub outcome: FleetPreviewTargetOutcomeResponse,
}

/// The result of a fleet preview across every requested target.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FleetPreviewResponse {
    /// One entry per requested target, in request order. Each eligible entry's
    /// preview carries the caller's per-target write verdict.
    pub per_target: Vec<FleetPreviewTargetResponse>,
}

/// One target's outcome in a fleet **write** (create / delete).
///
/// Externally tagged: `{"done": {…}}` (with a per-account
/// `DeviceUsersBulkOutcomeResponse` preserving partial success),
/// `{"ineligible": {"reason": …}}`, the bare string `"not_permitted"`, or
/// `{"unreachable": {"reason": …}}`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FleetWriteTargetOutcomeResponse {
    /// The write reached the device and ran; partial success is preserved.
    Done(DeviceUsersBulkOutcomeResponse),
    /// Structurally excluded (did not resolve, no user-management plane, or a
    /// structurally invalid request) — the device was never mutated.
    Ineligible {
        /// A short, language-neutral explanation.
        reason: String,
    },
    /// The caller lacks the `device_users:connection_target` permission on this
    /// target; it was excluded from the run.
    NotPermitted,
    /// Eligible and reached, but the device could not be driven to completion
    /// (transport failure or a device-reported error).
    Unreachable {
        /// A short, language-neutral explanation.
        reason: String,
    },
}

impl From<FleetTargetOutcome> for FleetWriteTargetOutcomeResponse {
    fn from(outcome: FleetTargetOutcome) -> Self {
        match outcome {
            FleetTargetOutcome::Done(bulk) => {
                Self::Done(DeviceUsersBulkOutcomeResponse::from(bulk))
            }
            FleetTargetOutcome::Ineligible { reason } => Self::Ineligible { reason },
            FleetTargetOutcome::NotPermitted => Self::NotPermitted,
            FleetTargetOutcome::Unreachable { reason } => Self::Unreachable { reason },
        }
    }
}

/// One `(target, outcome)` pair in a fleet write response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FleetWriteTargetResponse {
    /// The connection target this outcome is for.
    pub target_id: Uuid,
    /// What happened to it.
    pub outcome: FleetWriteTargetOutcomeResponse,
}

/// The result of a fleet write across every requested target.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FleetWriteResultResponse {
    /// One entry per requested target, in request order.
    pub per_target: Vec<FleetWriteTargetResponse>,
}

/// Normalize the request at its JSON boundary so a target can reach RBAC and
/// device fan-out at most once. `HashSet` is only membership state: collecting
/// from the input iterator preserves the first-seen request order.
fn deserialize_unique_target_ids<'de, D>(deserializer: D) -> Result<Vec<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let target_ids = Vec::<Uuid>::deserialize(deserializer)?;
    let mut seen = HashSet::with_capacity(target_ids.len());
    Ok(target_ids
        .into_iter()
        .filter(|target_id| seen.insert(*target_id))
        .collect())
}

/// Dry-run a user-management change across many connection targets.
///
/// A read-only dry run across the fleet: each selected target is gated on
/// `device_users:connection_target` (targets the caller cannot manage are marked `not_permitted`,
/// NOT a hard `403`), then the permitted subset is previewed without mutating
/// any device. Each eligible target's preview carries `writes_enabled = true`;
/// the same permission gates both preview and apply.
#[utoipa::path(
    post,
    path = "/api/v1/device-fleet/users:preview",
    tag = "devices",
    operation_id = "fleet_preview_device_users",
    request_body = PreviewFleetUsersRequest,
    responses(
        (status = 200, description = "Per-target dry-run preview across the fleet", body = FleetPreviewResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn fleet_preview_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(body): EnvelopeJson<PreviewFleetUsersRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let partition = match partition_targets(
        &state,
        &request_context,
        &body.target_ids,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(partition) => partition,
        Err(response) => return *response,
    };
    let write_verdicts = vec![true; partition.permitted_ids.len()];
    let op = WriteOp::from(PreviewDeviceUsersRequest {
        op: body.op,
        users: body.users,
        usernames: body.usernames,
    });
    let service = FleetWriteService::new(build_device_management_service(&state));
    let preview = service.fleet_preview(&partition.permitted_ids, op).await;
    let response = build_preview_response(
        &body.target_ids,
        &partition.permitted_flags,
        preview,
        &write_verdicts,
    );
    (StatusCode::OK, Json(response)).into_response()
}

/// Provision the same user accounts across many connection targets.
///
/// A device write across the fleet: each selected target is gated on
/// `device_users:connection_target` (targets the caller lacks the permission on are marked
/// `not_permitted` and excluded, NOT a hard `403`).
#[utoipa::path(
    post,
    path = "/api/v1/device-fleet/users:create",
    tag = "devices",
    operation_id = "fleet_create_device_users",
    request_body = CreateFleetUsersRequest,
    responses(
        (status = 200, description = "Per-target provisioning outcome across the fleet", body = FleetWriteResultResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn fleet_create_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(body): EnvelopeJson<CreateFleetUsersRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let partition = match partition_targets(
        &state,
        &request_context,
        &body.target_ids,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(partition) => partition,
        Err(response) => return *response,
    };
    let users: Vec<NewUser> = body.users.into_iter().map(NewUser::from).collect();
    let service = FleetWriteService::new(build_device_management_service(&state));
    let result = service
        .fleet_create_users(&partition.permitted_ids, &users)
        .await;
    let response = build_write_response(&body.target_ids, &partition.permitted_flags, result);
    audit_fleet_write(
        &state.services.audit,
        &request_context,
        &response,
        "device.users.created",
        "fleet_create_users",
    )
    .await;
    (StatusCode::OK, Json(response)).into_response()
}

/// Remove the same user accounts (by login name) across many connection targets.
///
/// A device write with the same per-target partitioning as the create endpoint,
/// gated on `device_users:connection_target`.
#[utoipa::path(
    post,
    path = "/api/v1/device-fleet/users:delete",
    tag = "devices",
    operation_id = "fleet_delete_device_users",
    request_body = DeleteFleetUsersRequest,
    responses(
        (status = 200, description = "Per-target removal outcome across the fleet", body = FleetWriteResultResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn fleet_delete_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(body): EnvelopeJson<DeleteFleetUsersRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = body.validate() {
        return handle_app_error(error);
    }
    let partition = match partition_targets(
        &state,
        &request_context,
        &body.target_ids,
        Action::DeviceUsers,
    )
    .await
    {
        Ok(partition) => partition,
        Err(response) => return *response,
    };
    let service = FleetWriteService::new(build_device_management_service(&state));
    let result = service
        .fleet_delete_users(&partition.permitted_ids, &body.usernames)
        .await;
    let response = build_write_response(&body.target_ids, &partition.permitted_flags, result);
    audit_fleet_write(
        &state.services.audit,
        &request_context,
        &response,
        "device.users.deleted",
        "fleet_delete_users",
    )
    .await;
    (StatusCode::OK, Json(response)).into_response()
}

/// Emit one canonical CCTV event per target with at least one successful account
/// write. The response already contains the authorization partition; no denied,
/// unreachable or wholly failed target can be mistaken for a committed write.
async fn audit_fleet_write<R: AuditRecorder + ?Sized>(
    recorder: &R,
    request_context: &RequestContext,
    response: &FleetWriteResultResponse,
    event_type: &'static str,
    operation: &'static str,
) {
    for entry in &response.per_target {
        if let FleetWriteTargetOutcomeResponse::Done(outcome) = &entry.outcome
            && !outcome.succeeded.is_empty()
        {
            record_cctv_operation(
                recorder,
                request_context,
                event_type,
                operation,
                entry.target_id,
                CctvAuditDetails::FleetUsers {
                    succeeded_count: outcome.succeeded.len(),
                    failed_count: outcome.failed.len(),
                },
            )
            .await;
        }
    }
}

/// The per-target access partition: which requested targets the caller may act
/// on, and the permitted subset handed to the fleet service.
struct TargetPartition {
    /// One flag per requested target, in request order: `true` = permitted.
    permitted_flags: Vec<bool>,
    /// The permitted targets, in request order — exactly the slice the fleet
    /// service is invoked with.
    permitted_ids: Vec<Uuid>,
}

/// Run the single-device connection-target gate once per requested target and
/// split them into permitted vs denied, preserving request order.
///
/// A permission `deny` (token-scope or policy) is captured as "not permitted"
/// for that target — it does not fail the request. Any OTHER error (a genuine
/// infrastructure fault while resolving the target's authorization attributes)
/// is not an authorization decision and IS propagated as a hard error, so it is
/// never silently mislabeled `not_permitted`.
async fn partition_targets(
    state: &AppState,
    request_context: &RequestContext,
    target_ids: &[Uuid],
    action: Action,
) -> Result<TargetPartition, Box<Response>> {
    let mut permitted_flags = Vec::with_capacity(target_ids.len());
    let mut permitted_ids = Vec::new();
    for &target_id in target_ids {
        // `Action` is `Clone` (fieldless), so each per-target gate gets its own
        // copy; the decision shape is identical per target.
        match ensure_target_scoped_access(state, request_context, target_id, action.clone()).await {
            Ok(()) => {
                permitted_flags.push(true);
                permitted_ids.push(target_id);
            }
            Err(AppError::Forbidden(_)) => permitted_flags.push(false),
            Err(other) => return Err(Box::new(handle_app_error(other))),
        }
    }
    Ok(TargetPartition {
        permitted_flags,
        permitted_ids,
    })
}

/// Weave the fleet service's per-permitted-target preview outcomes back into the
/// full request-ordered target list, injecting `not_permitted` for denied ones.
///
/// The service returns one entry per permitted target, in the same order it was
/// handed the permitted subset; walking `permitted_flags` in request order and
/// consuming that stream on each permitted position restores the normalized
/// first-seen request order without a lookup.
fn build_preview_response(
    target_ids: &[Uuid],
    permitted_flags: &[bool],
    preview: FleetPreview,
    write_verdicts: &[bool],
) -> FleetPreviewResponse {
    let mut per_target = Vec::with_capacity(target_ids.len());
    let mut service_results = preview.per_target.into_iter();
    // Both streams are in permitted order — one entry per permitted target.
    let mut verdicts = write_verdicts.iter().copied();
    for (&target_id, &permitted) in target_ids.iter().zip(permitted_flags) {
        let outcome = if permitted {
            let writes_enabled = verdicts.next().unwrap_or(false);
            match service_results.next() {
                Some((_id, outcome)) => {
                    FleetPreviewTargetOutcomeResponse::from_outcome(outcome, writes_enabled)
                }
                None => FleetPreviewTargetOutcomeResponse::Ineligible {
                    reason: MISSING_RESULT_REASON.to_owned(),
                },
            }
        } else {
            FleetPreviewTargetOutcomeResponse::NotPermitted
        };
        per_target.push(FleetPreviewTargetResponse { target_id, outcome });
    }
    FleetPreviewResponse { per_target }
}

/// Weave the fleet service's per-permitted-target write outcomes back into the
/// full request-ordered target list, injecting `not_permitted` for denied ones.
/// Same ordering contract as [`build_preview_response`].
fn build_write_response(
    target_ids: &[Uuid],
    permitted_flags: &[bool],
    result: FleetWriteResult,
) -> FleetWriteResultResponse {
    let mut per_target = Vec::with_capacity(target_ids.len());
    let mut service_results = result.per_target.into_iter();
    for (&target_id, &permitted) in target_ids.iter().zip(permitted_flags) {
        let outcome = if permitted {
            match service_results.next() {
                Some(entry) => FleetWriteTargetOutcomeResponse::from(entry.outcome),
                None => FleetWriteTargetOutcomeResponse::Ineligible {
                    reason: MISSING_RESULT_REASON.to_owned(),
                },
            }
        } else {
            FleetWriteTargetOutcomeResponse::NotPermitted
        };
        per_target.push(FleetWriteTargetResponse { target_id, outcome });
    }
    FleetWriteResultResponse { per_target }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::audit::domain::AuditEvent;
    use crate::contexts::device_management::application::{
        FleetTarget, PreviewAction, PreviewItem, WritePreview,
    };
    use crate::contexts::device_management::domain::{BulkOutcome, DriverError};
    use crate::contexts::shared::request_context::RequestAuth;
    use std::sync::Mutex;

    fn bulk_outcome() -> BulkOutcome {
        BulkOutcome {
            succeeded: vec!["guard1".to_owned()],
            failed: vec![("guard2".to_owned(), DriverError::Auth)],
        }
    }

    #[derive(Default)]
    struct FleetAuditRecorder(Mutex<Vec<AuditEvent>>);

    #[async_trait::async_trait]
    impl AuditRecorder for FleetAuditRecorder {
        async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
            self.0.lock().expect("audit mutex").push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn fleet_audit_records_only_targets_with_successful_accounts_and_no_names() {
        let actor = Uuid::now_v7();
        let targets: Vec<Uuid> = (1..=5).map(Uuid::from_u128).collect();
        let context = RequestContext::new(
            Some(actor),
            "session".to_owned(),
            None,
            "fleet-correlation".to_owned(),
            RequestAuth::default(),
        );
        let result = FleetWriteResult {
            per_target: vec![
                FleetTarget {
                    target_id: targets[0],
                    outcome: FleetTargetOutcome::Done(bulk_outcome()),
                },
                FleetTarget {
                    target_id: targets[2],
                    outcome: FleetTargetOutcome::Done(BulkOutcome {
                        succeeded: Vec::new(),
                        failed: vec![("failed-user".to_owned(), DriverError::Auth)],
                    }),
                },
                FleetTarget {
                    target_id: targets[3],
                    outcome: FleetTargetOutcome::Unreachable {
                        reason: "offline".to_owned(),
                    },
                },
                FleetTarget {
                    target_id: targets[4],
                    outcome: FleetTargetOutcome::Done(BulkOutcome {
                        succeeded: vec!["second-success".to_owned()],
                        failed: Vec::new(),
                    }),
                },
            ],
        };
        let response = build_write_response(&targets, &[true, false, true, true, true], result);
        for (event_type, operation) in [
            ("device.users.created", "fleet_create_users"),
            ("device.users.deleted", "fleet_delete_users"),
        ] {
            let recorder = FleetAuditRecorder::default();
            audit_fleet_write(&recorder, &context, &response, event_type, operation).await;
            let events = recorder.0.lock().expect("audit mutex");
            assert_eq!(
                events.len(),
                2,
                "denied, failed and unreachable must not record success"
            );
            for (event, target_id) in events.iter().zip([targets[0], targets[4]]) {
                assert_eq!(event.event_type, event_type);
                assert_eq!(event.actor_id, Some(actor));
                assert_eq!(event.actor_kind, "user");
                assert_eq!(event.target_type.as_deref(), Some("cctv_device"));
                assert_eq!(event.target_id, Some(target_id));
                assert_eq!(event.metadata["device_id"], serde_json::json!(target_id));
                assert_eq!(event.metadata["correlation_id"], "fleet-correlation");
                assert_eq!(event.metadata["operation"], operation);
                assert_eq!(event.metadata["succeeded_count"], 1);
                let metadata = event.metadata.to_string();
                for forbidden in [
                    "guard1",
                    "guard2",
                    "failed-user",
                    "second-success",
                    "password",
                    "credential",
                ] {
                    assert!(
                        !metadata.contains(forbidden),
                        "fleet audit must omit names and secrets"
                    );
                }
            }
            assert_eq!(events[0].metadata["result"], "partial_success");
            assert_eq!(events[0].metadata["failed_count"], 1);
            assert_eq!(events[1].metadata["result"], "success");
            assert_eq!(events[1].metadata["failed_count"], 0);
        }
    }

    #[test]
    fn fleet_requests_reject_empty_batches_with_the_canonical_validation_status() {
        use axum::http::StatusCode;

        use crate::api::rest::error_mapping::map_app_error_to_http;

        let target = Uuid::from_u128(1);
        let create = serde_json::from_value::<CreateFleetUsersRequest>(serde_json::json!({
            "target_ids": [target],
            "users": [],
        }))
        .expect("deserialize create");
        let create_no_targets = serde_json::from_value::<CreateFleetUsersRequest>(
            serde_json::json!({"target_ids": [], "users": [{"name": "fresh"}]}),
        )
        .expect("deserialize create without targets");
        let delete = serde_json::from_value::<DeleteFleetUsersRequest>(serde_json::json!({
            "target_ids": [target],
            "usernames": [],
        }))
        .expect("deserialize delete");
        let preview_create = serde_json::from_value::<PreviewFleetUsersRequest>(
            serde_json::json!({"target_ids": [target], "op": "create", "users": []}),
        )
        .expect("deserialize preview create");
        let preview_delete = serde_json::from_value::<PreviewFleetUsersRequest>(
            serde_json::json!({"target_ids": [target], "op": "delete", "usernames": []}),
        )
        .expect("deserialize preview delete");

        for validated in [
            create.validate(),
            create_no_targets.validate(),
            delete.validate(),
            preview_create.validate(),
            preview_delete.validate(),
        ] {
            let error = validated.expect_err("an empty fleet batch must be rejected");
            assert_eq!(
                map_app_error_to_http(error).status(),
                StatusCode::BAD_REQUEST,
                "an empty fleet batch must answer 400, not 200",
            );
        }

        let full = serde_json::from_value::<CreateFleetUsersRequest>(serde_json::json!({
            "target_ids": [target],
            "users": [{"name": "fresh"}],
        }))
        .expect("deserialize full create");
        assert!(full.validate().is_ok(), "a non-empty fleet create is valid");
    }

    #[test]
    fn request_dtos_deduplicate_target_ids_in_first_seen_order() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let target_ids = serde_json::json!([first, second, first, second]);

        let create: CreateFleetUsersRequest = serde_json::from_value(serde_json::json!({
            "target_ids": target_ids.clone(),
            "users": [],
        }))
        .expect("deserialize create request");
        let delete: DeleteFleetUsersRequest = serde_json::from_value(serde_json::json!({
            "target_ids": target_ids.clone(),
            "usernames": [],
        }))
        .expect("deserialize delete request");
        let preview: PreviewFleetUsersRequest = serde_json::from_value(serde_json::json!({
            "target_ids": target_ids,
            "op": "create",
        }))
        .expect("deserialize preview request");

        for actual in [&create.target_ids, &delete.target_ids, &preview.target_ids] {
            assert_eq!(actual, &[first, second]);
        }
    }

    // A denied target is excluded from the service call and surfaces as
    // not_permitted, while request order (including its position) is preserved.
    #[test]
    fn write_response_injects_not_permitted_and_preserves_order() {
        let denied = Uuid::from_u128(1);
        let ok_a = Uuid::from_u128(2);
        let ok_b = Uuid::from_u128(3);
        let target_ids = [denied, ok_a, ok_b];
        let permitted_flags = [false, true, true];

        // The service only ever saw the permitted subset [ok_a, ok_b], in order.
        let result = FleetWriteResult {
            per_target: vec![
                FleetTarget {
                    target_id: ok_a,
                    outcome: FleetTargetOutcome::Done(bulk_outcome()),
                },
                FleetTarget {
                    target_id: ok_b,
                    outcome: FleetTargetOutcome::Unreachable {
                        reason: "simulated unreachable".to_owned(),
                    },
                },
            ],
        };

        let response = build_write_response(&target_ids, &permitted_flags, result);

        let ids: Vec<Uuid> = response
            .per_target
            .iter()
            .map(|entry| entry.target_id)
            .collect();
        assert_eq!(ids, [denied, ok_a, ok_b], "request order is preserved");
        assert!(matches!(
            response.per_target[0].outcome,
            FleetWriteTargetOutcomeResponse::NotPermitted
        ));
        assert!(matches!(
            response.per_target[1].outcome,
            FleetWriteTargetOutcomeResponse::Done(_)
        ));
        assert!(matches!(
            response.per_target[2].outcome,
            FleetWriteTargetOutcomeResponse::Unreachable { .. }
        ));
    }

    // Preview mirrors the same partition: eligible verdict, service ineligible,
    // and the RBAC-injected not_permitted, all in request order.
    #[test]
    fn preview_response_injects_not_permitted_and_preserves_order() {
        let ok = Uuid::from_u128(10);
        let denied = Uuid::from_u128(11);
        let ineligible = Uuid::from_u128(12);
        let target_ids = [ok, denied, ineligible];
        let permitted_flags = [true, false, true];

        let preview = FleetPreview {
            per_target: vec![
                (
                    ok,
                    FleetPreviewOutcome::Preview(WritePreview {
                        per_item: vec![PreviewItem {
                            name: "fresh".to_owned(),
                            action: PreviewAction::Create,
                            reason: "new".to_owned(),
                        }],
                    }),
                ),
                (
                    ineligible,
                    FleetPreviewOutcome::Ineligible {
                        reason: "not supported".to_owned(),
                    },
                ),
            ],
        };

        // Verdicts align with the permitted subset [ok, ineligible].
        let response =
            build_preview_response(&target_ids, &permitted_flags, preview, &[true, true]);

        let ids: Vec<Uuid> = response
            .per_target
            .iter()
            .map(|entry| entry.target_id)
            .collect();
        assert_eq!(ids, [ok, denied, ineligible]);
        assert!(matches!(
            response.per_target[0].outcome,
            FleetPreviewTargetOutcomeResponse::Preview(_)
        ));
        assert!(matches!(
            response.per_target[1].outcome,
            FleetPreviewTargetOutcomeResponse::NotPermitted
        ));
        assert!(matches!(
            response.per_target[2].outcome,
            FleetPreviewTargetOutcomeResponse::Ineligible { .. }
        ));
    }

    // The whole request is refused only on a non-authorization fault; a plain
    // deny never reaches here (it is captured as a flag), so the merge builders
    // never see NotPermitted from the service.
    #[test]
    fn all_denied_yields_all_not_permitted_with_flag_reported() {
        let a = Uuid::from_u128(20);
        let b = Uuid::from_u128(21);
        let target_ids = [a, b];
        let permitted_flags = [false, false];

        // Every target denied: the service was handed an empty permitted set
        // and produced no per-target entries.
        let result = FleetWriteResult {
            per_target: Vec::new(),
        };

        let response = build_write_response(&target_ids, &permitted_flags, result);

        assert_eq!(response.per_target.len(), 2);
        assert!(
            response.per_target.iter().all(|entry| matches!(
                entry.outcome,
                FleetWriteTargetOutcomeResponse::NotPermitted
            )),
            "every denied target is not_permitted",
        );
    }

    // password is write-only: it must never appear in any serialized fleet
    // response, mirroring the single-device guarantee.
    #[test]
    fn fleet_responses_never_serialize_a_password() {
        let target = Uuid::from_u128(30);
        let write_response = build_write_response(
            &[target],
            &[true],
            FleetWriteResult {
                per_target: vec![FleetTarget {
                    target_id: target,
                    outcome: FleetTargetOutcome::Done(bulk_outcome()),
                }],
            },
        );
        let json = serde_json::to_string(&write_response).expect("serialize write response");
        assert!(
            !json.contains("password"),
            "fleet write response must not carry a password field: {json}"
        );

        let preview_response = build_preview_response(
            &[target],
            &[true],
            FleetPreview {
                per_target: vec![(
                    target,
                    FleetPreviewOutcome::Preview(WritePreview {
                        per_item: vec![PreviewItem {
                            name: "guard1".to_owned(),
                            action: PreviewAction::Create,
                            reason: "new".to_owned(),
                        }],
                    }),
                )],
            },
            &[false],
        );
        let json = serde_json::to_string(&preview_response).expect("serialize preview response");
        assert!(
            !json.contains("password"),
            "fleet preview response must not carry a password field: {json}"
        );
    }

    // The externally-tagged outcome renders not_permitted as a bare string and
    // the payload variants as objects — the documented wire shape.
    #[test]
    fn write_outcome_wire_shape_is_stable() {
        let not_permitted =
            serde_json::to_value(FleetWriteTargetOutcomeResponse::NotPermitted).expect("serialize");
        assert_eq!(not_permitted, serde_json::json!("not_permitted"));

        let ineligible = serde_json::to_value(FleetWriteTargetOutcomeResponse::Ineligible {
            reason: "off".to_owned(),
        })
        .expect("serialize");
        assert_eq!(
            ineligible,
            serde_json::json!({"ineligible": {"reason": "off"}})
        );
    }
}
