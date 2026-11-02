use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::validation_error::ValidationErrorResponse;
use crate::app::AppState;
use std::collections::HashMap;

use crate::contexts::access_control::domain::{Action, PolicyDecision, ResourceType};
use crate::contexts::connections::application::connection_params_schema::validate_connection_params;
use crate::contexts::inventory::application::config_resolver::{
    compute_secret_presence, redact_secret_fields,
};
use crate::contexts::inventory::application::ConfigContextScopePageRequest;
use crate::contexts::inventory::domain::{ConfigContext, ConfigContextBinding};
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::config_context_repository::{
    CONFIG_CONTEXT_BINDING_COLUMNS, CONFIG_CONTEXT_COLUMNS, ConfigContextBindingListFilters,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::contexts::shared::pagination::normalize_page;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::api::rest::routes::validation::parse_uuid;
use tracing::warn;

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConfigContextListQuery {
    pub name: Option<String>,
    pub is_active: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub include_hierarchy: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    /// Applied only when listing without a scope filter; the scope-resolution
    /// path keeps its precedence ordering.
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConfigContextListResponse {
    pub items: Vec<ConfigContext>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConfigContextBindingListResponse {
    pub items: Vec<ConfigContextBinding>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConfigContextPayload {
    pub name: String,
    pub description: Option<String>,
    pub weight: i32,
    pub data: Value,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConfigContextBindingListQuery {
    pub context_id: String,
    pub scope_type: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConfigContextBindingPayload {
    pub context_id: String,
    pub scope_type: String,
    pub scope_id: String,
}

fn parse_required_string(value: &str, label: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{label} is required")));
    }
    Ok(trimmed.to_string())
}

fn has_connection_params(data: &Value) -> bool {
    data.get("piping")
        .and_then(|value| value.get("connections"))
        .is_some()
}

/// True when the caller may see decrypted secret material (SECRET on
/// SECRET_VARIABLE). Config-context `data` can carry inline connection
/// credentials; reads redact those for everyone else (READ on CONFIG_CONTEXT no
/// longer exposes plaintext secrets). Errors fail closed.
async fn caller_can_see_secrets(state: &AppState, request_context: &RequestContext) -> bool {
    matches!(
        state
            .services
            .access_control
            .check_secret_visibility(request_context)
            .await,
        Ok(PolicyDecision::Allow)
    )
}

/// List config contexts.
#[utoipa::path(
    get,
    path = "/api/v1/config-contexts",
    tag = "config-contexts",
    params(ConfigContextListQuery),
    responses(
        (status = 200, description = "Config contexts", body = ConfigContextListResponse),
        (status = "4XX", description = "Config context list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_config_contexts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConfigContextListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let service = state.services.config_queries.clone();
    let name_filter = query
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let (limit, offset) = normalize_page(query.limit, query.offset);
    let is_active = parse_csv_strings(query.is_active.as_deref());
    let invalid_status = is_active
        .iter()
        .find(|value| value.as_str() != "true" && value.as_str() != "false");
    if let Some(value) = invalid_status {
        return handle_app_error(AppError::Validation(format!(
            "Invalid is_active value {value:?}: expected true or false"
        )));
    }

    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONFIG_CONTEXT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let page_result = match (&query.scope_type, &query.scope_id) {
        (Some(scope_type), Some(scope_id)) => {
            let scope_type = match parse_required_string(scope_type, "scope_type") {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            };
            let scope_id = match parse_uuid(scope_id, "scope_id") {
                Ok(value) => value,
                Err(error) => return handle_app_error(error),
            };
            let include_hierarchy = query.include_hierarchy.unwrap_or(false);
            // The scope-resolution path is ordered by configuration precedence
            // (weight + scope hierarchy); an allowlisted `sort` only applies to
            // the plain list branch below.
            service
                .list_contexts_for_scope_page(ConfigContextScopePageRequest {
                    scope_type: &scope_type,
                    scope_id,
                    include_hierarchy,
                    name: name_filter.as_deref(),
                    is_active: &is_active,
                    limit,
                    offset,
                })
                .await
        }
        (None, None) => {
            service
                .list_contexts_page(
                    name_filter.as_deref(),
                    &is_active,
                    sort.as_ref(),
                    limit,
                    offset,
                )
                .await
        }
        _ => {
            return handle_app_error(AppError::Validation(
                "Provide both scope_type and scope_id".to_string(),
            ));
        }
    };

    match page_result {
        Ok(mut page) => {
            if !caller_can_see_secrets(&state, &request_context).await {
                for item in &mut page.items {
                    let presence = compute_secret_presence(&item.data, &HashMap::new());
                    redact_secret_fields(&mut item.data, &presence);
                }
            }
            (
                StatusCode::OK,
                Json(ConfigContextListResponse {
                    items: page.items,
                    total: page.total,
                    limit: page.limit,
                    offset: page.offset,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a config context.
#[utoipa::path(
    get,
    path = "/api/v1/config-contexts/{id}",
    tag = "config-contexts",
    params(
        ("id" = String, Path, description = "Config context id"),
    ),
    responses(
        (status = 200, description = "Config context", body = ConfigContext),
        (status = 404, description = "Config context not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Config context request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_config_context(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let context_id = match parse_uuid(&id, "context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.config_queries.clone();
    match service.get_context(context_id).await {
        Ok(Some(mut context)) => {
            if !caller_can_see_secrets(&state, &request_context).await {
                let presence = compute_secret_presence(&context.data, &HashMap::new());
                redact_secret_fields(&mut context.data, &presence);
            }
            (StatusCode::OK, Json(context)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Config context not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a config context.
#[utoipa::path(
    post,
    path = "/api/v1/config-contexts",
    tag = "config-contexts",
    request_body = ConfigContextPayload,
    responses(
        (status = 201, description = "Config context created", body = ConfigContext),
        (status = 422, description = "Config context payload failed schema validation", body = ValidationErrorResponse),
        (status = "4XX", description = "Config context create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_config_context(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ConfigContextPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if has_connection_params(&payload.data)
        && let Err(errors) = validate_connection_params(&payload.data)
    {
        warn!(
            target: "piping_backend::api::rest::config_contexts",
            request_id = %request_id.0,
            violations = errors.len(),
            validation_errors = ?errors,
            "connection params schema validation rejected create payload"
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ValidationErrorResponse::from(errors)),
        )
            .into_response();
    }

    let context = ConfigContext {
        id: Uuid::now_v7(),
        name,
        description: payload.description,
        weight: payload.weight,
        data: payload.data,
        is_active: payload.is_active.unwrap_or(true),
        created_at: None,
        updated_at: None,
    };

    let service = state.services.config_queries.clone();
    match service.create_context(context).await {
        Ok(created) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a config context.
#[utoipa::path(
    put,
    path = "/api/v1/config-contexts/{id}",
    tag = "config-contexts",
    params(
        ("id" = String, Path, description = "Config context id"),
    ),
    request_body = ConfigContextPayload,
    responses(
        (status = 200, description = "Config context updated", body = ConfigContext),
        (status = 422, description = "Config context payload failed schema validation", body = ValidationErrorResponse),
        (status = "4XX", description = "Config context update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_config_context(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ConfigContextPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let context_id = match parse_uuid(&id, "context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if has_connection_params(&payload.data)
        && let Err(errors) = validate_connection_params(&payload.data)
    {
        warn!(
            target: "piping_backend::api::rest::config_contexts",
            request_id = %request_id.0,
            violations = errors.len(),
            validation_errors = ?errors,
            "connection params schema validation rejected update payload"
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ValidationErrorResponse::from(errors)),
        )
            .into_response();
    }

    let context = ConfigContext {
        id: context_id,
        name,
        description: payload.description,
        weight: payload.weight,
        data: payload.data,
        is_active: payload.is_active.unwrap_or(true),
        created_at: None,
        updated_at: None,
    };

    let service = state.services.config_queries.clone();
    match service.update_context(context).await {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a config context.
#[utoipa::path(
    delete,
    path = "/api/v1/config-contexts/{id}",
    tag = "config-contexts",
    params(
        ("id" = String, Path, description = "Config context id"),
    ),
    responses(
        (status = 204, description = "Config context deleted"),
        (status = 404, description = "Config context not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Config context delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_config_context(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let context_id = match parse_uuid(&id, "context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.config_queries.clone();
    match service.delete_context(context_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Config context not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// List config-context bindings.
#[utoipa::path(
    get,
    path = "/api/v1/config-context-bindings",
    tag = "config-contexts",
    params(ConfigContextBindingListQuery),
    responses(
        (status = 200, description = "Config context bindings", body = ConfigContextBindingListResponse),
        (status = "4XX", description = "Config context binding list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context binding list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_config_context_bindings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConfigContextBindingListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let context_id = match parse_uuid(&query.context_id, "context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let (limit, offset) = normalize_page(query.limit, query.offset);
    let filters = ConfigContextBindingListFilters {
        scope_type: parse_csv_strings(query.scope_type.as_deref()),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONFIG_CONTEXT_BINDING_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.config_queries.clone();
    match service
        .list_context_bindings_page(context_id, &filters, sort.as_ref(), limit, offset)
        .await
    {
        Ok(page) => (
            StatusCode::OK,
            Json(ConfigContextBindingListResponse {
                items: page.items,
                total: page.total,
                limit: page.limit,
                offset: page.offset,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bind a config context to a scope.
#[utoipa::path(
    post,
    path = "/api/v1/config-context-bindings",
    tag = "config-contexts",
    request_body = ConfigContextBindingPayload,
    responses(
        (status = 201, description = "Config context binding created", body = ConfigContextBinding),
        (status = 404, description = "Config context not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Config context binding create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context binding create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_config_context_binding(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ConfigContextBindingPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let context_id = match parse_uuid(&payload.context_id, "context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_id = match parse_uuid(&payload.scope_id, "scope_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_type = match parse_required_string(&payload.scope_type, "scope_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.config_queries.clone();
    match service.get_context(context_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Config context not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    }

    let binding = ConfigContextBinding {
        id: Uuid::now_v7(),
        context_id,
        scope_type,
        scope_id,
    };

    match service.create_context_binding(binding).await {
        Ok(created) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Remove a config-context binding.
#[utoipa::path(
    delete,
    path = "/api/v1/config-context-bindings/{id}",
    tag = "config-contexts",
    params(
        ("id" = String, Path, description = "Config context binding id"),
    ),
    responses(
        (status = 204, description = "Config context binding deleted"),
        (status = 404, description = "Config context binding not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Config context binding delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Config context binding delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_config_context_binding(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::ConfigContext,
    )
    .await
    {
        return handle_app_error(error);
    }

    let binding_id = match parse_uuid(&id, "binding_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.config_queries.clone();
    match service.delete_context_binding(binding_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Config context binding not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}
