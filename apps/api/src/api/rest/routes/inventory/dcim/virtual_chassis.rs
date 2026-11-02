//! Virtual chassis REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `VirtualChassisRepository` on `AppState`.
//!
//! Route base: `/api/v1/inventory/virtual-chassis`
//! (singular == plural for this resource; no separate plural form needed)

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
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::virtual_chassis_repository::VirtualChassisListParams;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkVirtualChassisResult, NewVirtualChassis, UpdateVirtualChassis, VIRTUAL_CHASSIS_COLUMNS,
    VirtualChassisRecord, VirtualChassisRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for virtual chassis listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VirtualChassisListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub master_id: Option<String>,
    pub owner_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VirtualChassisCreatePayload {
    pub name: String,
    /// DNS domain name for the chassis management plane (empty string if unset).
    pub domain: Option<String>,
    pub master_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VirtualChassisUpdatePayload {
    pub name: Option<String>,
    pub domain: Option<String>,
    /// Pass `null` explicitly to clear the master device.
    pub master_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VirtualChassisBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<VirtualChassisCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VirtualChassisResponse {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub master_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub member_count: i64,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkVirtualChassisResponse {
    pub created: Vec<VirtualChassisResponse>,
    pub errors: Vec<String>,
}

fn map_virtual_chassis(record: VirtualChassisRecord) -> VirtualChassisResponse {
    VirtualChassisResponse {
        id: record.id,
        name: record.name,
        domain: record.domain,
        master_id: record.master_id,
        description: record.description,
        comments: record.comments,
        member_count: record.member_count,
        owner_id: record.owner_id,
        owner_name: record.owner_name,
        custom_fields: record.custom_fields,
    }
}

fn map_bulk_virtual_chassis_result(result: BulkVirtualChassisResult) -> BulkVirtualChassisResponse {
    BulkVirtualChassisResponse {
        created: result
            .created
            .into_iter()
            .map(map_virtual_chassis)
            .collect(),
        errors: result.errors,
    }
}

/// List virtual chassis.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-chassis",
    tag = "inventory",
    params(VirtualChassisListQuery),
    responses(
        (status = 200, description = "Virtual chassis list", body = InventoryListEnvelope<VirtualChassisResponse>),
        (status = "4XX", description = "Virtual chassis list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual chassis list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_chassis(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualChassisListQuery>,
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
    let master_ids = match parse_csv_uuids(query.master_id.as_deref(), "master_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let owner_ids = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VIRTUAL_CHASSIS_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_chassis
        .list(VirtualChassisListParams {
            master_ids: &master_ids,
            owner_ids: &owner_ids,
            search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            limit: query.limit,
            offset: query.offset,
            sort,
        })
        .await
    {
        Ok((records, total)) => {
            let items: Vec<VirtualChassisResponse> =
                records.into_iter().map(map_virtual_chassis).collect();
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

/// Get a virtual chassis.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-chassis/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual chassis id"),
    ),
    responses(
        (status = 200, description = "Virtual chassis", body = VirtualChassisResponse),
        (status = 404, description = "Virtual chassis not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual chassis get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual chassis get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_chassis(
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
    match state.repositories.virtual_chassis.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_virtual_chassis(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Virtual chassis not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a virtual chassis.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-chassis",
    tag = "inventory",
    request_body = VirtualChassisCreatePayload,
    responses(
        (status = 201, description = "Virtual chassis created", body = VirtualChassisResponse),
        (status = "4XX", description = "Virtual chassis create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual chassis create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_chassis(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualChassisCreatePayload>,
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

    let new_vc = NewVirtualChassis {
        id: Uuid::now_v7(),
        name,
        domain: payload.domain.unwrap_or_default(),
        master_id: payload.master_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        owner_id: None,
    };

    match state.repositories.virtual_chassis.create(&new_vc).await {
        Ok(record) => (StatusCode::CREATED, Json(map_virtual_chassis(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a virtual chassis.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual-chassis/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual chassis id"),
    ),
    request_body = VirtualChassisUpdatePayload,
    responses(
        (status = 200, description = "Virtual chassis updated", body = VirtualChassisResponse),
        (status = 404, description = "Virtual chassis not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Virtual chassis update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual chassis update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_chassis(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualChassisUpdatePayload>,
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

    let changes = UpdateVirtualChassis {
        name: payload.name,
        domain: payload.domain,
        master_id: payload.master_id,
        description: payload.description,
        comments: payload.comments,
    };

    match state
        .repositories
        .virtual_chassis
        .update(id, &changes)
        .await
    {
        Ok(Some(record)) => (StatusCode::OK, Json(map_virtual_chassis(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Virtual chassis not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a virtual chassis.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual-chassis/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual chassis id"),
    ),
    responses(
        (status = 204, description = "Virtual chassis deleted"),
        (status = "4XX", description = "Virtual chassis delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Virtual chassis delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_chassis(
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
    match state.repositories.virtual_chassis.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create virtual chassis.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-chassis/bulk",
    tag = "inventory",
    request_body = VirtualChassisBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk virtual chassis create result", body = BulkVirtualChassisResponse),
        (status = "4XX", description = "Bulk virtual chassis create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk virtual chassis create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_chassis(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualChassisBulkCreatePayload>,
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
        items.push(NewVirtualChassis {
            id: Uuid::now_v7(),
            name,
            domain: p.domain.unwrap_or_default(),
            master_id: p.master_id,
            description: p.description.unwrap_or_default(),
            comments: p.comments.unwrap_or_default(),
            owner_id: None,
        });
    }

    match state.repositories.virtual_chassis.bulk_create(&items).await {
        Ok(result) => (
            StatusCode::OK,
            Json(map_bulk_virtual_chassis_result(result)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}
