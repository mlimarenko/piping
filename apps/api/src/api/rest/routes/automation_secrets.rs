//! REST handlers for admin-managed named automation secrets.
//!
//! These endpoints manage the *identity* and metadata of named secrets used by
//! playbook `runtime.secrets.env` declarations. The secret VALUE is never read
//! or written here — it stays write-only behind
//! `PUT/DELETE /api/v1/secrets/{id}/value`. The list only reports whether a
//! value is set (`has_value`), never the value itself.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::secrets::domain::NamedSecretSummary;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

/// Public projection of a named automation secret. Never carries the value.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AutomationSecretResponse {
    pub id: Uuid,
    pub key: String,
    pub description: Option<String>,
    /// Whether a secret value is currently set (value itself is write-only).
    pub has_value: bool,
    pub is_masked: bool,
    pub created_at: DateTime<Utc>,
    /// Number of automation references (playbook versions, etc.) using it.
    pub usage_count: i64,
}

impl From<NamedSecretSummary> for AutomationSecretResponse {
    fn from(summary: NamedSecretSummary) -> Self {
        Self {
            id: summary.id,
            key: summary.key,
            description: summary.description,
            has_value: summary.has_value,
            is_masked: summary.is_masked,
            created_at: summary.created_at,
            usage_count: summary.usage_count,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListAutomationSecretsResponse {
    pub items: Vec<AutomationSecretResponse>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateAutomationSecretRequest {
    /// `[a-z0-9]([a-z0-9._-]{0,198}[a-z0-9])?`; the `secret-field/` prefix is
    /// reserved and rejected.
    pub key: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateAutomationSecretRequest {
    pub description: Option<String>,
}

/// List named automation secrets.
#[utoipa::path(
    get,
    path = "/api/v1/automation-secrets",
    tag = "secrets",
    responses(
        (status = 200, description = "Named automation secrets", body = ListAutomationSecretsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_automation_secrets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_secret_admin_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    match state.repositories.secrets.list_named().await {
        Ok(summaries) => {
            let items = summaries
                .into_iter()
                .map(AutomationSecretResponse::from)
                .collect();
            Json(ListAutomationSecretsResponse { items }).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a named automation secret (no value; set it via the value route).
#[utoipa::path(
    post,
    path = "/api/v1/automation-secrets",
    tag = "secrets",
    request_body = CreateAutomationSecretRequest,
    responses(
        (status = 201, description = "Named automation secret created", body = AutomationSecretResponse),
        (status = 400, description = "Invalid secret key", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Secret key already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_automation_secret(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateAutomationSecretRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_secret_admin_access(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }

    let created_by = match request_context.require_authenticated_user_id() {
        Ok(user_id) => Some(user_id),
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .secrets
        .create_named(&payload.key, payload.description.as_deref(), created_by)
        .await
    {
        Ok(summary) => (
            StatusCode::CREATED,
            Json(AutomationSecretResponse::from(summary)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a named automation secret's description.
#[utoipa::path(
    patch,
    path = "/api/v1/automation-secrets/{id}",
    tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret UUID")),
    request_body = UpdateAutomationSecretRequest,
    responses(
        (status = 200, description = "Named automation secret updated", body = AutomationSecretResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Secret not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_automation_secret(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdateAutomationSecretRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_secret_admin_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }

    match state
        .repositories
        .secrets
        .update_description(id, payload.description.as_deref())
        .await
    {
        Ok(Some(summary)) => Json(AutomationSecretResponse::from(summary)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Named automation secret not found: {id}"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a named automation secret. Fails with 409 if it is still in use.
#[utoipa::path(
    delete,
    path = "/api/v1/automation-secrets/{id}",
    tag = "secrets",
    params(("id" = Uuid, Path, description = "Secret UUID")),
    responses(
        (status = 204, description = "Named automation secret deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Secret not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Secret is still in use", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_automation_secret(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_secret_admin_access(&state, &request_context, Action::Delete).await {
        return handle_app_error(error);
    }

    match state.repositories.secrets.delete_named(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Named automation secret not found: {id}"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Type-level admin gate: `<action> × SecretVariable`. Managing named-secret
/// identity is an administrative operation; the per-secret run grant (gate C3,
/// `Execute × SecretVariable`) and value reveal (`Secret × SecretVariable`) are
/// separate and enforced elsewhere.
async fn ensure_secret_admin_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    build_policy_evaluation_service(state)
        .ensure_access(request_context, action, ResourceType::SecretVariable, None)
        .await
}
