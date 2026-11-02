use std::collections::BTreeMap;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::permission_helpers::ensure_access;
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::connection_config::ConnConfScope;
use crate::contexts::connections::application::connection_service::{
    CreateProxyNodeAssignmentInput, CreateProxyNodeInput, CreatedProxyNode,
    ProxyNodeBootstrap, UpdateProxyNodeAssignmentInput, UpdateProxyNodeInput,
};
use crate::contexts::connections::domain::{ProxyNode, ProxyNodeAssignment, TargetProxyAssignment};
use crate::contexts::connections::infrastructure::{
    PROXY_NODE_ASSIGNMENT_COLUMNS, ProxyNodeAssignmentFilter, ProxyNodeFilter,
};
use crate::contexts::connections::infrastructure::persistence::repositories::connection_repository::PROXY_NODE_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::api::rest::routes::validation::{parse_uuid, parse_uuid_or_blank};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ProxyNodeListQuery {
    /// Comma-separated enabled flags (`true`/`false`); matches any (`IN`).
    pub enabled: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ProxyNodeAssignmentListQuery {
    /// Comma-separated proxy node UUIDs; matches any (`IN`).
    pub proxy_node_id: Option<String>,
    /// Comma-separated assignment scope types; matches any (`IN`).
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    /// Comma-separated enabled flags (`true`/`false`); matches any (`IN`).
    pub enabled: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateProxyNodeRequest {
    pub name: String,
    pub slug: String,
    pub base_url: String,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateProxyNodeRequest {
    pub name: String,
    pub slug: String,
    pub base_url: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateProxyNodeAssignmentRequest {
    pub proxy_node_id: String,
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateProxyNodeAssignmentRequest {
    #[serde(default)]
    pub proxy_node_id: Option<String>,
    #[serde(default)]
    pub scope_type: Option<String>,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertTargetProxyAssignmentRequest {
    pub proxy_node_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProxyNodeResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub base_url: String,
    pub enabled: bool,
    pub secret_version: i32,
    pub secret_rotated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProxyNodeBootstrapResponse {
    pub node_id: String,
    pub node_slug: String,
    pub node_secret: String,
    pub env: BTreeMap<String, String>,
    pub docker_run_command: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedProxyNodeResponse {
    pub node: ProxyNodeResponse,
    pub bootstrap: ProxyNodeBootstrapResponse,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProxyNodeListResponse {
    pub items: Vec<ProxyNodeResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProxyNodeAssignmentResponse {
    pub id: String,
    pub proxy_node_id: String,
    pub proxy_node_name: Option<String>,
    pub proxy_node_slug: Option<String>,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProxyNodeAssignmentListResponse {
    pub items: Vec<ProxyNodeAssignmentResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TargetProxyAssignmentResponse {
    pub target_id: String,
    pub proxy_node_id: String,
    pub proxy_node_name: Option<String>,
    pub proxy_node_slug: Option<String>,
    pub updated_at: String,
}

/// List proxy nodes.
#[utoipa::path(
    get,
    path = "/api/v1/admin/proxy-nodes",
    tag = "proxy-nodes",
    params(ProxyNodeListQuery),
    responses(
        (status = 200, description = "Proxy nodes", body = ProxyNodeListResponse),
        (status = 403, description = "Proxy node access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_proxy_nodes(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ProxyNodeListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let enabled = parse_enabled_flags(query.enabled.as_deref());
    let enabled = match enabled {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PROXY_NODE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = ProxyNodeFilter {
        enabled,
        search: query.search,
        sort,
    };
    let service = state.services.connection.clone();
    let items = match service
        .list_proxy_nodes(filter.clone(), Some(limit), Some(offset))
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let total = match service.count_proxy_nodes(filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let response = ProxyNodeListResponse {
        items: items.into_iter().map(map_proxy_node).collect(),
        total,
        limit,
        offset,
    };
    (StatusCode::OK, Json(response)).into_response()
}

/// Get a proxy node.
#[utoipa::path(
    get,
    path = "/api/v1/admin/proxy-nodes/{id}",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node UUID"),
    ),
    responses(
        (status = 200, description = "Proxy node", body = ProxyNodeResponse),
        (status = 400, description = "Invalid proxy node UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_proxy_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(proxy_node_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let proxy_node_id = match parse_uuid(&proxy_node_id, "proxy node id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    match service.get_proxy_node(proxy_node_id).await {
        Ok(Some(proxy_node)) => (StatusCode::OK, Json(map_proxy_node(proxy_node))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Proxy node not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Register a proxy node.
#[utoipa::path(
    post,
    path = "/api/v1/admin/proxy-nodes",
    tag = "proxy-nodes",
    request_body = CreateProxyNodeRequest,
    responses(
        (status = 201, description = "Proxy node created with one-time bootstrap secret", body = CreatedProxyNodeResponse),
        (status = 400, description = "Invalid proxy node payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_proxy_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateProxyNodeRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let service = state.services.connection.clone();
    let input = CreateProxyNodeInput {
        name: payload.name,
        slug: payload.slug,
        base_url: payload.base_url,
        enabled: payload.enabled.unwrap_or(true),
    };
    match service.create_proxy_node_with_bootstrap(input).await {
        Ok(created) => (StatusCode::CREATED, Json(map_created_proxy_node(created))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Rotate proxy node secret and return a one-time bootstrap command.
#[utoipa::path(
    post,
    path = "/api/v1/admin/proxy-nodes/{id}/rotate-secret",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node UUID"),
    ),
    responses(
        (status = 200, description = "Proxy node secret rotated", body = CreatedProxyNodeResponse),
        (status = 400, description = "Invalid proxy node UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn rotate_proxy_node_secret(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(proxy_node_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let proxy_node_id = match parse_uuid(&proxy_node_id, "proxy node id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    match service.rotate_proxy_node_secret(proxy_node_id).await {
        Ok(created) => (StatusCode::OK, Json(map_created_proxy_node(created))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a proxy node.
#[utoipa::path(
    put,
    path = "/api/v1/admin/proxy-nodes/{id}",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node UUID"),
    ),
    request_body = UpdateProxyNodeRequest,
    responses(
        (status = 200, description = "Proxy node updated", body = ProxyNodeResponse),
        (status = 400, description = "Invalid proxy node payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_proxy_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(proxy_node_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateProxyNodeRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let proxy_node_id = match parse_uuid(&proxy_node_id, "proxy node id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    let input = UpdateProxyNodeInput {
        name: payload.name,
        slug: payload.slug,
        base_url: payload.base_url,
        enabled: payload.enabled,
    };
    match service.update_proxy_node(proxy_node_id, input).await {
        Ok(proxy_node) => (StatusCode::OK, Json(map_proxy_node(proxy_node))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a proxy node.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/proxy-nodes/{id}",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node UUID"),
    ),
    responses(
        (status = 204, description = "Proxy node deleted"),
        (status = 400, description = "Invalid proxy node UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Proxy node is still referenced by assignments (DELETE_BLOCKED with blocker details)", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_proxy_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(proxy_node_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let proxy_node_id = match parse_uuid(&proxy_node_id, "proxy node id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    match service.delete_proxy_node(proxy_node_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Proxy node '{proxy_node_id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// List proxy node assignments.
#[utoipa::path(
    get,
    path = "/api/v1/admin/proxy-node-assignments",
    tag = "proxy-nodes",
    params(ProxyNodeAssignmentListQuery),
    responses(
        (status = 200, description = "Proxy node assignments", body = ProxyNodeAssignmentListResponse),
        (status = 403, description = "Proxy node assignment access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_proxy_node_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ProxyNodeAssignmentListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let proxy_node_id = match parse_csv_uuids(query.proxy_node_id.as_deref(), "proxy_node_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_type = match parse_csv_scopes(query.scope_type.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_id = match parse_uuid_or_blank(query.scope_id, "scope_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let enabled = parse_enabled_flags(query.enabled.as_deref());
    let enabled = match enabled {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PROXY_NODE_ASSIGNMENT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let filter = ProxyNodeAssignmentFilter {
        proxy_node_id,
        scope_type,
        scope_id,
        enabled,
        sort,
    };
    let service = state.services.connection.clone();
    let assignments = match service
        .list_proxy_node_assignments(filter.clone(), Some(limit), Some(offset))
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let total = match service.count_proxy_node_assignments(filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mut items = Vec::with_capacity(assignments.len());
    for assignment in assignments {
        match map_proxy_node_assignment(&state, assignment).await {
            Ok(item) => items.push(item),
            Err(error) => return handle_app_error(error),
        }
    }
    (
        StatusCode::OK,
        Json(ProxyNodeAssignmentListResponse {
            items,
            total,
            limit,
            offset,
        }),
    )
        .into_response()
}

/// Create a proxy node assignment.
#[utoipa::path(
    post,
    path = "/api/v1/admin/proxy-node-assignments",
    tag = "proxy-nodes",
    request_body = CreateProxyNodeAssignmentRequest,
    responses(
        (status = 201, description = "Proxy node assignment created", body = ProxyNodeAssignmentResponse),
        (status = 400, description = "Invalid proxy node assignment payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node assignment access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_proxy_node_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateProxyNodeAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let proxy_node_id = match parse_uuid(&payload.proxy_node_id, "proxy_node_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_type = match parse_scope(&payload.scope_type) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_id = match parse_uuid_or_blank(payload.scope_id, "scope_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.connection.clone();
    let input = CreateProxyNodeAssignmentInput {
        proxy_node_id,
        scope_type,
        scope_id,
        weight: payload.weight,
        enabled: payload.enabled,
    };
    match service.create_proxy_node_assignment(input).await {
        Ok(assignment) => match map_proxy_node_assignment(&state, assignment).await {
            Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

/// Get a proxy node assignment.
#[utoipa::path(
    get,
    path = "/api/v1/admin/proxy-node-assignments/{id}",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node assignment UUID"),
    ),
    responses(
        (status = 200, description = "Proxy node assignment", body = ProxyNodeAssignmentResponse),
        (status = 400, description = "Invalid proxy node assignment UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node assignment access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_proxy_node_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(assignment_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let assignment_id = match parse_uuid(&assignment_id, "proxy node assignment id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    match service.get_proxy_node_assignment(assignment_id).await {
        Ok(Some(assignment)) => match map_proxy_node_assignment(&state, assignment).await {
            Ok(response) => (StatusCode::OK, Json(response)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound(
            "Proxy node assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Update a proxy node assignment.
#[utoipa::path(
    put,
    path = "/api/v1/admin/proxy-node-assignments/{id}",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node assignment UUID"),
    ),
    request_body = UpdateProxyNodeAssignmentRequest,
    responses(
        (status = 200, description = "Proxy node assignment updated", body = ProxyNodeAssignmentResponse),
        (status = 400, description = "Invalid proxy node assignment payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node assignment access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_proxy_node_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(assignment_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateProxyNodeAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let assignment_id = match parse_uuid(&assignment_id, "proxy node assignment id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let proxy_node_id = match parse_uuid_or_blank(payload.proxy_node_id, "proxy_node_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_type = match payload.scope_type.as_deref().map(parse_scope).transpose() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let scope_id = match parse_uuid_or_blank(payload.scope_id, "scope_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    let input = UpdateProxyNodeAssignmentInput {
        proxy_node_id,
        scope_type,
        scope_id,
        weight: payload.weight,
        enabled: payload.enabled,
    };
    match service
        .update_proxy_node_assignment(assignment_id, input)
        .await
    {
        Ok(assignment) => match map_proxy_node_assignment(&state, assignment).await {
            Ok(response) => (StatusCode::OK, Json(response)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

/// Delete a proxy node assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/proxy-node-assignments/{id}",
    tag = "proxy-nodes",
    params(
        ("id" = String, Path, description = "Proxy node assignment UUID"),
    ),
    responses(
        (status = 204, description = "Proxy node assignment deleted"),
        (status = 400, description = "Invalid proxy node assignment UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Proxy node assignment access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Proxy node assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_proxy_node_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(assignment_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::ProxyNode,
    )
    .await
    {
        return handle_app_error(error);
    }

    let assignment_id = match parse_uuid(&assignment_id, "proxy node assignment id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    match service.delete_proxy_node_assignment(assignment_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get the proxy assigned to a target.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/targets/{id}/proxy-assignment",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory target UUID"),
    ),
    responses(
        (status = 200, description = "Target proxy assignment", body = Option<TargetProxyAssignmentResponse>),
        (status = 400, description = "Invalid target UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Target access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_target_proxy_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let target_id = match parse_uuid(&target_id, "target id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_target_access(&state, &request_context, Action::Read, target_id).await
    {
        return handle_app_error(error);
    }

    let service = state.services.connection.clone();
    let assignment = match service.get_target_proxy_assignment(target_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match assignment {
        Some(assignment) => match map_target_assignment_with_proxy_name(&state, assignment).await {
            Ok(response) => (StatusCode::OK, Json(Some(response))).into_response(),
            Err(error) => handle_app_error(error),
        },
        None => (
            StatusCode::OK,
            Json(Option::<TargetProxyAssignmentResponse>::None),
        )
            .into_response(),
    }
}

/// Assign a proxy to a target.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/targets/{id}/proxy-assignment",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory target UUID"),
    ),
    request_body = UpsertTargetProxyAssignmentRequest,
    responses(
        (status = 200, description = "Target proxy assignment updated", body = TargetProxyAssignmentResponse),
        (status = 204, description = "Target proxy assignment cleared"),
        (status = 400, description = "Invalid target or proxy node UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Target access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn upsert_target_proxy_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpsertTargetProxyAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let target_id = match parse_uuid(&target_id, "target id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_target_access(&state, &request_context, Action::Update, target_id).await
    {
        return handle_app_error(error);
    }

    let service = state.services.connection.clone();
    if payload.proxy_node_id.is_none() {
        return match service.clear_target_proxy_assignment(target_id).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => handle_app_error(error),
        };
    }
    let Some(proxy_node_id) = payload.proxy_node_id.as_ref() else {
        return handle_app_error(AppError::Validation(
            "proxy node id is required".to_string(),
        ));
    };
    let proxy_node_id = match parse_uuid(proxy_node_id, "proxy node id") {
        Ok(parsed) => parsed,
        Err(error) => return handle_app_error(error),
    };

    let assignment = match service
        .set_target_proxy_assignment(target_id, proxy_node_id)
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match map_target_assignment_with_proxy_name(&state, assignment).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Clear a target's proxy assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/targets/{id}/proxy-assignment",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory target UUID"),
    ),
    responses(
        (status = 204, description = "Target proxy assignment cleared"),
        (status = 400, description = "Invalid target UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Target access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn clear_target_proxy_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let target_id = match parse_uuid(&target_id, "target id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_target_access(&state, &request_context, Action::Update, target_id).await
    {
        return handle_app_error(error);
    }

    let service = state.services.connection.clone();
    match service.clear_target_proxy_assignment(target_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

async fn map_target_assignment_with_proxy_name(
    state: &AppState,
    assignment: TargetProxyAssignment,
) -> Result<TargetProxyAssignmentResponse, AppError> {
    let service = state.services.connection.clone();
    let proxy_node = service.get_proxy_node(assignment.proxy_node_id).await?;
    Ok(TargetProxyAssignmentResponse {
        target_id: assignment.target_id.to_string(),
        proxy_node_id: assignment.proxy_node_id.to_string(),
        proxy_node_name: proxy_node.as_ref().map(|value| value.name.clone()),
        proxy_node_slug: proxy_node.as_ref().map(|value| value.slug.clone()),
        updated_at: assignment.updated_at.to_rfc3339(),
    })
}

async fn map_proxy_node_assignment(
    state: &AppState,
    assignment: ProxyNodeAssignment,
) -> Result<ProxyNodeAssignmentResponse, AppError> {
    let service = state.services.connection.clone();
    let proxy_node = service.get_proxy_node(assignment.proxy_node_id).await?;
    Ok(ProxyNodeAssignmentResponse {
        id: assignment.id.to_string(),
        proxy_node_id: assignment.proxy_node_id.to_string(),
        proxy_node_name: proxy_node.as_ref().map(|value| value.name.clone()),
        proxy_node_slug: proxy_node.as_ref().map(|value| value.slug.clone()),
        scope_type: assignment.scope_type.as_str().to_string(),
        scope_id: assignment.scope_id.map(|value| value.to_string()),
        weight: assignment.weight,
        enabled: assignment.enabled,
        created_at: assignment.created_at.to_rfc3339(),
        updated_at: assignment.updated_at.to_rfc3339(),
    })
}

fn map_proxy_node(proxy_node: ProxyNode) -> ProxyNodeResponse {
    ProxyNodeResponse {
        id: proxy_node.id.to_string(),
        name: proxy_node.name,
        slug: proxy_node.slug,
        base_url: proxy_node.base_url,
        enabled: proxy_node.enabled,
        secret_version: proxy_node.secret_version,
        secret_rotated_at: proxy_node.secret_rotated_at.map(|value| value.to_rfc3339()),
        created_at: proxy_node.created_at.to_rfc3339(),
        updated_at: proxy_node.updated_at.to_rfc3339(),
    }
}

fn map_proxy_node_bootstrap(bootstrap: ProxyNodeBootstrap) -> ProxyNodeBootstrapResponse {
    ProxyNodeBootstrapResponse {
        node_id: bootstrap.node_id,
        node_slug: bootstrap.node_slug,
        node_secret: bootstrap.node_secret,
        env: bootstrap.env,
        docker_run_command: bootstrap.docker_run_command,
    }
}

fn map_created_proxy_node(created: CreatedProxyNode) -> CreatedProxyNodeResponse {
    CreatedProxyNodeResponse {
        node: map_proxy_node(created.node),
        bootstrap: map_proxy_node_bootstrap(created.bootstrap),
    }
}

fn parse_scope(raw: &str) -> Result<ConnConfScope, AppError> {
    ConnConfScope::parse(raw)
        .ok_or_else(|| AppError::Validation(format!("unknown scope_type '{}'", raw)))
}

/// Parse a comma-separated list of assignment scope types into a deduplicated
/// vector. Empty/whitespace entries are skipped; an unknown scope is a client
/// error.
fn parse_csv_scopes(raw: Option<&str>) -> Result<Vec<ConnConfScope>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<ConnConfScope> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let scope = parse_scope(trimmed)?;
        if !out.contains(&scope) {
            out.push(scope);
        }
    }
    Ok(out)
}

fn parse_enabled_flags(raw: Option<&str>) -> Result<Vec<String>, AppError> {
    let enabled = parse_csv_strings(raw);
    let invalid = enabled
        .iter()
        .find(|value| value.as_str() != "true" && value.as_str() != "false");
    if let Some(value) = invalid {
        return Err(AppError::Validation(format!(
            "Invalid enabled value {value:?}: expected true or false"
        )));
    }
    Ok(enabled)
}

fn target_resource_id(target_id: Uuid) -> String {
    format!("connection_target:{}", target_id)
}

/// Per-target authorization with the target's inventory facets attached, so
/// attribute-constrained ConnectionTarget policies (site/role/tenant/…) scope
/// proxy-assignment reads and updates too. No protocol — proxy
/// assignment is not protocol-specific.
async fn ensure_target_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    target_id: Uuid,
) -> Result<(), AppError> {
    let attributes = connection_target_authz_attributes_for(state, target_id, None).await?;
    let resource_id = target_resource_id(target_id);
    build_policy_evaluation_service(state)
        .ensure_access_with_attributes(
            request_context,
            action,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
            Some(attributes),
        )
        .await
}
