use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::idempotency_key::extract_idempotency_key;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::dto::license_lifecycle::{EntitlementState, LicenseActivateRequest};
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::error_mapping::build_error_response;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::application::{
    AuditRecorder, OperationTimelineActionKind, OperationTimelineResult, OperationTimelineService,
    OperationTimelineWriteRequest,
};
use crate::contexts::shared::domain::reason_codes;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_error_event, log_event, metrics};

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LicenseRefreshResponse {
    pub status: String,
    pub synced_at: String,
    pub refresh_after_seconds: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LicenseHealthDetailsResponse {
    pub status: String,
    pub reason_code: String,
    pub entitlement_expires_at: Option<String>,
    pub grace_started_at: Option<String>,
    pub grace_ends_at: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_successful_sync_at: Option<String>,
    pub last_sync_error: Option<String>,
    pub last_failure_kind: Option<String>,
    pub consecutive_failures: i32,
}

/// Gate licensing endpoints on the dedicated `License` resource type. License
/// administration requires an explicit `License` policy: authentication alone is
/// not enough, so a signed-in user without the grant cannot read state or
/// trigger activate/refresh.
async fn ensure_license_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(request_context, action, ResourceType::License, None)
        .await
}

/// Get the current license state.
#[utoipa::path(
    get,
    path = "/api/v1/license/state",
    tag = "license",
    responses(
        (status = 200, description = "Current license entitlement state", body = EntitlementState),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_license_state(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.user_id.is_none() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }
    if let Err(error) = ensure_license_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }
    log_event(
        LogLevel::Info,
        "license.state.requested",
        EventFields::default()
            .with_operation("license_state")
            .with_user_id(
                request_context
                    .user_id
                    .map(|user_id| user_id.to_string())
                    .unwrap_or_else(|| "anonymous".to_string()),
            )
            .with_correlation_id(request_context.correlation_id.clone()),
    );

    let response = match load_entitlement_state_projection(&state).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Activate a license.
#[utoipa::path(
    post,
    path = "/api/v1/license/activate",
    tag = "license",
    request_body = LicenseActivateRequest,
    responses(
        (status = 200, description = "License activated", body = EntitlementState),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Issuer revoked the token or archived its organization", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Issuer no longer recognizes the activation token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Activation conflicts with current license state", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 422, description = "Activation payload rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn activate_license(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<LicenseActivateRequest>,
) -> impl IntoResponse {
    let timeline = OperationTimelineService::new(state.services.audit.clone());
    let idempotency_key = match extract_idempotency_key(&headers) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if request_context.user_id.is_none() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }
    if let Err(error) = ensure_license_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }
    let mut fields = EventFields::default()
        .with_operation("license_activate")
        .with_user_id(
            request_context
                .user_id
                .map(|user_id| user_id.to_string())
                .unwrap_or_else(|| "anonymous".to_string()),
        )
        .with_correlation_id(request_context.correlation_id.clone());
    if let Some(idempotency_key) = idempotency_key.as_ref() {
        fields = fields.with_context("idempotency_key", idempotency_key.value.clone());
    }
    log_event(LogLevel::Info, "license.activate.requested", fields);

    let service_result = match state
        .services
        .license_activation
        .activate(&payload.activation_token)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            let activation_reason_code = activation_timeline_reason_code(&error);
            metrics::record_license_activation_failure(
                activation_reason_code.as_deref().unwrap_or("unknown"),
            );
            let server_instance_id = state
                .services
                .licensing
                .load_instance_fingerprint()
                .await
                .ok()
                .flatten();
            try_record_activation_timeline_event(
                &timeline,
                request_context.user_id,
                OperationTimelineResult::Rejected,
                activation_reason_code.as_deref(),
                server_instance_id.as_deref(),
            )
            .await;
            log_error_event(
                "license.activate.persist_failed",
                "license_persist_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("license_activate")
                    .with_status("failed"),
            );
            return activation_error_response(error);
        }
    };

    let response = match load_entitlement_state_projection(&state).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    metrics::record_license_activation_success();
    try_record_activation_timeline_event(
        &timeline,
        request_context.user_id,
        OperationTimelineResult::Success,
        None,
        Some(service_result.instance_id.as_str()),
    )
    .await;
    log_event(
        LogLevel::Info,
        "license.activate.completed",
        EventFields::default()
            .with_operation("license_activate")
            .with_status("ok")
            .with_resource("instance", service_result.instance_id.clone())
            .with_context("seat_limit", response.seat_limit.to_string()),
    );
    (StatusCode::OK, Json(response)).into_response()
}

/// Refresh the license from the control plane.
#[utoipa::path(
    post,
    path = "/api/v1/license/refresh",
    tag = "license",
    responses(
        (status = 200, description = "License state refreshed", body = LicenseRefreshResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Issuer refused the entitlement; new seats are suspended", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Issuer no longer recognizes the activation token", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Issuer binding conflict or superseded refresh", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 502, description = "Licensing service unavailable or response unrecognized", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn refresh_license(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let idempotency_key = match extract_idempotency_key(&headers) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.user_id.is_none() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }
    if let Err(error) = ensure_license_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }
    let mut fields = EventFields::default()
        .with_operation("license_refresh")
        .with_user_id(
            request_context
                .user_id
                .map(|user_id| user_id.to_string())
                .unwrap_or_else(|| "anonymous".to_string()),
        )
        .with_correlation_id(request_context.correlation_id.clone());
    if let Some(idempotency_key) = idempotency_key.as_ref() {
        fields = fields.with_context("idempotency_key", idempotency_key.value.clone());
    }
    log_event(LogLevel::Info, "license.refresh.requested", fields);

    let response = match refresh_license_state(&state).await {
        Ok(value) => {
            log_event(
                LogLevel::Info,
                "license.refresh.completed",
                EventFields::default()
                    .with_operation("license_refresh")
                    .with_status("ok")
                    .with_context(
                        "refresh_after_seconds",
                        value.refresh_after_seconds.to_string(),
                    ),
            );
            value
        }
        Err(error) => return handle_app_error(error),
    };
    (StatusCode::OK, Json(response)).into_response()
}

/// Get detailed license health information.
#[utoipa::path(
    get,
    path = "/api/v1/license/health/details",
    tag = "license",
    responses(
        (status = 200, description = "License health details", body = LicenseHealthDetailsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_license_health_details(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.user_id.is_none() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }
    if let Err(error) = ensure_license_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    let details = match state.services.license_status.get_health_details().await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let Some(details) = details else {
        return (
            StatusCode::OK,
            Json(LicenseHealthDetailsResponse {
                status: "expired".to_string(),
                reason_code: reason_codes::ENTITLEMENT_EXPIRED.to_string(),
                entitlement_expires_at: None,
                grace_started_at: None,
                grace_ends_at: None,
                last_sync_at: None,
                last_successful_sync_at: None,
                last_sync_error: None,
                last_failure_kind: None,
                consecutive_failures: 0,
            }),
        )
            .into_response();
    };

    (
        StatusCode::OK,
        Json(LicenseHealthDetailsResponse {
            status: details.status,
            reason_code: details.reason_code,
            entitlement_expires_at: details
                .snapshot
                .entitlement_expires_at
                .map(|value| value.to_rfc3339()),
            grace_started_at: details
                .snapshot
                .grace_started_at
                .map(|value| value.to_rfc3339()),
            grace_ends_at: details
                .snapshot
                .grace_ends_at
                .map(|value| value.to_rfc3339()),
            last_sync_at: details
                .snapshot
                .last_sync_at
                .map(|value| value.to_rfc3339()),
            last_successful_sync_at: details
                .snapshot
                .last_successful_sync_at
                .map(|value| value.to_rfc3339()),
            last_sync_error: details.snapshot.last_sync_error,
            last_failure_kind: details.snapshot.last_failure_kind,
            consecutive_failures: details.snapshot.consecutive_failures,
        }),
    )
        .into_response()
}

pub(crate) async fn refresh_license_state(
    state: &AppState,
) -> Result<LicenseRefreshResponse, AppError> {
    let result = state.services.licensing.refresh_license_state().await?;
    Ok(LicenseRefreshResponse {
        status: result.status,
        synced_at: result.synced_at,
        refresh_after_seconds: result.refresh_after_seconds,
    })
}

async fn load_entitlement_state_projection(state: &AppState) -> Result<EntitlementState, AppError> {
    let projection = state
        .services
        .license_status
        .get_entitlement_projection()
        .await?;
    let version = if projection.entitlement_version.is_empty() {
        0
    } else {
        project_entitlement_version(&projection.entitlement_version)
    };
    Ok(EntitlementState {
        status: projection.status,
        seat_limit: projection.seat_limit,
        seats_used: projection.seats_used,
        valid_from: projection.valid_from.to_rfc3339(),
        valid_until: projection.valid_until.to_rfc3339(),
        version,
    })
}

fn project_entitlement_version(version: &str) -> i64 {
    let uuid = Uuid::new_v5(&Uuid::NAMESPACE_OID, version.trim().as_bytes());
    let [b0, b1, b2, b3, b4, b5, b6, b7, ..] = *uuid.as_bytes();
    let raw = u64::from_be_bytes([b0, b1, b2, b3, b4, b5, b6, b7]);
    (raw & i64::MAX as u64) as i64
}

fn activation_error_response(error: AppError) -> axum::response::Response {
    match error {
        AppError::LicenseEntitlementInvalid(message) => build_error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            message,
            "ENTITLEMENT_INVALID",
        ),
        AppError::Validation(message) => {
            let (mapped_message, reason_code) = parse_reason_code_message(&message);
            build_error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_ERROR",
                mapped_message,
                reason_code.unwrap_or("VALIDATION_ERROR".to_string()),
            )
        }
        AppError::Conflict(message) => {
            let (mapped_message, reason_code) = parse_reason_code_message(&message);
            build_error_response(
                StatusCode::CONFLICT,
                "CONFLICT",
                mapped_message,
                reason_code.unwrap_or("CONFLICT".to_string()),
            )
        }
        other => handle_app_error(other),
    }
}

fn parse_reason_code_message(message: &str) -> (String, Option<String>) {
    let Some((raw_code, raw_message)) = message.split_once(':') else {
        return (message.to_string(), None);
    };
    let reason_code = raw_code.trim();
    let mapped_message = raw_message.trim();
    if mapped_message.is_empty() || !reason_codes::is_canonical_reason_code(reason_code) {
        return (message.to_string(), None);
    }

    (mapped_message.to_string(), Some(reason_code.to_string()))
}

fn activation_timeline_reason_code(error: &AppError) -> Option<String> {
    match error {
        AppError::LicenseIssuerDenied(reason) => Some(reason.code().to_string()),
        AppError::Validation(message) | AppError::Conflict(message) => {
            let (_, reason_code) = parse_reason_code_message(message);
            reason_code.filter(|value| reason_codes::is_canonical_reason_code(value))
        }
        AppError::Auth(_) => Some(reason_codes::AUTH_SESSION_EXPIRED.to_string()),
        _ => None,
    }
}

async fn record_activation_timeline_event<R>(
    timeline: &OperationTimelineService<R>,
    actor_user_id: Option<Uuid>,
    result: OperationTimelineResult,
    reason_code: Option<&str>,
    instance_fingerprint: Option<&str>,
) -> Result<(), AppError>
where
    R: AuditRecorder,
{
    let action_kind = match result {
        OperationTimelineResult::Success => OperationTimelineActionKind::ActivationCompleted,
        OperationTimelineResult::Rejected => OperationTimelineActionKind::ActivationRejected,
    };
    timeline
        .write(OperationTimelineWriteRequest {
            organization_id: None,
            actor_user_id,
            action_kind,
            result,
            reason_code: reason_code.map(str::to_string),
            metadata: json!({
                "instance_fingerprint": instance_fingerprint,
            }),
        })
        .await
        .map(|_| ())
}

async fn try_record_activation_timeline_event<R>(
    timeline: &OperationTimelineService<R>,
    actor_user_id: Option<Uuid>,
    result: OperationTimelineResult,
    reason_code: Option<&str>,
    instance_fingerprint: Option<&str>,
) where
    R: AuditRecorder,
{
    if let Err(error) = record_activation_timeline_event(
        timeline,
        actor_user_id,
        result,
        reason_code,
        instance_fingerprint,
    )
    .await
    {
        warn!(error = %error, "failed to persist activation timeline event");
    }
}

#[cfg(test)]
mod tests {
    use super::project_entitlement_version;

    #[test]
    fn entitlement_version_projection_is_stable() {
        let first = project_entitlement_version("v2");
        let second = project_entitlement_version("v2");
        let other = project_entitlement_version("v3");

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(first >= 0);
    }
}
