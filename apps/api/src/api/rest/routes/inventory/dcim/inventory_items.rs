//! Inventory items REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `InventoryItemRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ObjectRef;
use crate::contexts::inventory::domain::content_type::INVENTORY_ITEM_COMPONENT_OBJECT_TYPES;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::inventory_item_repository::{
    INVENTORY_ITEM_COLUMNS, InventoryItemFilter,
};
use crate::contexts::inventory::infrastructure::persistence::{
    BulkInventoryItemResult, InventoryItemRecord, InventoryItemRepository, NewInventoryItem,
    UpdateInventoryItem,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ObjectRefInput, ensure_access, parse_optional_uuid,
    parse_required_string, parse_uuid, resolve_object_type,
};

/// Query parameters for inventory item listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct InventoryItemListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub device_id: Option<String>,
    pub parent_id: Option<String>,
    pub role_id: Option<String>,
    pub manufacturer_id: Option<String>,
    pub discovered: Option<String>,
    pub status: Option<String>,
    pub component_type_id: Option<String>,
    pub component_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryItemCreatePayload {
    pub device_id: String,
    pub parent_id: Option<String>,
    pub role_id: Option<String>,
    pub manufacturer_id: Option<String>,
    pub name: String,
    pub label: Option<String>,
    pub part_id: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub discovered: Option<bool>,
    pub description: Option<String>,
    pub status: Option<String>,
    /// Canonical polymorphic component link: `{ object_type: "app_label.model",
    /// object_id }`. `null`/absent leaves it unlinked. Allowed targets:
    /// console/power/front/rear ports and interfaces.
    pub component: Option<ObjectRefInput>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryItemUpdatePayload {
    pub parent_id: Option<String>,
    pub role_id: Option<String>,
    pub manufacturer_id: Option<String>,
    pub name: Option<String>,
    pub label: Option<String>,
    pub part_id: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub discovered: Option<bool>,
    pub description: Option<String>,
    pub status: Option<String>,
    /// When present, re-points the polymorphic component link.
    pub component: Option<ObjectRefInput>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InventoryItemBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<InventoryItemCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryItemResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub device_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub manufacturer_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub part_id: String,
    pub serial: String,
    pub asset_tag: Option<String>,
    pub discovered: bool,
    pub description: String,
    pub status: String,
    /// Materialized polymorphic component link, or `null` when unlinked.
    pub component: Option<ObjectRef>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkInventoryItemResponse {
    pub created: Vec<InventoryItemResponse>,
    pub errors: Vec<String>,
}

super::super::impl_inventory_custom_fields!(InventoryItemResponse, "dcim.inventoryitem");

fn map_inventory_item(
    record: InventoryItemRecord,
    component: Option<ObjectRef>,
) -> InventoryItemResponse {
    InventoryItemResponse {
        id: record.id,
        custom_fields: None,
        device_id: record.device_id,
        parent_id: record.parent_id,
        role_id: record.role_id,
        manufacturer_id: record.manufacturer_id,
        name: record.name,
        label: record.label,
        part_id: record.part_id,
        serial: record.serial,
        asset_tag: record.asset_tag,
        discovered: record.discovered,
        description: record.description,
        status: record.status,
        component,
    }
}

/// Resolve a single record's component pair into a response.
async fn map_one_inventory_item(
    state: &AppState,
    record: InventoryItemRecord,
) -> Result<InventoryItemResponse, AppError> {
    let component = match (record.component_type_id, record.component_id) {
        (Some(type_id), Some(object_id)) => Some(
            state
                .repositories
                .object_ref_resolver
                .resolve_one(type_id, object_id)
                .await?,
        ),
        _ => None,
    };
    let mut response = map_inventory_item(record, component);
    super::super::mask_inventory_projection_page(state, std::slice::from_mut(&mut response))
        .await?;
    Ok(response)
}

/// Batch-resolve a page of records (one resolver round-trip, no N+1).
async fn map_inventory_items(
    state: &AppState,
    records: Vec<InventoryItemRecord>,
) -> Result<Vec<InventoryItemResponse>, AppError> {
    let pairs: Vec<(Uuid, Uuid)> = records
        .iter()
        .filter_map(
            |record| match (record.component_type_id, record.component_id) {
                (Some(type_id), Some(object_id)) => Some((type_id, object_id)),
                _ => None,
            },
        )
        .collect();
    let refs = state
        .repositories
        .object_ref_resolver
        .resolve(&pairs)
        .await?;
    let mut responses: Vec<_> = records
        .into_iter()
        .map(|record| {
            let component = match (record.component_type_id, record.component_id) {
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
            map_inventory_item(record, component)
        })
        .collect();
    super::super::mask_inventory_projection_page(state, &mut responses).await?;
    Ok(responses)
}

async fn map_bulk_inventory_item_result(
    state: &AppState,
    result: BulkInventoryItemResult,
) -> Result<BulkInventoryItemResponse, AppError> {
    Ok(BulkInventoryItemResponse {
        created: map_inventory_items(state, result.created).await?,
        errors: result.errors,
    })
}

/// Resolve an optional polymorphic component input into the `(component_type_id,
/// component_id)` pair stored on the record.
async fn resolve_component(
    state: &AppState,
    component: Option<&ObjectRefInput>,
) -> Result<(Option<Uuid>, Option<Uuid>), AppError> {
    match component {
        Some(input) => {
            let type_id = resolve_object_type(
                &state.db_pool,
                &input.object_type,
                INVENTORY_ITEM_COMPONENT_OBJECT_TYPES,
            )
            .await?;
            Ok((Some(type_id), Some(input.object_id)))
        }
        None => Ok((None, None)),
    }
}

/// List inventory items.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/inventory-items",
    tag = "inventory",
    params(InventoryItemListQuery),
    responses(
        (status = 200, description = "Inventory items", body = InventoryListEnvelope<InventoryItemResponse>),
        (status = "4XX", description = "Inventory item list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_inventory_items(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<InventoryItemListQuery>,
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

    let device_id = match parse_csv_uuids(query.device_id.as_deref(), "device_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_csv_uuids(query.parent_id.as_deref(), "parent_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_csv_uuids(query.role_id.as_deref(), "role_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let manufacturer_id = match parse_csv_uuids(query.manufacturer_id.as_deref(), "manufacturer_id")
    {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let component_type_id =
        match parse_csv_uuids(query.component_type_id.as_deref(), "component_type_id") {
            Ok(v) => v,
            Err(error) => return handle_app_error(error),
        };
    let component_id = match parse_csv_uuids(query.component_id.as_deref(), "component_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        INVENTORY_ITEM_COLUMNS,
    ) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let filter = InventoryItemFilter {
        device_id,
        parent_id,
        role_id,
        manufacturer_id,
        discovered: parse_csv_strings(query.discovered.as_deref()),
        status: parse_csv_strings(query.status.as_deref()),
        component_type_id,
        component_id,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        sort,
    };

    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .inventory_items
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => match map_inventory_items(&state, records).await {
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
        },
        Err(error) => handle_app_error(error),
    }
}

/// Get an inventory item.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/inventory-items/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory item id"),
    ),
    responses(
        (status = 200, description = "Inventory item", body = InventoryItemResponse),
        (status = 404, description = "Inventory item not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Inventory item get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_inventory_item(
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.inventory_items.get_by_id(id).await {
        Ok(Some(record)) => match map_one_inventory_item(&state, record).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Inventory item not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an inventory item.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/inventory-items",
    tag = "inventory",
    request_body = InventoryItemCreatePayload,
    responses(
        (status = 201, description = "Inventory item created", body = InventoryItemResponse),
        (status = "4XX", description = "Inventory item create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_inventory_item(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InventoryItemCreatePayload>,
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

    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let device_id = match parse_uuid(&payload.device_id, "device_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_id = match parse_optional_uuid(payload.parent_id, "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_optional_uuid(payload.role_id, "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let manufacturer_id = match parse_optional_uuid(payload.manufacturer_id, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (component_type_id, component_id) =
        match resolve_component(&state, payload.component.as_ref()).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let new_item = NewInventoryItem {
        id: Uuid::now_v7(),
        device_id,
        parent_id,
        role_id,
        manufacturer_id,
        name,
        label: payload.label,
        part_id: payload.part_id,
        serial: payload.serial,
        asset_tag: payload.asset_tag,
        discovered: payload.discovered,
        description: payload.description,
        status: payload.status,
        component_type_id,
        component_id,
    };

    match state.repositories.inventory_items.create(&new_item).await {
        Ok(record) => match map_one_inventory_item(&state, record).await {
            Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

/// Update an inventory item.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/inventory-items/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory item id"),
    ),
    request_body = InventoryItemUpdatePayload,
    responses(
        (status = 200, description = "Inventory item updated", body = InventoryItemResponse),
        (status = 404, description = "Inventory item not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Inventory item update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_inventory_item(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<InventoryItemUpdatePayload>,
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let parent_id = match parse_optional_uuid(payload.parent_id, "parent_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_optional_uuid(payload.role_id, "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let manufacturer_id = match parse_optional_uuid(payload.manufacturer_id, "manufacturer_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (component_type_id, component_id) =
        match resolve_component(&state, payload.component.as_ref()).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let changes = UpdateInventoryItem {
        parent_id,
        role_id,
        manufacturer_id,
        name: payload.name,
        label: payload.label,
        part_id: payload.part_id,
        serial: payload.serial,
        asset_tag: payload.asset_tag,
        discovered: payload.discovered,
        description: payload.description,
        status: payload.status,
        component_type_id,
        component_id,
    };

    match state
        .repositories
        .inventory_items
        .update(id, &changes)
        .await
    {
        Ok(Some(record)) => match map_one_inventory_item(&state, record).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Inventory item not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an inventory item.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/inventory-items/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory item id"),
    ),
    responses(
        (status = 204, description = "Inventory item deleted"),
        (status = "4XX", description = "Inventory item delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_inventory_item(
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
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.inventory_items.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create inventory items.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/inventory-items/bulk",
    tag = "inventory",
    request_body = InventoryItemBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk inventory item create result", body = BulkInventoryItemResponse),
        (status = "4XX", description = "Bulk inventory item create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk inventory item create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_inventory_items(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InventoryItemBulkCreatePayload>,
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

    let mut items = Vec::new();
    for (index, p) in payload.items.into_iter().enumerate() {
        let name = match parse_required_string(&p.name, "name") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let device_id = match parse_uuid(&p.device_id, "device_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let parent_id = match parse_optional_uuid(p.parent_id, "parent_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let role_id = match parse_optional_uuid(p.role_id, "role_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let manufacturer_id = match parse_optional_uuid(p.manufacturer_id, "manufacturer_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let (component_type_id, component_id) =
            match resolve_component(&state, p.component.as_ref()).await {
                Ok(value) => value,
                Err(error) => {
                    return handle_app_error(AppError::Validation(format!(
                        "item {index}: {error:?}"
                    )));
                }
            };
        items.push(NewInventoryItem {
            id: Uuid::now_v7(),
            device_id,
            parent_id,
            role_id,
            manufacturer_id,
            name,
            label: p.label,
            part_id: p.part_id,
            serial: p.serial,
            asset_tag: p.asset_tag,
            discovered: p.discovered,
            description: p.description,
            status: p.status,
            component_type_id,
            component_id,
        });
    }

    match state.repositories.inventory_items.bulk_create(&items).await {
        Ok(result) => match map_bulk_inventory_item_result(&state, result).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}
