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
use crate::contexts::inventory::domain::VirtualCircuitType;
use crate::contexts::inventory::infrastructure::persistence::{
    VIRTUAL_CIRCUIT_TYPE_COLUMNS, VirtualCircuitTypeRepository,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VirtualCircuitTypeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct VirtualCircuitTypePayload {
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

struct ParsedVirtualCircuitType {
    name: String,
    slug: String,
}

fn parse_virtual_circuit_type_payload(
    payload: &VirtualCircuitTypePayload,
) -> Result<ParsedVirtualCircuitType, AppError> {
    Ok(ParsedVirtualCircuitType {
        name: parse_required_string(&payload.name, "name")?,
        slug: parse_required_string(&payload.slug, "slug")?,
    })
}

fn build_virtual_circuit_type(
    id: Uuid,
    parsed: ParsedVirtualCircuitType,
    payload: VirtualCircuitTypePayload,
) -> VirtualCircuitType {
    VirtualCircuitType {
        id,
        name: parsed.name,
        slug: parsed.slug,
        color: payload.color.unwrap_or_default(),
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        custom_fields: payload.custom_fields,
    }
}

/// List virtual circuit types.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-circuit-types",
    operation_id = "list_inventory_virtual_circuit_types",
    tag = "inventory",
    params(VirtualCircuitTypeListQuery),
    responses(
        (status = 200, description = "Virtual circuit types", body = InventoryListEnvelope<VirtualCircuitType>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_circuit_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualCircuitTypeListQuery>,
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
        VIRTUAL_CIRCUIT_TYPE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .virtual_circuit_types
        .list_paginated(
            query.limit,
            query.offset,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
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

/// Get a virtual circuit type.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-circuit-types/{id}",
    operation_id = "get_inventory_virtual_circuit_type",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual circuit type ID"),
    ),
    responses(
        (status = 200, description = "Virtual circuit type", body = VirtualCircuitType),
        (status = 404, description = "Virtual circuit type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_circuit_type(
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
    let entity_id = match parse_uuid(&id, "virtual_circuit_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_circuit_types
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Virtual circuit type not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a virtual circuit type.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-circuit-types",
    operation_id = "create_inventory_virtual_circuit_type",
    tag = "inventory",
    request_body = VirtualCircuitTypePayload,
    responses(
        (status = 201, description = "Virtual circuit type created", body = VirtualCircuitType),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_circuit_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualCircuitTypePayload>,
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
    let parsed = match parse_virtual_circuit_type_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_virtual_circuit_type(Uuid::now_v7(), parsed, payload);
    match state.repositories.virtual_circuit_types.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create virtual circuit types.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-circuit-types/bulk",
    operation_id = "bulk_create_inventory_virtual_circuit_types",
    tag = "inventory",
    request_body = BulkCreateRequest<VirtualCircuitTypePayload>,
    responses(
        (status = 200, description = "Bulk virtual circuit type create result", body = BulkCreateResponse<VirtualCircuitType>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_circuit_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<VirtualCircuitTypePayload>>,
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
        let parsed = match parse_virtual_circuit_type_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_virtual_circuit_type(Uuid::now_v7(), parsed, payload);

        match state.repositories.virtual_circuit_types.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a virtual circuit type.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual-circuit-types/{id}",
    operation_id = "update_inventory_virtual_circuit_type",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual circuit type ID"),
    ),
    request_body = VirtualCircuitTypePayload,
    responses(
        (status = 200, description = "Virtual circuit type updated", body = VirtualCircuitType),
        (status = 404, description = "Virtual circuit type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_circuit_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualCircuitTypePayload>,
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
    let entity_id = match parse_uuid(&id, "virtual_circuit_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_virtual_circuit_type_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .virtual_circuit_types
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Virtual circuit type not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_virtual_circuit_type(entity_id, parsed, payload);
    match state.repositories.virtual_circuit_types.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a virtual circuit type.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual-circuit-types/{id}",
    operation_id = "delete_inventory_virtual_circuit_type",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual circuit type ID"),
    ),
    responses(
        (status = 204, description = "Virtual circuit type deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_circuit_type(
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
    let entity_id = match parse_uuid(&id, "virtual_circuit_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_circuit_types
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Virtual circuit type not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_circuit_types
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
