//! REST handlers for the canonical persisted settings service.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::application::{EffectiveSetting, SettingOverride};
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

async fn ensure_admin_settings_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            action,
            ResourceType::Settings,
            Some("settings"),
        )
        .await
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct EffectiveSettingsQuery {
    /// Optional comma-separated key list. When omitted, persisted user and
    /// instance keys are returned.
    pub keys: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertSettingRequest {
    pub value: Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EffectiveSettingsResponse {
    pub settings: Vec<EffectiveSetting>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminSettingsDefaultsResponse {
    pub settings: Vec<SettingOverride>,
}

fn parse_keys(raw: Option<String>) -> Option<Vec<String>> {
    raw.map(|keys| {
        keys.split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    })
}

/// List effective persisted settings for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/settings/effective",
    tag = "settings",
    params(EffectiveSettingsQuery),
    responses(
        (status = 200, description = "Effective persisted settings", body = EffectiveSettingsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_effective_settings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<EffectiveSettingsQuery>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .settings
        .list_effective(user_id, parse_keys(query.keys))
        .await
    {
        Ok(settings) => Json(EffectiveSettingsResponse { settings }).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get one effective persisted setting for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/settings/effective/{key}",
    tag = "settings",
    params(
        ("key" = String, Path, description = "Setting key"),
    ),
    responses(
        (status = 200, description = "Effective persisted setting", body = EffectiveSetting),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_effective_setting(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state.services.settings.get_effective(user_id, &key).await {
        Ok(setting) => Json(setting).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create or update a user setting override.
#[utoipa::path(
    put,
    path = "/api/v1/settings/user/{key}",
    tag = "settings",
    params(
        ("key" = String, Path, description = "Setting key"),
    ),
    request_body = UpsertSettingRequest,
    responses(
        (status = 204, description = "User setting override stored"),
        (status = 400, description = "Invalid key or value", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn upsert_user_setting(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(key): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpsertSettingRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .settings
        .upsert_user_override(user_id, &key, payload.value)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a user setting override.
#[utoipa::path(
    delete,
    path = "/api/v1/settings/user/{key}",
    tag = "settings",
    params(
        ("key" = String, Path, description = "Setting key"),
    ),
    responses(
        (status = 204, description = "User setting override deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_user_setting(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .settings
        .delete_user_override(user_id, &key)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List instance default settings.
#[utoipa::path(
    get,
    path = "/api/v1/admin/settings/defaults",
    tag = "admin-settings",
    responses(
        (status = 200, description = "Instance default settings", body = AdminSettingsDefaultsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Admin access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_admin_setting_defaults(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_admin_settings_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    match state.services.settings.list_instance_defaults().await {
        Ok(settings) => Json(AdminSettingsDefaultsResponse { settings }).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create or update an instance default setting.
#[utoipa::path(
    put,
    path = "/api/v1/admin/settings/defaults/{key}",
    tag = "admin-settings",
    params(
        ("key" = String, Path, description = "Setting key"),
    ),
    request_body = UpsertSettingRequest,
    responses(
        (status = 200, description = "Instance default setting stored", body = SettingOverride),
        (status = 400, description = "Invalid key or value", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Admin access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn upsert_admin_setting_default(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(key): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpsertSettingRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let updated_by_user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_admin_settings_access(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    match state
        .services
        .settings
        .upsert_instance_default(&key, payload.value, Some(updated_by_user_id))
        .await
    {
        Ok(setting) => Json(setting).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an instance default setting.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/settings/defaults/{key}",
    tag = "admin-settings",
    params(
        ("key" = String, Path, description = "Setting key"),
    ),
    responses(
        (status = 204, description = "Instance default setting deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Admin access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_admin_setting_default(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_admin_settings_access(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    match state.services.settings.delete_instance_default(&key).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}
