use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::content_type::MAC_ASSIGNED_OBJECT_TYPES;
use crate::contexts::inventory::domain::{MacAddress, ObjectRef};
use crate::contexts::inventory::infrastructure::persistence::{
    MAC_ADDRESS_COLUMNS, MacAddressFilter, MacAddressRepository, SqlxObjectRefResolver,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, ObjectRefInput,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items, resolve_object_type,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct MacAddressListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated assigned object content-type ids; matches any (`IN`).
    pub assigned_object_type_id: Option<String>,
    /// Comma-separated assigned object ids; matches any (`IN`).
    pub assigned_object_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MacAddressPayload {
    pub mac_address: String,
    /// Canonical polymorphic assignment: `{ object_type: "app_label.model",
    /// object_id }`. `null`/absent clears the assignment. Allowed targets:
    /// `dcim.interface`, `virtualization.vminterface`.
    pub assigned_object: Option<ObjectRefInput>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Response projection for a MAC address.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MacAddressResponse {
    pub id: Uuid,
    pub mac_address: String,
    /// Materialized polymorphic assignment, or `null` when unassigned.
    pub assigned_object: Option<ObjectRef>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}

/// Resolve the assignment pair on a `MacAddress` and project it to a response.
async fn map_one_mac_address(
    resolver: &SqlxObjectRefResolver,
    item: MacAddress,
) -> Result<MacAddressResponse, AppError> {
    let assigned_object = match (item.assigned_object_type_id, item.assigned_object_id) {
        (Some(type_id), Some(object_id)) => Some(resolver.resolve_one(type_id, object_id).await?),
        _ => None,
    };
    Ok(MacAddressResponse {
        id: item.id,
        mac_address: item.mac_address,
        assigned_object,
        description: item.description,
        comments: item.comments,
        custom_fields: item.custom_fields,
    })
}

/// Batch-resolve a page of MAC addresses (one resolver round-trip, no N+1).
async fn map_mac_addresses(
    resolver: &SqlxObjectRefResolver,
    items: Vec<MacAddress>,
) -> Result<Vec<MacAddressResponse>, AppError> {
    let pairs: Vec<(Uuid, Uuid)> = items
        .iter()
        .filter_map(
            |item| match (item.assigned_object_type_id, item.assigned_object_id) {
                (Some(type_id), Some(object_id)) => Some((type_id, object_id)),
                _ => None,
            },
        )
        .collect();
    let refs = resolver.resolve(&pairs).await?;
    Ok(items
        .into_iter()
        .map(|item| {
            let assigned_object = match (item.assigned_object_type_id, item.assigned_object_id) {
                (Some(type_id), Some(object_id)) => Some(
                    refs.get(&(type_id, object_id))
                        .cloned()
                        .unwrap_or(ObjectRef {
                            object_type: String::new(),
                            object_id,
                            label: None,
                        }),
                ),
                _ => None,
            };
            MacAddressResponse {
                id: item.id,
                mac_address: item.mac_address,
                assigned_object,
                description: item.description,
                comments: item.comments,
                custom_fields: item.custom_fields,
            }
        })
        .collect())
}

struct ParsedMacAddress {
    mac_address: String,
    assigned_object_type_id: Option<Uuid>,
    assigned_object_id: Option<Uuid>,
}

/// Validate + resolve the MAC payload, including its optional polymorphic
/// assignment (token → seeded content-type id, gated by the allowed subset).
async fn parse_mac_address_payload(
    state: &AppState,
    payload: &MacAddressPayload,
) -> Result<ParsedMacAddress, AppError> {
    let (assigned_object_type_id, assigned_object_id) = match &payload.assigned_object {
        Some(input) => {
            let type_id = resolve_object_type(
                &state.db_pool,
                &input.object_type,
                MAC_ASSIGNED_OBJECT_TYPES,
            )
            .await?;
            (Some(type_id), Some(input.object_id))
        }
        None => (None, None),
    };
    Ok(ParsedMacAddress {
        mac_address: parse_required_string(&payload.mac_address, "mac_address")?,
        assigned_object_type_id,
        assigned_object_id,
    })
}

fn build_mac_address(id: Uuid, parsed: ParsedMacAddress, payload: MacAddressPayload) -> MacAddress {
    MacAddress {
        id,
        mac_address: parsed.mac_address,
        assigned_object_type_id: parsed.assigned_object_type_id,
        assigned_object_id: parsed.assigned_object_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List MAC addresses.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/mac-addresses",
    operation_id = "list_inventory_mac_addresses",
    tag = "inventory",
    params(MacAddressListQuery),
    responses(
        (status = 200, description = "MAC addresses", body = InventoryListEnvelope<MacAddressResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_mac_addresses(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<MacAddressListQuery>,
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
        MAC_ADDRESS_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let assigned_object_type_id = match parse_csv_uuids(
        query.assigned_object_type_id.as_deref(),
        "assigned_object_type_id",
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let assigned_object_id =
        match parse_csv_uuids(query.assigned_object_id.as_deref(), "assigned_object_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let filter = MacAddressFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        assigned_object_type_id,
        assigned_object_id,
        sort,
    };
    match state
        .repositories
        .mac_addresses
        .list_paginated(query.limit, query.offset, filter)
        .await
    {
        Ok((items, total)) => {
            match map_mac_addresses(&state.repositories.object_ref_resolver, items).await {
                Ok(items) => (
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
        Err(error) => handle_app_error(error),
    }
}

/// Get a MAC address.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/mac-addresses/{id}",
    operation_id = "get_inventory_mac_address",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "MAC address ID"),
    ),
    responses(
        (status = 200, description = "MAC address", body = MacAddressResponse),
        (status = 404, description = "MAC address not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_mac_address(
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
    let entity_id = match parse_uuid(&id, "mac_address_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.mac_addresses.get_by_id(entity_id).await {
        Ok(Some(item)) => {
            match map_one_mac_address(&state.repositories.object_ref_resolver, item).await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Ok(None) => handle_app_error(AppError::NotFound("MAC address not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a MAC address.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/mac-addresses",
    operation_id = "create_inventory_mac_address",
    tag = "inventory",
    request_body = MacAddressPayload,
    responses(
        (status = 201, description = "MAC address created", body = MacAddressResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_mac_address(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MacAddressPayload>,
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
    let parsed = match parse_mac_address_payload(&state, &payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_mac_address(Uuid::now_v7(), parsed, payload);
    match state.repositories.mac_addresses.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            match map_one_mac_address(&state.repositories.object_ref_resolver, item).await {
                Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create MAC addresses.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/mac-addresses/bulk",
    operation_id = "bulk_create_inventory_mac_addresses",
    tag = "inventory",
    request_body = BulkCreateRequest<MacAddressPayload>,
    responses(
        (status = 200, description = "Bulk MAC address create result", body = BulkCreateResponse<MacAddressResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_mac_addresses(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<MacAddressPayload>>,
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
        let parsed = match parse_mac_address_payload(&state, &payload).await {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_mac_address(Uuid::now_v7(), parsed, payload);

        match state.repositories.mac_addresses.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    let created = match map_mac_addresses(&state.repositories.object_ref_resolver, created).await {
        Ok(created) => created,
        Err(error) => return handle_app_error(error),
    };

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a MAC address.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/mac-addresses/{id}",
    operation_id = "update_inventory_mac_address",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "MAC address ID"),
    ),
    request_body = MacAddressPayload,
    responses(
        (status = 200, description = "MAC address updated", body = MacAddressResponse),
        (status = 404, description = "MAC address not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_mac_address(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<MacAddressPayload>,
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
    let entity_id = match parse_uuid(&id, "mac_address_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_mac_address_payload(&state, &payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state.repositories.mac_addresses.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("MAC address not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_mac_address(entity_id, parsed, payload);
    match state.repositories.mac_addresses.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            match map_one_mac_address(&state.repositories.object_ref_resolver, item).await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a MAC address.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/mac-addresses/{id}",
    operation_id = "delete_inventory_mac_address",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "MAC address ID"),
    ),
    responses(
        (status = 204, description = "MAC address deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_mac_address(
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
    let entity_id = match parse_uuid(&id, "mac_address_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.mac_addresses.get_by_id(entity_id).await {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("MAC address not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.mac_addresses.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
