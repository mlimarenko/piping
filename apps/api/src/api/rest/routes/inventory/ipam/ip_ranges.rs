//! IP ranges REST routes — full CRUD + bulk.
//!
//! Handlers never run SQL directly; all persistence goes through
//! `IpRangeRepository` on `AppState`.

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
    BulkIpRangeResult, IP_RANGE_COLUMNS, IpRangeFilter, IpRangeListItem, IpRangeRecord,
    IpRangeRepository, NewIpRange, UpdateIpRange, normalize_cidr, parse_csv_strings,
    parse_csv_uuids,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_required_string, parse_uuid};

/// Query parameters for IP range listing.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct IpRangeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// CIDR prefix; when set, only ranges fully contained in this prefix are returned.
    pub parent_prefix: Option<String>,
    /// Comma-separated IP range statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated VRF ids; matches any (`IN`).
    pub vrf_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated mark-utilized flags (`true,false`); matches any (`IN`).
    pub mark_utilized: Option<String>,
    /// Comma-separated mark-populated flags (`true,false`); matches any (`IN`).
    pub mark_populated: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpRangeCreatePayload {
    pub start_address: String,
    pub end_address: String,
    pub status: Option<String>,
    pub description: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: Option<bool>,
    pub mark_populated: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpRangeUpdatePayload {
    pub start_address: Option<String>,
    pub end_address: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: Option<bool>,
    pub mark_populated: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IpRangeBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<IpRangeCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IpRangeResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub start_address: String,
    pub end_address: String,
    pub size: i32,
    pub status: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: bool,
    pub mark_populated: bool,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

/// Hydrated IP range list item — extends `IpRangeResponse` with vrf/tenant
/// names for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IpRangeListItemResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub start_address: String,
    pub end_address: String,
    pub size: i32,
    pub status: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub vrf_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: bool,
    pub mark_populated: bool,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkIpRangeResponse {
    pub created: Vec<IpRangeResponse>,
    pub errors: Vec<String>,
}

fn map_ip_range_list_item(item: IpRangeListItem) -> IpRangeListItemResponse {
    let IpRangeListItem {
        record,
        vrf_name,
        tenant_name,
    } = item;
    IpRangeListItemResponse {
        id: record.id,
        external_id: record.external_id,
        start_address: record.start_address,
        end_address: record.end_address,
        size: record.size,
        status: record.status,
        description: record.description,
        comments: record.comments,
        vrf_id: record.vrf_id,
        vrf_name,
        tenant_id: record.tenant_id,
        tenant_name,
        role_id: record.role_id,
        mark_utilized: record.mark_utilized,
        mark_populated: record.mark_populated,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_ip_range(record: IpRangeRecord) -> IpRangeResponse {
    IpRangeResponse {
        id: record.id,
        external_id: record.external_id,
        start_address: record.start_address,
        end_address: record.end_address,
        size: record.size,
        status: record.status,
        description: record.description,
        comments: record.comments,
        vrf_id: record.vrf_id,
        tenant_id: record.tenant_id,
        role_id: record.role_id,
        mark_utilized: record.mark_utilized,
        mark_populated: record.mark_populated,
        owner_id: record.owner_id,
        tags: record.tags,
    }
}

fn map_bulk_ip_range_result(result: BulkIpRangeResult) -> BulkIpRangeResponse {
    BulkIpRangeResponse {
        created: result.created.into_iter().map(map_ip_range).collect(),
        errors: result.errors,
    }
}

/// List IP ranges.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ip_ranges",
    tag = "inventory",
    params(IpRangeListQuery),
    responses(
        (status = 200, description = "IP ranges", body = InventoryListEnvelope<IpRangeListItemResponse>),
        (status = "4XX", description = "IP range list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP range list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_ip_ranges(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<IpRangeListQuery>,
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
    let vrf_id = match parse_csv_uuids(query.vrf_id.as_deref(), "vrf_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parent_prefix = match query
        .parent_prefix
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => match normalize_cidr(value) {
            Ok(normalized) => Some(normalized),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        IP_RANGE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = IpRangeFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        parent_prefix,
        status: parse_csv_strings(query.status.as_deref()),
        vrf_id,
        tenant_id,
        mark_utilized: parse_csv_strings(query.mark_utilized.as_deref()),
        mark_populated: parse_csv_strings(query.mark_populated.as_deref()),
        sort,
    };
    match state
        .repositories
        .ip_ranges
        .list(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<IpRangeListItemResponse> =
                records.into_iter().map(map_ip_range_list_item).collect();
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

/// Get an IP range.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/ip_ranges/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IP range id"),
    ),
    responses(
        (status = 200, description = "IP range", body = IpRangeResponse),
        (status = 404, description = "IP range not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "IP range get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP range get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_ip_range(
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
    match state.repositories.ip_ranges.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_ip_range(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IP range not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an IP range.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ip_ranges",
    tag = "inventory",
    request_body = IpRangeCreatePayload,
    responses(
        (status = 201, description = "IP range created", body = IpRangeResponse),
        (status = "4XX", description = "IP range create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP range create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_ip_range(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpRangeCreatePayload>,
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

    let start_address = match parse_required_string(&payload.start_address, "start_address") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let end_address = match parse_required_string(&payload.end_address, "end_address") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let new_range = NewIpRange {
        id: Uuid::now_v7(),
        start_address,
        end_address,
        status: payload.status,
        description: payload.description,
        vrf_id: payload.vrf_id,
        tenant_id: payload.tenant_id,
        role_id: payload.role_id,
        mark_utilized: payload.mark_utilized,
        mark_populated: payload.mark_populated,
        owner_id: None,
    };

    match state.repositories.ip_ranges.create(&new_range).await {
        Ok(record) => (StatusCode::CREATED, Json(map_ip_range(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an IP range.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/ip_ranges/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IP range id"),
    ),
    request_body = IpRangeUpdatePayload,
    responses(
        (status = 200, description = "IP range updated", body = IpRangeResponse),
        (status = 404, description = "IP range not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "IP range update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP range update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_ip_range(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<IpRangeUpdatePayload>,
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

    let changes = UpdateIpRange {
        start_address: payload.start_address,
        end_address: payload.end_address,
        status: payload.status,
        description: payload.description,
        vrf_id: payload.vrf_id,
        tenant_id: payload.tenant_id,
        role_id: payload.role_id,
        mark_utilized: payload.mark_utilized,
        mark_populated: payload.mark_populated,
    };

    match state.repositories.ip_ranges.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_ip_range(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("IP range not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an IP range.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/ip_ranges/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "IP range id"),
    ),
    responses(
        (status = 204, description = "IP range deleted"),
        (status = "4XX", description = "IP range delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "IP range delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_ip_range(
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
    match state.repositories.ip_ranges.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create IP ranges.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/ip_ranges/bulk",
    tag = "inventory",
    request_body = IpRangeBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk IP range create result", body = BulkIpRangeResponse),
        (status = "4XX", description = "Bulk IP range create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk IP range create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_ip_ranges(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<IpRangeBulkCreatePayload>,
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
        let start_address = match parse_required_string(&p.start_address, "start_address") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let end_address = match parse_required_string(&p.end_address, "end_address") {
            Ok(value) => value,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewIpRange {
            id: Uuid::now_v7(),
            start_address,
            end_address,
            status: p.status,
            description: p.description,
            vrf_id: p.vrf_id,
            tenant_id: p.tenant_id,
            role_id: p.role_id,
            mark_utilized: p.mark_utilized,
            mark_populated: p.mark_populated,
            owner_id: None,
        });
    }

    match state.repositories.ip_ranges.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_ip_range_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
