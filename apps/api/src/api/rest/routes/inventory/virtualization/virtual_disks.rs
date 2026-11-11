//! Virtual disks REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `VirtualDiskRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::{
    BulkVirtualDiskResult, NewVirtualDisk, UpdateVirtualDisk, VIRTUAL_DISK_COLUMNS,
    VirtualDiskFilter, VirtualDiskRecord, VirtualDiskRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for virtual disk listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VirtualDiskListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated virtual machine ids; matches any (`IN`).
    pub virtual_machine_id: Option<String>,
    /// Comma-separated owner ids; matches any (`IN`).
    pub owner_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VirtualDiskCreatePayload {
    pub virtual_machine_id: String,
    pub name: String,
    /// Disk size in megabytes (must be >= 0).
    pub size: i32,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VirtualDiskUpdatePayload {
    pub name: Option<String>,
    pub size: Option<i32>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VirtualDiskBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<VirtualDiskCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VirtualDiskResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub virtual_machine_id: Uuid,
    pub name: String,
    pub size: i32,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkVirtualDiskResponse {
    pub created: Vec<VirtualDiskResponse>,
    pub errors: Vec<String>,
}

super::super::impl_inventory_custom_fields!(VirtualDiskResponse, "virtualization.virtualdisk");

fn map_virtual_disk(record: VirtualDiskRecord) -> VirtualDiskResponse {
    VirtualDiskResponse {
        id: record.id,
        custom_fields: None,
        virtual_machine_id: record.virtual_machine_id,
        name: record.name,
        size: record.size,
        description: record.description,
        owner_id: record.owner_id,
        owner_name: record.owner_name,
    }
}

fn map_bulk_virtual_disk_result(result: BulkVirtualDiskResult) -> BulkVirtualDiskResponse {
    BulkVirtualDiskResponse {
        created: result.created.into_iter().map(map_virtual_disk).collect(),
        errors: result.errors,
    }
}

/// List virtual disks.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-disks",
    tag = "inventory",
    params(VirtualDiskListQuery),
    responses(
        (status = 200, description = "Virtual disks", body = InventoryListEnvelope<VirtualDiskResponse>),
        (status = "4XX", description = "Virtual disk list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual disk list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_disks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualDiskListQuery>,
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
        VIRTUAL_DISK_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let virtual_machine_id =
        match parse_csv_uuids(query.virtual_machine_id.as_deref(), "virtual_machine_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = VirtualDiskFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        virtual_machine_id,
        owner_id,
        sort,
    };

    match state
        .repositories
        .virtual_disks
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let mut items: Vec<VirtualDiskResponse> =
                records.into_iter().map(map_virtual_disk).collect();
            if let Err(error) =
                super::super::mask_inventory_projection_page(&state, &mut items).await
            {
                return handle_app_error(error);
            }
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

/// Get a virtual disk.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-disks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual disk id"),
    ),
    responses(
        (status = 200, description = "Virtual disk", body = VirtualDiskResponse),
        (status = 404, description = "Virtual disk not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual disk get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual disk get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_disk(
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
    match state.repositories.virtual_disks.get_by_id(id).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::OK,
                map_virtual_disk(record),
            )
            .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Virtual disk not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a virtual disk.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-disks",
    tag = "inventory",
    request_body = VirtualDiskCreatePayload,
    responses(
        (status = 201, description = "Virtual disk created", body = VirtualDiskResponse),
        (status = "4XX", description = "Virtual disk create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual disk create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_disk(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualDiskCreatePayload>,
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
    let virtual_machine_id = match parse_uuid(&payload.virtual_machine_id, "virtual_machine_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_disk = NewVirtualDisk {
        id: Uuid::now_v7(),
        virtual_machine_id,
        name,
        size: payload.size,
        description: payload.description,
        owner_id: None,
    };

    match state.repositories.virtual_disks.create(&new_disk).await {
        Ok(record) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::CREATED,
                map_virtual_disk(record),
            )
            .await
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a virtual disk.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual-disks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual disk id"),
    ),
    request_body = VirtualDiskUpdatePayload,
    responses(
        (status = 200, description = "Virtual disk updated", body = VirtualDiskResponse),
        (status = 404, description = "Virtual disk not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual disk update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual disk update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_disk(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualDiskUpdatePayload>,
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

    let changes = UpdateVirtualDisk {
        name: payload.name,
        size: payload.size,
        description: payload.description,
    };

    match state.repositories.virtual_disks.update(id, &changes).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(
                &state,
                StatusCode::OK,
                map_virtual_disk(record),
            )
            .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Virtual disk not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a virtual disk.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual-disks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual disk id"),
    ),
    responses(
        (status = 204, description = "Virtual disk deleted"),
        (status = "4XX", description = "Virtual disk delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual disk delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_disk(
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
    match state.repositories.virtual_disks.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create virtual disks.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-disks/bulk",
    tag = "inventory",
    request_body = VirtualDiskBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk virtual disk create result", body = BulkVirtualDiskResponse),
        (status = "4XX", description = "Bulk virtual disk create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk virtual disk create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_disks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualDiskBulkCreatePayload>,
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
        let virtual_machine_id = match parse_uuid(&p.virtual_machine_id, "virtual_machine_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewVirtualDisk {
            id: Uuid::now_v7(),
            virtual_machine_id,
            name,
            size: p.size,
            description: p.description,
            owner_id: None,
        });
    }

    match state.repositories.virtual_disks.bulk_create(&items).await {
        Ok(result) => {
            let mut body = map_bulk_virtual_disk_result(result);
            match super::super::mask_inventory_projection_page(&state, &mut body.created).await {
                Ok(()) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}
