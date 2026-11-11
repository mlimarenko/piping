//! VLANs REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `VlanRepository` on `AppState`.

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
    BulkCreateResult, NewVlan, SortSpec, UpdateVlan, VLAN_COLUMNS, VlanFilter, VlanListItem,
    VlanRecord, VlanRepository, parse_csv_strings, parse_csv_uuids, parse_id_filter,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for VLAN listing — pagination, optional parent-FK filters,
/// optional `vid` exact match, multi-value status, and allowlisted sorting.
///
/// FK and `status` filters accept comma-separated values (`?site_id=a,b`) and
/// combine with AND across fields, `IN` within a field. `sort` is a stable
/// logical column key (`vid`, `name`, `status`, `site`, `tenant`); `order` is
/// `asc` (default) or `desc`. An unknown sort key or invalid order is rejected
/// with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VlanListQuery {
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated site ids; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated VLAN-group ids; matches any (`IN`).
    pub group_id: Option<String>,
    /// Optional VLAN id (`vid`) exact match.
    pub vid: Option<i32>,
    /// Comma-separated VLAN statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VlanCreatePayload {
    pub vid: i32,
    pub name: String,
    pub status: Option<String>,
    pub site_id: Option<String>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<String>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VlanUpdatePayload {
    pub vid: Option<i32>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub site_id: Option<String>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<String>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VlanBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<VlanCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VlanResponse {
    pub id: Uuid,
    pub vid: i32,
    pub name: String,
    pub status: String,
    pub site_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Hydrated VLAN list item — extends `VlanResponse` with site/tenant names for
/// the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VlanListItemResponse {
    pub id: Uuid,
    pub vid: i32,
    pub name: String,
    pub status: String,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub group_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkVlanResponse {
    pub created: Vec<VlanResponse>,
    pub errors: Vec<String>,
}

fn map_vlan_list_item(item: VlanListItem) -> VlanListItemResponse {
    let VlanListItem {
        record,
        site_name,
        tenant_name,
    } = item;
    VlanListItemResponse {
        id: record.id,
        vid: record.vid,
        name: record.name,
        status: record.status,
        site_id: record.site_id,
        site_name,
        group_id: record.group_id,
        tenant_id: record.tenant_id,
        tenant_name,
        role_id: record.role_id,
        description: record.description,
    }
}

fn map_vlan(record: VlanRecord) -> VlanResponse {
    VlanResponse {
        id: record.id,
        vid: record.vid,
        name: record.name,
        status: record.status,
        site_id: record.site_id,
        group_id: record.group_id,
        tenant_id: record.tenant_id,
        role_id: record.role_id,
        description: record.description,
    }
}

fn map_bulk_vlan_result(result: BulkCreateResult<VlanRecord>) -> BulkVlanResponse {
    BulkVlanResponse {
        created: result.created.into_iter().map(map_vlan).collect(),
        errors: result.errors,
    }
}

/// List VLANs.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vlans",
    tag = "inventory",
    params(VlanListQuery),
    responses(
        (status = 200, description = "VLANs", body = InventoryListEnvelope<VlanListItemResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_vlans(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VlanListQuery>,
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

    let ids = match parse_id_filter(query.ids.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = match parse_csv_uuids(query.site_id.as_deref(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_csv_uuids(query.group_id.as_deref(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VLAN_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let filter = VlanFilter {
        ids,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        site_id,
        group_id,
        tenant_id,
        vid: query.vid,
        status,
        sort,
    };

    match state
        .repositories
        .vlans
        .list_hydrated(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<VlanListItemResponse> =
                records.into_iter().map(map_vlan_list_item).collect();
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

/// Get a VLAN.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/vlans/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VLAN ID"),
    ),
    responses(
        (status = 200, description = "VLAN", body = VlanResponse),
        (status = 404, description = "VLAN not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_vlan(
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
    match state.repositories.vlans.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_vlan(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("VLAN not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a VLAN.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vlans",
    tag = "inventory",
    request_body = VlanCreatePayload,
    responses(
        (status = 201, description = "VLAN created", body = VlanResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_vlan(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VlanCreatePayload>,
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
    let site_id = match parse_optional_uuid(payload.site_id.clone(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_vlan = NewVlan {
        id: Uuid::now_v7(),
        vid: payload.vid,
        name,
        status: payload.status,
        site_id,
        group_id: payload.group_id,
        tenant_id,
        role_id: payload.role_id,
        description: payload.description,
    };

    match state.repositories.vlans.create(&new_vlan).await {
        Ok(record) => (StatusCode::CREATED, Json(map_vlan(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a VLAN.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/vlans/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VLAN ID"),
    ),
    request_body = VlanUpdatePayload,
    responses(
        (status = 200, description = "VLAN updated", body = VlanResponse),
        (status = 404, description = "VLAN not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_vlan(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VlanUpdatePayload>,
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
    let site_id = match parse_optional_uuid(payload.site_id.clone(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateVlan {
        vid: payload.vid,
        name: payload.name,
        status: payload.status,
        site_id,
        group_id: payload.group_id,
        tenant_id,
        role_id: payload.role_id,
        description: payload.description,
    };

    match state.repositories.vlans.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_vlan(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("VLAN not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a VLAN.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/vlans/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "VLAN ID"),
    ),
    responses(
        (status = 204, description = "VLAN deleted"),
    ),
)]
pub async fn delete_vlan(
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
    match state.repositories.vlans.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create VLANs.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/vlans/bulk",
    tag = "inventory",
    request_body = VlanBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk VLAN create result", body = BulkVlanResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_vlans(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VlanBulkCreatePayload>,
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
        let site_id = match parse_optional_uuid(p.site_id.clone(), "site_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let tenant_id = match parse_optional_uuid(p.tenant_id.clone(), "tenant_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewVlan {
            id: Uuid::now_v7(),
            vid: p.vid,
            name,
            status: p.status,
            site_id,
            group_id: p.group_id,
            tenant_id,
            role_id: p.role_id,
            description: p.description,
        });
    }

    match state.repositories.vlans.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_vlan_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
