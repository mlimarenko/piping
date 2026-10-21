use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::secret_field_action_for_intent;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::parse_uuid;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent,
    is_user_connection_credential_set_consumer_type, normalize_secret_field_consumer_type,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SecretFieldStatesQuery {
    pub consumer_type: String,
    pub consumer_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApplySecretFieldIntentPayload {
    pub consumer_type: String,
    pub consumer_id: String,
    pub owner_scope_type: String,
    pub owner_scope_id: String,
    pub field_key: String,
    pub intent: String,
    pub value: Option<String>,
    pub secret_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SecretFieldStateResponse {
    pub consumer_type: String,
    pub consumer_id: String,
    pub owner_scope_type: String,
    pub owner_scope_id: String,
    pub field_key: String,
    pub enabled: bool,
    pub secret_id: Option<String>,
    pub has_value: bool,
}

/// List the state of secret fields for an entity.
#[utoipa::path(
    get,
    path = "/api/v1/secret-fields",
    tag = "secret-fields",
    params(
        ("consumer_type" = String, Query, description = "Secret field consumer type"),
        ("consumer_id" = String, Query, description = "Secret field consumer UUID"),
    ),
    responses(
        (status = 200, description = "Secret field states", body = [SecretFieldStateResponse]),
        (status = 400, description = "Invalid secret field query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Secret field access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_secret_field_states(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<SecretFieldStatesQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_secret_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }

    let consumer_type = match normalize_secret_field_consumer_type(&query.consumer_type) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let consumer_id = match parse_uuid(&query.consumer_id, "consumer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_user_owned_secret_access(&state, &request_context, &consumer_type, consumer_id, None)
            .await
    {
        return handle_app_error(error);
    }

    match state
        .services
        .secret_field_states
        .list_for_consumer(&consumer_type, consumer_id)
        .await
    {
        Ok(states) => (
            StatusCode::OK,
            Json(
                states
                    .into_iter()
                    .map(map_state_response)
                    .collect::<Vec<_>>(),
            ),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Apply set/clear/keep intents to secret fields.
#[utoipa::path(
    put,
    path = "/api/v1/secret-fields",
    tag = "secret-fields",
    request_body = ApplySecretFieldIntentPayload,
    responses(
        (status = 200, description = "Secret field state", body = SecretFieldStateResponse),
        (status = 400, description = "Invalid secret field intent", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Secret field access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn apply_secret_field_intent(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ApplySecretFieldIntentPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let action = match secret_field_action_for_intent(&payload.intent) {
        Some(value) => value,
        None => {
            return handle_app_error(AppError::Validation(
                "intent must be one of: set, rotate, clear".to_string(),
            ));
        }
    };
    if let Err(error) = ensure_secret_access(&state, &request_context, action).await {
        return handle_app_error(error);
    }

    let intent = match SecretFieldIntent::parse(&payload.intent) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let consumer_id = match parse_uuid(&payload.consumer_id, "consumer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let owner_scope_id = match parse_uuid(&payload.owner_scope_id, "owner_scope_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_user_owned_secret_access(
        &state,
        &request_context,
        &payload.consumer_type,
        consumer_id,
        Some(owner_scope_id),
    )
    .await
    {
        return handle_app_error(error);
    }
    let secret_id = match payload.secret_id.as_ref() {
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => match parse_uuid(raw, "secret_id") {
            Ok(value) => Some(value),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };

    let consumer_type = match normalize_secret_field_consumer_type(&payload.consumer_type) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let input = ApplySecretFieldIntentInput {
        consumer_type,
        consumer_id,
        owner_scope_type: payload.owner_scope_type,
        owner_scope_id,
        field_key: payload.field_key,
        intent,
        value: payload.value,
        secret_id,
    };

    match state
        .services
        .secret_field_intents
        .apply_intent(input)
        .await
    {
        Ok(state) => (StatusCode::OK, Json(map_state_response(state))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

async fn ensure_user_owned_secret_access(
    state: &AppState,
    request_context: &RequestContext,
    consumer_type: &str,
    consumer_id: Uuid,
    owner_scope_id: Option<Uuid>,
) -> Result<(), AppError> {
    let normalized = normalize_secret_field_consumer_type(consumer_type)?;
    if !is_user_connection_credential_set_consumer_type(&normalized) {
        return Ok(());
    }

    let actor_id = request_context.user_id.ok_or_else(|| {
        AppError::Forbidden("User credentials require authentication".to_string())
    })?;
    if owner_scope_id.is_some_and(|owner| owner != actor_id) {
        return Err(AppError::Forbidden(
            "Cannot modify credentials for another user".to_string(),
        ));
    }
    state
        .services
        .user_credential_sets
        .get_for_user(actor_id, consumer_id)
        .await
        .map_err(|error| match error {
            AppError::NotFound(_) | AppError::Forbidden(_) => {
                AppError::Forbidden("Credential set is not owned by current user".to_string())
            }
            other => other,
        })?;
    Ok(())
}

async fn ensure_secret_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    let policy_service = build_policy_evaluation_service(state);
    policy_service
        .ensure_access(request_context, action, ResourceType::SecretVariable, None)
        .await
}

fn map_state_response(
    state: crate::contexts::secrets::application::SecretFieldState,
) -> SecretFieldStateResponse {
    SecretFieldStateResponse {
        consumer_type: state.consumer_type,
        consumer_id: state.consumer_id.to_string(),
        owner_scope_type: state.owner_scope_type,
        owner_scope_id: state.owner_scope_id.to_string(),
        field_key: state.field_key,
        enabled: state.enabled,
        secret_id: state.secret_id.map(|value| value.to_string()),
        has_value: state.has_value,
    }
}
