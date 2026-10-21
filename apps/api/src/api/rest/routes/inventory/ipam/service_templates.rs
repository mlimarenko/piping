use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ServiceTemplate;
use crate::contexts::inventory::infrastructure::persistence::{
    SERVICE_TEMPLATE_COLUMNS, ServiceTemplateFilter, ServiceTemplateRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ServiceTemplateListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated service protocols; matches any (`IN`).
    pub protocol: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct ServiceTemplatePayload {
    pub name: String,
    pub protocol: String,
    pub ports: Vec<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

struct ParsedServiceTemplate {
    name: String,
    protocol: String,
}

fn parse_service_template_payload(
    payload: &ServiceTemplatePayload,
) -> Result<ParsedServiceTemplate, AppError> {
    Ok(ParsedServiceTemplate {
        name: parse_required_string(&payload.name, "name")?,
        protocol: parse_required_string(&payload.protocol, "protocol")?,
    })
}

fn build_service_template(
    id: Uuid,
    parsed: ParsedServiceTemplate,
    payload: ServiceTemplatePayload,
) -> ServiceTemplate {
    ServiceTemplate {
        id,
        name: parsed.name,
        protocol: parsed.protocol,
        ports: payload.ports,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List service templates.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/service-templates",
    operation_id = "list_inventory_service_templates",
    tag = "inventory",
    params(ServiceTemplateListQuery),
    responses(
        (status = 200, description = "Service templates", body = InventoryListEnvelope<ServiceTemplate>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_service_templates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ServiceTemplateListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        SERVICE_TEMPLATE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let filter = ServiceTemplateFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        protocol: parse_csv_strings(query.protocol.as_deref()),
        sort,
    };
    match state
        .repositories
        .service_templates
        .list_paginated(query.limit, query.offset, filter)
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(InventoryListEnvelope {
                items,
                total,
                limit: resolved_limit,
                offset: resolved_offset,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a service template.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/service-templates/{id}",
    operation_id = "get_inventory_service_template",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Service template ID"),
    ),
    responses(
        (status = 200, description = "Service template", body = ServiceTemplate),
        (status = 404, description = "Service template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_service_template(
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
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let entity_id = match parse_uuid(&id, "service_template_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .service_templates
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Service template not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a service template.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/service-templates",
    operation_id = "create_inventory_service_template",
    tag = "inventory",
    request_body = ServiceTemplatePayload,
    responses(
        (status = 201, description = "Service template created", body = ServiceTemplate),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_service_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ServiceTemplatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let parsed = match parse_service_template_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_service_template(Uuid::now_v7(), parsed, payload);
    match state.repositories.service_templates.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create service templates.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/service-templates/bulk",
    operation_id = "bulk_create_inventory_service_templates",
    tag = "inventory",
    request_body = BulkCreateRequest<ServiceTemplatePayload>,
    responses(
        (status = 200, description = "Bulk service template create result", body = BulkCreateResponse<ServiceTemplate>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_service_templates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<ServiceTemplatePayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let mut created = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        let parsed = match parse_service_template_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_service_template(Uuid::now_v7(), parsed, payload);

        match state.repositories.service_templates.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a service template.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/service-templates/{id}",
    operation_id = "update_inventory_service_template",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Service template ID"),
    ),
    request_body = ServiceTemplatePayload,
    responses(
        (status = 200, description = "Service template updated", body = ServiceTemplate),
        (status = 404, description = "Service template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_service_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ServiceTemplatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let entity_id = match parse_uuid(&id, "service_template_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_service_template_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .service_templates
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Service template not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_service_template(entity_id, parsed, payload);
    match state.repositories.service_templates.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a service template.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/service-templates/{id}",
    operation_id = "delete_inventory_service_template",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Service template ID"),
    ),
    responses(
        (status = 204, description = "Service template deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_service_template(
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
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let entity_id = match parse_uuid(&id, "service_template_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .service_templates
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Service template not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.service_templates.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
