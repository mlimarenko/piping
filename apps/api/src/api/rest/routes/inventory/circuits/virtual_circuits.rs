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
use crate::contexts::inventory::domain::VirtualCircuit;
use crate::contexts::inventory::infrastructure::persistence::{
    VIRTUAL_CIRCUIT_COLUMNS, VirtualCircuitFilter, VirtualCircuitListItem, VirtualCircuitRepository,
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
pub struct VirtualCircuitListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub status: Option<String>,
    pub provider_network_id: Option<String>,
    pub type_id: Option<String>,
    pub provider_account_id: Option<String>,
    pub tenant_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct VirtualCircuitPayload {
    pub cid: String,
    pub status: String,
    pub provider_network_id: String,
    pub type_id: String,
    pub provider_account_id: Option<String>,
    pub tenant_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Hydrated virtual circuit list item — includes provider-network, type and
/// tenant names for the collection list endpoint.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct VirtualCircuitListItemResponse {
    pub id: Uuid,
    pub cid: String,
    pub status: String,
    pub provider_network_id: Uuid,
    pub provider_network_name: Option<String>,
    pub type_id: Uuid,
    pub type_name: Option<String>,
    pub provider_account_id: Option<Uuid>,
    pub provider_account_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub description: String,
    pub comments: String,
}

fn map_virtual_circuit_list_item(item: VirtualCircuitListItem) -> VirtualCircuitListItemResponse {
    let VirtualCircuitListItem {
        record,
        provider_network_name,
        type_name,
        provider_account_name,
        tenant_name,
    } = item;
    VirtualCircuitListItemResponse {
        id: record.id,
        cid: record.cid,
        status: record.status,
        provider_network_id: record.provider_network_id,
        provider_network_name,
        type_id: record.type_id,
        type_name,
        provider_account_id: record.provider_account_id,
        provider_account_name,
        tenant_id: record.tenant_id,
        tenant_name,
        description: record.description,
        comments: record.comments,
    }
}

struct ParsedVirtualCircuit {
    cid: String,
    status: String,
    provider_network_id: Uuid,
    type_id: Uuid,
    provider_account_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
}

fn parse_virtual_circuit_payload(
    payload: &VirtualCircuitPayload,
) -> Result<ParsedVirtualCircuit, AppError> {
    Ok(ParsedVirtualCircuit {
        cid: parse_required_string(&payload.cid, "cid")?,
        status: parse_required_string(&payload.status, "status")?,
        provider_network_id: parse_uuid(&payload.provider_network_id, "provider_network_id")?,
        type_id: parse_uuid(&payload.type_id, "type_id")?,
        provider_account_id: parse_optional_uuid(
            payload.provider_account_id.clone(),
            "provider_account_id",
        )?,
        tenant_id: parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?,
    })
}

fn build_virtual_circuit(
    id: Uuid,
    parsed: ParsedVirtualCircuit,
    payload: VirtualCircuitPayload,
) -> VirtualCircuit {
    VirtualCircuit {
        id,
        cid: parsed.cid,
        status: parsed.status,
        provider_network_id: parsed.provider_network_id,
        type_id: parsed.type_id,
        provider_account_id: parsed.provider_account_id,
        tenant_id: parsed.tenant_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List virtual circuits.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-circuits",
    operation_id = "list_inventory_virtual_circuits",
    tag = "inventory",
    params(VirtualCircuitListQuery),
    responses(
        (status = 200, description = "Virtual circuits", body = InventoryListEnvelope<VirtualCircuitListItemResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_circuits(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualCircuitListQuery>,
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
    let filter = VirtualCircuitFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        status: parse_csv_strings(query.status.as_deref()),
        provider_network_id: match parse_csv_uuids(
            query.provider_network_id.as_deref(),
            "provider_network_id",
        ) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
        type_id: match parse_csv_uuids(query.type_id.as_deref(), "type_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
        provider_account_id: match parse_csv_uuids(
            query.provider_account_id.as_deref(),
            "provider_account_id",
        ) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
        tenant_id: match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VIRTUAL_CIRCUIT_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .virtual_circuits
        .list_paginated_hydrated(filter, sort, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<VirtualCircuitListItemResponse> = records
                .into_iter()
                .map(map_virtual_circuit_list_item)
                .collect();
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit: resolved_limit,
                    offset: resolved_offset,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a virtual circuit.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-circuits/{id}",
    operation_id = "get_inventory_virtual_circuit",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual circuit ID"),
    ),
    responses(
        (status = 200, description = "Virtual circuit", body = VirtualCircuit),
        (status = 404, description = "Virtual circuit not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_circuit(
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
    let entity_id = match parse_uuid(&id, "virtual_circuit_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_circuits
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Virtual circuit not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a virtual circuit.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-circuits",
    operation_id = "create_inventory_virtual_circuit",
    tag = "inventory",
    request_body = VirtualCircuitPayload,
    responses(
        (status = 201, description = "Virtual circuit created", body = VirtualCircuit),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_circuit(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualCircuitPayload>,
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
    let parsed = match parse_virtual_circuit_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_virtual_circuit(Uuid::now_v7(), parsed, payload);
    match state.repositories.virtual_circuits.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create virtual circuits.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-circuits/bulk",
    operation_id = "bulk_create_inventory_virtual_circuits",
    tag = "inventory",
    request_body = BulkCreateRequest<VirtualCircuitPayload>,
    responses(
        (status = 200, description = "Bulk virtual circuit create result", body = BulkCreateResponse<VirtualCircuit>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_circuits(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<VirtualCircuitPayload>>,
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
        let parsed = match parse_virtual_circuit_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_virtual_circuit(Uuid::now_v7(), parsed, payload);

        match state.repositories.virtual_circuits.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a virtual circuit.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual-circuits/{id}",
    operation_id = "update_inventory_virtual_circuit",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual circuit ID"),
    ),
    request_body = VirtualCircuitPayload,
    responses(
        (status = 200, description = "Virtual circuit updated", body = VirtualCircuit),
        (status = 404, description = "Virtual circuit not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_circuit(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualCircuitPayload>,
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
    let entity_id = match parse_uuid(&id, "virtual_circuit_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_virtual_circuit_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .virtual_circuits
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Virtual circuit not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_virtual_circuit(entity_id, parsed, payload);
    match state.repositories.virtual_circuits.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a virtual circuit.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual-circuits/{id}",
    operation_id = "delete_inventory_virtual_circuit",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual circuit ID"),
    ),
    responses(
        (status = 204, description = "Virtual circuit deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_circuit(
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
    let entity_id = match parse_uuid(&id, "virtual_circuit_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_circuits
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Virtual circuit not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.virtual_circuits.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
