//! Racks REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `RackRepository` on `AppState`.

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
    BulkRackResult, NewRack, RACK_COLUMNS, RackFilter, RackListItem, RackRecord, RackRepository,
    SortSpec, UpdateRack, parse_csv_strings, parse_csv_uuids, parse_id_filter,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

/// Query parameters for rack listing — pagination, optional parent-FK filters,
/// multi-value status, and allowlisted sorting.
///
/// FK and `status` filters accept comma-separated values (`?site_id=a,b`) and
/// combine with AND across fields, `IN` within a field. `sort` is a stable
/// logical column key (`name`, `status`, `site`, `tenant`, `rack_role`);
/// `order` is `asc` (default) or `desc`. An unknown sort key or invalid order is
/// rejected with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct RackListQuery {
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated site ids; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated location ids; matches any (`IN`).
    pub location_id: Option<String>,
    /// Comma-separated rack statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated rack-role ids; matches any (`IN`).
    pub role_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated descending-unit flags (`true,false`); matches any (`IN`).
    pub desc_units: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackCreatePayload {
    pub name: String,
    pub site_id: String,
    pub status: Option<String>,
    pub facility_id: Option<String>,
    pub tenant_id: Option<String>,
    pub rack_role_id: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackUpdatePayload {
    pub name: Option<String>,
    pub site_id: Option<String>,
    pub status: Option<String>,
    pub facility_id: Option<String>,
    pub tenant_id: Option<String>,
    pub rack_role_id: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RackBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<RackCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RackResponse {
    pub id: Uuid,
    pub name: String,
    pub site_id: Uuid,
    pub status: String,
    pub facility_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub rack_role_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: bool,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

/// Hydrated rack list item — extends `RackResponse` with FK names for the
/// collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RackListItemResponse {
    pub id: Uuid,
    pub name: String,
    pub site_id: Uuid,
    pub site_name: Option<String>,
    pub status: String,
    pub facility_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub rack_role_id: Option<Uuid>,
    pub rack_role_name: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: bool,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkRackResponse {
    pub created: Vec<RackResponse>,
    pub errors: Vec<String>,
}

fn map_rack_list_item(item: RackListItem) -> RackListItemResponse {
    let RackListItem {
        record,
        site_name,
        tenant_name,
        rack_role_name,
    } = item;
    RackListItemResponse {
        id: record.id,
        name: record.name,
        site_id: record.site_id,
        site_name,
        status: record.status,
        facility_id: record.facility_id,
        tenant_id: record.tenant_id,
        tenant_name,
        rack_role_id: record.rack_role_id,
        rack_role_name,
        serial: record.serial,
        asset_tag: record.asset_tag,
        type_: record.type_,
        width: record.width,
        u_height: record.u_height,
        desc_units: record.desc_units,
        outer_width: record.outer_width,
        outer_depth: record.outer_depth,
    }
}

fn map_rack(record: RackRecord) -> RackResponse {
    RackResponse {
        id: record.id,
        name: record.name,
        site_id: record.site_id,
        status: record.status,
        facility_id: record.facility_id,
        tenant_id: record.tenant_id,
        rack_role_id: record.rack_role_id,
        serial: record.serial,
        asset_tag: record.asset_tag,
        type_: record.type_,
        width: record.width,
        u_height: record.u_height,
        desc_units: record.desc_units,
        outer_width: record.outer_width,
        outer_depth: record.outer_depth,
    }
}

fn map_bulk_rack_result(result: BulkRackResult) -> BulkRackResponse {
    BulkRackResponse {
        created: result.created.into_iter().map(map_rack).collect(),
        errors: result.errors,
    }
}

/// List racks.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/racks",
    tag = "inventory",
    params(RackListQuery),
    responses(
        (status = 200, description = "Racks", body = InventoryListEnvelope<RackListItemResponse>),
        (status = "4XX", description = "Rack list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_racks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RackListQuery>,
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
    let location_id = match parse_csv_uuids(query.location_id.as_deref(), "location_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_csv_uuids(query.role_id.as_deref(), "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let desc_units = parse_csv_strings(query.desc_units.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        RACK_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let filter = RackFilter {
        ids,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        site_id,
        location_id,
        role_id,
        tenant_id,
        status,
        desc_units,
        sort,
    };

    match state
        .repositories
        .racks
        .list_hydrated(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<RackListItemResponse> =
                records.into_iter().map(map_rack_list_item).collect();
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

/// Get a rack.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/racks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack id"),
    ),
    responses(
        (status = 200, description = "Rack", body = RackResponse),
        (status = 404, description = "Rack not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_rack(
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
    match state.repositories.racks.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rack(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a rack.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/racks",
    tag = "inventory",
    request_body = RackCreatePayload,
    responses(
        (status = 201, description = "Rack created", body = RackResponse),
        (status = "4XX", description = "Rack create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_rack(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackCreatePayload>,
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
    let site_id = match parse_uuid(&payload.site_id, "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let rack_role_id = match parse_optional_uuid(payload.rack_role_id.clone(), "rack_role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_rack = NewRack {
        id: Uuid::now_v7(),
        name,
        site_id,
        status: payload.status,
        facility_id: payload.facility_id,
        tenant_id,
        rack_role_id,
        serial: payload.serial,
        asset_tag: payload.asset_tag,
        type_: payload.type_,
        width: payload.width,
        u_height: payload.u_height,
        desc_units: payload.desc_units,
        outer_width: payload.outer_width,
        outer_depth: payload.outer_depth,
    };

    match state.repositories.racks.create(&new_rack).await {
        Ok(record) => (StatusCode::CREATED, Json(map_rack(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a rack.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/racks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack id"),
    ),
    request_body = RackUpdatePayload,
    responses(
        (status = 200, description = "Rack updated", body = RackResponse),
        (status = 404, description = "Rack not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_rack(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RackUpdatePayload>,
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
    let rack_role_id = match parse_optional_uuid(payload.rack_role_id.clone(), "rack_role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateRack {
        name: payload.name,
        site_id,
        status: payload.status,
        facility_id: payload.facility_id,
        tenant_id,
        rack_role_id,
        serial: payload.serial,
        asset_tag: payload.asset_tag,
        type_: payload.type_,
        width: payload.width,
        u_height: payload.u_height,
        desc_units: payload.desc_units,
        outer_width: payload.outer_width,
        outer_depth: payload.outer_depth,
    };

    match state.repositories.racks.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_rack(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Rack not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a rack.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/racks/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Rack id"),
    ),
    responses(
        (status = 204, description = "Rack deleted"),
        (status = 404, description = "Rack not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Rack delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Rack delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_rack(
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
    match state.repositories.racks.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create racks.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/racks/bulk",
    tag = "inventory",
    request_body = RackBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk rack create result", body = BulkRackResponse),
        (status = "4XX", description = "Bulk rack create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk rack create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_racks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RackBulkCreatePayload>,
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
        let site_id = match parse_uuid(&p.site_id, "site_id") {
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
        let rack_role_id = match parse_optional_uuid(p.rack_role_id.clone(), "rack_role_id") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewRack {
            id: Uuid::now_v7(),
            name,
            site_id,
            status: p.status,
            facility_id: p.facility_id,
            tenant_id,
            rack_role_id,
            serial: p.serial,
            asset_tag: p.asset_tag,
            type_: p.type_,
            width: p.width,
            u_height: p.u_height,
            desc_units: p.desc_units,
            outer_width: p.outer_width,
            outer_depth: p.outer_depth,
        });
    }

    match state.repositories.racks.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_rack_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
