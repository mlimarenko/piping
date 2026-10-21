use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::parse_uuid;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::secrets::application::secret_store::SecretStore;
use crate::contexts::secrets::infrastructure::persistence::SecretRepository;
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetSecretValuePayload {
    pub value: String,
}

/// Set a secret value.
#[utoipa::path(
    put,
    path = "/api/v1/secrets/{id}/value",
    tag = "secrets",
    params(
        ("id" = String, Path, description = "Secret UUID"),
    ),
    request_body = SetSecretValuePayload,
    responses(
        (status = 204, description = "Secret value updated"),
        (status = 400, description = "Invalid request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Secret not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn set_secret_value(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<SetSecretValuePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let secret_id = match parse_uuid(&id, "secret_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_secret_value_access(&state, &request_context, secret_id).await {
        return handle_app_error(error);
    }
    let value = match parse_required_string(&payload.value, "value") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .secrets
        .get_variable_by_id(secret_id)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return handle_app_error(AppError::NotFound(format!(
                "Secret variable not found: {}",
                secret_id
            )));
        }
        Err(error) => {
            log_error_event(
                "rest.secrets.secret_value.lookup.failed",
                "secret_lookup_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("rest.secrets.set_secret_value")
                    .with_resource("secret_variable", secret_id.to_string())
                    .with_message("Failed to load secret variable"),
            );
            return handle_app_error(error);
        }
    }

    let store = state.services.secret_store.clone();
    if let Err(error) = store.set_secret(secret_id, &value).await {
        return handle_app_error(error);
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Delete a secret value.
#[utoipa::path(
    delete,
    path = "/api/v1/secrets/{id}/value",
    tag = "secrets",
    params(
        ("id" = String, Path, description = "Secret UUID"),
    ),
    responses(
        (status = 204, description = "Secret value deleted"),
        (status = 400, description = "Invalid secret UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Secret not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_secret_value(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let secret_id = match parse_uuid(&id, "secret_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_secret_value_access(&state, &request_context, secret_id).await {
        return handle_app_error(error);
    }
    let exists = match state.repositories.secrets.variable_exists(secret_id).await {
        Ok(value) => value,
        Err(error) => {
            log_error_event(
                "rest.secrets.secret_value.lookup.failed",
                "secret_lookup_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("rest.secrets.delete_secret_value")
                    .with_resource("secret_variable", secret_id.to_string())
                    .with_message("Failed to check secret variable"),
            );
            return handle_app_error(error);
        }
    };
    if !exists {
        return handle_app_error(AppError::NotFound(format!(
            "Secret variable not found: {}",
            secret_id
        )));
    }

    let store = state.services.secret_store.clone();
    if let Err(error) = store.delete_secret(secret_id).await {
        return handle_app_error(error);
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn ensure_secret_value_access(
    state: &AppState,
    request_context: &crate::contexts::shared::request_context::RequestContext,
    secret_id: Uuid,
) -> Result<(), AppError> {
    // Gate on the canonical prefixed resource id so per-secret allow/deny
    // policies apply. Policies with empty/`*` resources still match (they match
    // any resource id), keeping the existing coarse-grained UI policies working.
    let resource_id = format!("secret_variable:{secret_id}");
    let policy_service = build_policy_evaluation_service(state);
    policy_service
        .ensure_access(
            request_context,
            Action::Secret,
            ResourceType::SecretVariable,
            Some(&resource_id),
        )
        .await
}

fn parse_required_string(value: &str, label: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{label} cannot be empty")));
    }
    Ok(trimmed.to_string())
}
