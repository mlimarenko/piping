//! Inventory item roles REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `InventoryItemRoleRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::infrastructure::persistence::repositories::derive_slug;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkInventoryItemRoleResult, INVENTORY_ITEM_ROLE_COLUMNS, InventoryItemRoleRecord,
    InventoryItemRoleRepository, NewInventoryItemRole, UpdateInventoryItemRole,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for inventory item role listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct InventoryItemRoleListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub owner_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InventoryItemRoleCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InventoryItemRoleUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InventoryItemRoleBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<InventoryItemRoleCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryItemRoleResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkInventoryItemRoleResponse {
    pub created: Vec<InventoryItemRoleResponse>,
    pub errors: Vec<String>,
}

fn map_inventory_item_role(record: InventoryItemRoleRecord) -> InventoryItemRoleResponse {
    InventoryItemRoleResponse {
        id: record.id,
        name: record.name,
        slug: record.slug,
        color: record.color,
        description: record.description,
        owner_id: record.owner_id,
        owner_name: record.owner_name,
        custom_fields: record.custom_fields,
    }
}

fn map_bulk_inventory_item_role_result(
    result: BulkInventoryItemRoleResult,
) -> BulkInventoryItemRoleResponse {
    BulkInventoryItemRoleResponse {
        created: result
            .created
            .into_iter()
            .map(map_inventory_item_role)
            .collect(),
        errors: result.errors,
    }
}

fn resolve_slug(slug: Option<String>, name: &str) -> String {
    slug.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| derive_slug(name))
}

/// List inventory item roles.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/inventory-item-roles",
    tag = "inventory",
    params(InventoryItemRoleListQuery),
    responses(
        (status = 200, description = "Inventory item roles", body = InventoryListEnvelope<InventoryItemRoleResponse>),
        (status = "4XX", description = "Inventory item role list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item role list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_inventory_item_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<InventoryItemRoleListQuery>,
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
        INVENTORY_ITEM_ROLE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .inventory_item_roles
        .list(
            query.limit,
            query.offset,
            &owner_id,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<InventoryItemRoleResponse> =
                records.into_iter().map(map_inventory_item_role).collect();
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

/// Get an inventory item role.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/inventory-item-roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory item role id"),
    ),
    responses(
        (status = 200, description = "Inventory item role", body = InventoryItemRoleResponse),
        (status = 404, description = "Inventory item role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Inventory item role get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item role get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_inventory_item_role(
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
    match state.repositories.inventory_item_roles.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_inventory_item_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Inventory item role not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create an inventory item role.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/inventory-item-roles",
    tag = "inventory",
    request_body = InventoryItemRoleCreatePayload,
    responses(
        (status = 201, description = "Inventory item role created", body = InventoryItemRoleResponse),
        (status = "4XX", description = "Inventory item role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_inventory_item_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InventoryItemRoleCreatePayload>,
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
    let slug = resolve_slug(payload.slug, &name);

    let new_role = NewInventoryItemRole {
        id: Uuid::now_v7(),
        name,
        slug,
        color: payload.color,
        description: payload.description,
        owner_id: None,
        custom_fields: payload.custom_fields,
    };

    match state
        .repositories
        .inventory_item_roles
        .create(&new_role)
        .await
    {
        Ok(record) => (StatusCode::CREATED, Json(map_inventory_item_role(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an inventory item role.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/inventory-item-roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory item role id"),
    ),
    request_body = InventoryItemRoleUpdatePayload,
    responses(
        (status = 200, description = "Inventory item role updated", body = InventoryItemRoleResponse),
        (status = 404, description = "Inventory item role not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Inventory item role update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item role update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_inventory_item_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<InventoryItemRoleUpdatePayload>,
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

    let changes = UpdateInventoryItemRole {
        name: payload.name,
        slug: payload.slug,
        color: payload.color,
        description: payload.description,
        custom_fields: payload.custom_fields,
    };

    match state
        .repositories
        .inventory_item_roles
        .update(id, &changes)
        .await
    {
        Ok(Some(record)) => (StatusCode::OK, Json(map_inventory_item_role(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Inventory item role not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an inventory item role.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/inventory-item-roles/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory item role id"),
    ),
    responses(
        (status = 204, description = "Inventory item role deleted"),
        (status = "4XX", description = "Inventory item role delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Inventory item role delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_inventory_item_role(
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
    match state.repositories.inventory_item_roles.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create inventory item roles.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/inventory-item-roles/bulk",
    tag = "inventory",
    request_body = InventoryItemRoleBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk inventory item role create result", body = BulkInventoryItemRoleResponse),
        (status = "4XX", description = "Bulk inventory item role create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk inventory item role create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_inventory_item_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InventoryItemRoleBulkCreatePayload>,
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
        let slug = resolve_slug(p.slug, &name);
        items.push(NewInventoryItemRole {
            id: Uuid::now_v7(),
            name,
            slug,
            color: p.color,
            description: p.description,
            owner_id: None,
            custom_fields: p.custom_fields,
        });
    }

    match state
        .repositories
        .inventory_item_roles
        .bulk_create(&items)
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(map_bulk_inventory_item_role_result(result)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}
