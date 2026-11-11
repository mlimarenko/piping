//! REST handlers for the current user's local authentication password.

use axum::{
    Extension, Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::application::validate_local_password;
use crate::contexts::access_control::domain::{
    CredentialProvisioningCommand, CredentialProvisioningMode,
};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ChangeOwnPasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ChangeOwnPasswordResponse {
    pub user_id: String,
    pub password_changed_at: String,
}

/// Change the current user's local password.
#[utoipa::path(
    put,
    path = "/api/v1/user/settings/password",
    tag = "user-settings",
    request_body = ChangeOwnPasswordRequest,
    responses(
        (status = 200, description = "Password changed", body = ChangeOwnPasswordResponse),
        (status = 400, description = "Invalid password request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required or current password is invalid", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn change_own_password(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ChangeOwnPasswordRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let auth_service = state.services.auth.clone();
    if let Err(error) = auth_service
        .verify_local_password_for_user(user_id, &payload.current_password)
        .await
    {
        return handle_app_error(error);
    }

    if let Err(error) = validate_local_password(
        payload.new_password.trim(),
        state.settings.auth.password_min_length,
    ) {
        return handle_app_error(error);
    }

    let command = match CredentialProvisioningCommand::new(
        CredentialProvisioningMode::Manual,
        Some(payload.new_password),
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service
        .set_local_password_with_command(user_id, command)
        .await
    {
        Ok(result) => Json(ChangeOwnPasswordResponse {
            user_id: result.credential.user_id.to_string(),
            password_changed_at: result.credential.password_changed_at.to_rfc3339(),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}
