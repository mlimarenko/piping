use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::parse_uuid;
use crate::app::AppState;
use crate::contexts::connections::application::user_credential_set_service::UserCredentialSetQuery;
use crate::contexts::connections::application::{
    CreateUserCredentialSetInput, UpdateUserCredentialSetInput,
};
use crate::contexts::connections::domain::{ConnectionProtocol, parse_connection_protocol};
use crate::contexts::connections::infrastructure::USER_CREDENTIAL_SET_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListUserCredentialSetsQuery {
    pub protocol: Option<String>,
    pub is_default: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateUserCredentialSetRequest {
    pub protocol: String,
    pub name: String,
    pub params: Value,
    pub is_default: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateUserCredentialSetRequest {
    pub name: String,
    pub params: Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserCredentialSetResponse {
    pub id: String,
    pub user_id: String,
    pub protocol: String,
    pub name: String,
    pub params: Value,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserCredentialSetListResponse {
    pub items: Vec<UserCredentialSetResponse>,
    /// Total number of credential sets matching the filters, not the page length.
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// List the current user's credential sets.
#[utoipa::path(
    get,
    path = "/api/v1/user/settings/credentials",
    tag = "user-settings",
    params(
        ("protocol" = Option<String>, Query, description = "Comma-separated credential protocol filter"),
        ("is_default" = Option<String>, Query, description = "Comma-separated default-state filter"),
        ("limit" = Option<u32>, Query, description = "Maximum number of credential sets to return"),
        ("offset" = Option<u32>, Query, description = "Number of credential sets to skip"),
        ("sort" = Option<String>, Query, description = "Allowlisted sort key"),
        ("order" = Option<String>, Query, description = "Sort direction: asc or desc"),
    ),
    responses(
        (status = 200, description = "User credential sets", body = UserCredentialSetListResponse),
        (status = 400, description = "Invalid protocol", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_user_credential_sets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListUserCredentialSetsQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let protocol = match parse_protocol_filter_list(query.protocol.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let is_default = match parse_default_filter(query.is_default.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        USER_CREDENTIAL_SET_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let service = state.services.user_credential_sets.clone();
    match service
        .list_for_user(
            user_id,
            UserCredentialSetQuery {
                protocol,
                is_default,
                sort,
            },
            limit,
            offset,
        )
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(UserCredentialSetListResponse {
                items: items.into_iter().map(map_set).collect(),
                total,
                limit,
                offset,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a user credential set.
#[utoipa::path(
    post,
    path = "/api/v1/user/settings/credentials",
    tag = "user-settings",
    request_body = CreateUserCredentialSetRequest,
    responses(
        (status = 201, description = "User credential set created", body = UserCredentialSetResponse),
        (status = 400, description = "Invalid credential set payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_user_credential_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateUserCredentialSetRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let protocol = match parse_protocol_filter(Some(payload.protocol.as_str())) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return handle_app_error(AppError::Validation("protocol is required".to_string()));
        }
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.user_credential_sets.clone();
    match service
        .create_for_user(
            user_id,
            CreateUserCredentialSetInput {
                protocol,
                name: payload.name,
                params: payload.params,
                make_default: payload.is_default,
            },
        )
        .await
    {
        Ok(item) => (StatusCode::CREATED, Json(map_set(item))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a user credential set.
#[utoipa::path(
    put,
    path = "/api/v1/user/settings/credentials/{id}",
    tag = "user-settings",
    params(
        ("id" = String, Path, description = "Credential set UUID"),
    ),
    request_body = UpdateUserCredentialSetRequest,
    responses(
        (status = 200, description = "User credential set updated", body = UserCredentialSetResponse),
        (status = 400, description = "Invalid credential set payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Credential set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_user_credential_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(set_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateUserCredentialSetRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let set_id = match parse_uuid(&set_id, "credential set id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.user_credential_sets.clone();
    match service
        .update_for_user(
            user_id,
            set_id,
            UpdateUserCredentialSetInput {
                name: payload.name,
                params: payload.params,
            },
        )
        .await
    {
        Ok(item) => (StatusCode::OK, Json(map_set(item))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a user credential set.
#[utoipa::path(
    delete,
    path = "/api/v1/user/settings/credentials/{id}",
    tag = "user-settings",
    params(
        ("id" = String, Path, description = "Credential set UUID"),
    ),
    responses(
        (status = 204, description = "User credential set deleted"),
        (status = 400, description = "Invalid credential set UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Credential set is owned by another user", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Credential set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Default credential set cannot be deleted", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Credential set deletion failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_user_credential_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(set_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let set_id = match parse_uuid(&set_id, "credential set id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.user_credential_sets.clone();
    match service.delete_for_user(user_id, set_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Set the default user credential set.
#[utoipa::path(
    post,
    path = "/api/v1/user/settings/credentials/{id}/default",
    tag = "user-settings",
    params(
        ("id" = String, Path, description = "Credential set UUID"),
    ),
    responses(
        (status = 204, description = "Default credential set updated"),
        (status = 400, description = "Invalid credential set UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Credential set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn set_default_user_credential_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(set_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let set_id = match parse_uuid(&set_id, "credential set id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.user_credential_sets.clone();
    match service.set_default_for_user(user_id, set_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

fn map_set(
    value: crate::contexts::connections::domain::UserCredentialSet,
) -> UserCredentialSetResponse {
    UserCredentialSetResponse {
        id: value.id.to_string(),
        user_id: value.user_id.to_string(),
        protocol: value.protocol.as_str().to_string(),
        name: value.name,
        params: value.params,
        is_default: value.is_default,
        created_at: value.created_at.to_rfc3339(),
        updated_at: value.updated_at.to_rfc3339(),
    }
}

fn parse_protocol_filter(raw: Option<&str>) -> Result<Option<ConnectionProtocol>, AppError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let protocol = parse_connection_protocol(value)
                .map_err(|error| AppError::Validation(error.to_string()))?;
            match protocol {
                ConnectionProtocol::Ssh | ConnectionProtocol::Rdp | ConnectionProtocol::Vnc => {
                    Ok(Some(protocol))
                }
                _ => Err(AppError::Validation(
                    "Credential sets are supported only for ssh/rdp/vnc".to_string(),
                )),
            }
        }
    }
}

fn parse_protocol_filter_list(raw: Option<&str>) -> Result<Vec<ConnectionProtocol>, AppError> {
    let mut protocols = Vec::new();
    for value in parse_csv_strings(raw) {
        let protocol = parse_protocol_filter(Some(value.as_str()))?
            .ok_or_else(|| AppError::Validation("protocol is required".to_string()))?;
        if !protocols.contains(&protocol) {
            protocols.push(protocol);
        }
    }
    Ok(protocols)
}

fn parse_default_filter(raw: Option<&str>) -> Result<Vec<String>, AppError> {
    let mut values = Vec::new();
    for value in parse_csv_strings(raw) {
        let normalized = value.to_ascii_lowercase();
        match normalized.as_str() {
            "true" | "false" => {
                if !values.contains(&normalized) {
                    values.push(normalized);
                }
            }
            _ => {
                return Err(AppError::Validation(format!(
                    "Invalid is_default value {value:?}: expected true or false"
                )));
            }
        }
    }
    Ok(values)
}
