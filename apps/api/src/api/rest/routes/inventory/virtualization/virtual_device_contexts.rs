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
use crate::contexts::inventory::domain::VirtualDeviceContext;
use crate::contexts::inventory::infrastructure::persistence::{
    VIRTUAL_DEVICE_CONTEXT_COLUMNS, VirtualDeviceContextFilter, VirtualDeviceContextRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VirtualDeviceContextListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated device ids; matches any (`IN`).
    pub device_id: Option<String>,
    /// Comma-separated IPv4 address ids; matches any (`IN`).
    pub primary_ip4_id: Option<String>,
    /// Comma-separated IPv6 address ids; matches any (`IN`).
    pub primary_ip6_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct VirtualDeviceContextPayload {
    pub name: String,
    pub status: String,
    pub identifier: Option<i16>,
    pub device_id: Option<String>,
    pub primary_ip4_id: Option<String>,
    pub primary_ip6_id: Option<String>,
    pub tenant_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

struct ParsedVirtualDeviceContext {
    name: String,
    status: String,
    device_id: Option<Uuid>,
    primary_ip4_id: Option<Uuid>,
    primary_ip6_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
}

fn parse_virtual_device_context_payload(
    payload: &VirtualDeviceContextPayload,
) -> Result<ParsedVirtualDeviceContext, AppError> {
    Ok(ParsedVirtualDeviceContext {
        name: parse_required_string(&payload.name, "name")?,
        status: parse_required_string(&payload.status, "status")?,
        device_id: parse_optional_uuid(payload.device_id.clone(), "device_id")?,
        primary_ip4_id: parse_optional_uuid(payload.primary_ip4_id.clone(), "primary_ip4_id")?,
        primary_ip6_id: parse_optional_uuid(payload.primary_ip6_id.clone(), "primary_ip6_id")?,
        tenant_id: parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?,
    })
}

fn build_virtual_device_context(
    id: Uuid,
    parsed: ParsedVirtualDeviceContext,
    payload: VirtualDeviceContextPayload,
) -> VirtualDeviceContext {
    VirtualDeviceContext {
        id,
        name: parsed.name,
        status: parsed.status,
        identifier: payload.identifier,
        device_id: parsed.device_id,
        primary_ip4_id: parsed.primary_ip4_id,
        primary_ip6_id: parsed.primary_ip6_id,
        tenant_id: parsed.tenant_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List virtual device contexts.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-device-contexts",
    operation_id = "list_inventory_virtual_device_contexts",
    tag = "inventory",
    params(VirtualDeviceContextListQuery),
    responses(
        (status = 200, description = "Virtual device contexts", body = InventoryListEnvelope<VirtualDeviceContext>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_device_contexts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualDeviceContextListQuery>,
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
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VIRTUAL_DEVICE_CONTEXT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let device_id = match parse_csv_uuids(query.device_id.as_deref(), "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let primary_ip4_id = match parse_csv_uuids(query.primary_ip4_id.as_deref(), "primary_ip4_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let primary_ip6_id = match parse_csv_uuids(query.primary_ip6_id.as_deref(), "primary_ip6_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = VirtualDeviceContextFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        status: parse_csv_strings(query.status.as_deref()),
        device_id,
        primary_ip4_id,
        primary_ip6_id,
        tenant_id,
        sort,
    };
    match state
        .repositories
        .virtual_device_contexts
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

/// Get a virtual device context.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-device-contexts/{id}",
    operation_id = "get_inventory_virtual_device_context",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual device context ID"),
    ),
    responses(
        (status = 200, description = "Virtual device context", body = VirtualDeviceContext),
        (status = 404, description = "Virtual device context not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_device_context(
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
    let entity_id = match parse_uuid(&id, "virtual_device_context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_device_contexts
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Virtual device context not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a virtual device context.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-device-contexts",
    operation_id = "create_inventory_virtual_device_context",
    tag = "inventory",
    request_body = VirtualDeviceContextPayload,
    responses(
        (status = 201, description = "Virtual device context created", body = VirtualDeviceContext),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_device_context(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualDeviceContextPayload>,
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
    let parsed = match parse_virtual_device_context_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_virtual_device_context(Uuid::now_v7(), parsed, payload);
    match state
        .repositories
        .virtual_device_contexts
        .create(&item)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create virtual device contexts.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-device-contexts/bulk",
    operation_id = "bulk_create_inventory_virtual_device_contexts",
    tag = "inventory",
    request_body = BulkCreateRequest<VirtualDeviceContextPayload>,
    responses(
        (status = 200, description = "Bulk virtual device context create result", body = BulkCreateResponse<VirtualDeviceContext>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_device_contexts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<VirtualDeviceContextPayload>>,
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
        let parsed = match parse_virtual_device_context_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_virtual_device_context(Uuid::now_v7(), parsed, payload);

        match state
            .repositories
            .virtual_device_contexts
            .create(&item)
            .await
        {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a virtual device context.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual-device-contexts/{id}",
    operation_id = "update_inventory_virtual_device_context",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual device context ID"),
    ),
    request_body = VirtualDeviceContextPayload,
    responses(
        (status = 200, description = "Virtual device context updated", body = VirtualDeviceContext),
        (status = 404, description = "Virtual device context not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_device_context(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualDeviceContextPayload>,
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
    let entity_id = match parse_uuid(&id, "virtual_device_context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_virtual_device_context_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .virtual_device_contexts
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Virtual device context not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_virtual_device_context(entity_id, parsed, payload);
    match state
        .repositories
        .virtual_device_contexts
        .update(&item)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a virtual device context.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual-device-contexts/{id}",
    operation_id = "delete_inventory_virtual_device_context",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual device context ID"),
    ),
    responses(
        (status = 204, description = "Virtual device context deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_device_context(
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
    let entity_id = match parse_uuid(&id, "virtual_device_context_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_device_contexts
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Virtual device context not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_device_contexts
        .delete(entity_id)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
