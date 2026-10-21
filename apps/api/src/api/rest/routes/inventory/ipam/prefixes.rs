//! Prefixes REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `PrefixRepository` on `AppState`.

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
    BulkPrefixResult, NewPrefix, PREFIX_COLUMNS, PrefixFilter, PrefixListItem,
    PrefixPlanningPreview, PrefixRecord, PrefixRepository, PrefixTreeFilter, PrefixTreeNode,
    PrefixTreeReadModel, SortSpec, UpdatePrefix, parse_csv_strings, parse_csv_uuids,
    parse_id_filter,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string,
};

/// Query parameters for prefix listing — pagination, optional parent-FK filters,
/// multi-value status, and allowlisted sorting.
///
/// FK and `status` filters accept comma-separated values (`?site_id=a,b`) and
/// combine with AND across fields, `IN` within a field. `sort` is a stable
/// logical column key (`prefix`, `status`, `site`, `vrf`, `tenant`); `order` is
/// `asc` (default) or `desc`. An unknown sort key or invalid order is rejected
/// with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PrefixListQuery {
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    /// Comma-separated site ids; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated VRF ids; matches any (`IN`).
    pub vrf_id: Option<String>,
    /// Comma-separated prefix statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Comma-separated IPAM-role ids; matches any (`IN`).
    pub role_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated pool flags (`true,false`); matches any (`IN`).
    pub is_pool: Option<String>,
    /// Comma-separated mark-utilized flags (`true,false`); matches any (`IN`).
    pub mark_utilized: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

/// Query parameters for the prefix tree.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PrefixTreeQuery {
    /// Case-insensitive free-text search over prefix, status, description, and labels.
    pub search: Option<String>,
    /// Root prefix id. When present, only this prefix and descendants are returned.
    pub root_id: Option<String>,
    /// Root CIDR. Used when a caller has a CIDR but not a prefix id.
    pub root_prefix: Option<String>,
    /// IP family: 4 or 6.
    pub family: Option<u8>,
    /// Comma-separated site ids.
    pub site_id: Option<String>,
    /// Comma-separated VRF ids.
    pub vrf_id: Option<String>,
    /// Comma-separated tenant ids.
    pub tenant_id: Option<String>,
    /// Comma-separated prefix statuses.
    pub status: Option<String>,
    /// Maximum nodes to return; capped at 500.
    pub limit: Option<u32>,
}

/// Query parameters for next-available child-prefix preview.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PrefixPlanningPreviewQuery {
    /// Desired child prefix length, for example 24 under a /16.
    pub prefix_length: i32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PrefixCreatePayload {
    pub prefix: String,
    pub status: Option<String>,
    pub site_id: Option<String>,
    pub vrf_id: Option<String>,
    pub tenant_id: Option<String>,
    pub vlan_id: Option<String>,
    pub role_id: Option<String>,
    pub is_pool: Option<bool>,
    pub mark_utilized: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PrefixUpdatePayload {
    pub prefix: Option<String>,
    pub status: Option<String>,
    pub site_id: Option<String>,
    pub vrf_id: Option<String>,
    pub tenant_id: Option<String>,
    pub vlan_id: Option<String>,
    pub role_id: Option<String>,
    pub is_pool: Option<bool>,
    pub mark_utilized: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PrefixBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<PrefixCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrefixResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub prefix: String,
    pub status: String,
    pub site_id: Option<Uuid>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub vlan_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub is_pool: bool,
    pub mark_utilized: bool,
    pub description: Option<String>,
    pub utilized_addresses: i64,
    pub usable_addresses: Option<i64>,
    pub utilization_percent: Option<f64>,
}

/// Hydrated prefix list item — extends `PrefixResponse` with site/vrf/tenant
/// names for the collection list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrefixListItemResponse {
    pub id: Uuid,
    pub custom_fields: Option<serde_json::Value>,
    pub prefix: String,
    pub status: String,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub vrf_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub vlan_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub is_pool: bool,
    pub mark_utilized: bool,
    pub description: Option<String>,
    pub utilized_addresses: i64,
    pub usable_addresses: Option<i64>,
    pub utilization_percent: Option<f64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkPrefixResponse {
    pub created: Vec<PrefixResponse>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrefixTreeAppliedFiltersResponse {
    pub search: Option<String>,
    pub root_id: Option<Uuid>,
    pub root_prefix: Option<String>,
    pub family: Option<i32>,
    pub site_id: Vec<Uuid>,
    pub vrf_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub limit: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrefixTreeNodeResponse {
    pub id: Uuid,
    pub node_id: String,
    pub prefix: String,
    pub parent_id: Option<Uuid>,
    pub parent_prefix: Option<String>,
    pub root_id: Option<Uuid>,
    pub root_prefix: Option<String>,
    pub depth: i32,
    pub child_count: i64,
    pub status: String,
    pub family: i32,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub vrf_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub mark_utilized: bool,
    pub utilized_addresses: i64,
    pub usable_addresses: Option<i64>,
    pub utilization_percent: Option<f64>,
    pub breadcrumbs: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrefixTreeResponse {
    pub nodes: Vec<PrefixTreeNodeResponse>,
    pub total: i64,
    pub limit: u32,
    pub applied_filters: PrefixTreeAppliedFiltersResponse,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrefixPlanningPreviewResponse {
    pub parent_id: Uuid,
    pub parent_prefix: String,
    pub requested_prefix_length: i32,
    pub status: String,
    pub candidate_prefix: Option<String>,
    pub reason: Option<String>,
    pub conflicts: Vec<String>,
    pub searched_candidates: u32,
}

super::super::impl_inventory_custom_fields!(PrefixResponse, "ipam.prefix");
super::super::impl_inventory_custom_fields!(PrefixListItemResponse, "ipam.prefix");

fn map_prefix_list_item(item: PrefixListItem) -> PrefixListItemResponse {
    let PrefixListItem {
        record,
        site_name,
        vrf_name,
        tenant_name,
        utilized_addresses,
        usable_addresses,
        utilization_percent,
    } = item;
    PrefixListItemResponse {
        id: record.id,
        custom_fields: None,
        prefix: record.prefix,
        status: record.status,
        site_id: record.site_id,
        site_name,
        vrf_id: record.vrf_id,
        vrf_name,
        tenant_id: record.tenant_id,
        tenant_name,
        vlan_id: record.vlan_id,
        role_id: record.role_id,
        is_pool: record.is_pool,
        mark_utilized: record.mark_utilized,
        description: record.description,
        utilized_addresses,
        usable_addresses,
        utilization_percent,
    }
}

fn map_prefix(record: PrefixRecord) -> PrefixResponse {
    PrefixResponse {
        id: record.id,
        custom_fields: None,
        prefix: record.prefix,
        status: record.status,
        site_id: record.site_id,
        vrf_id: record.vrf_id,
        tenant_id: record.tenant_id,
        vlan_id: record.vlan_id,
        role_id: record.role_id,
        is_pool: record.is_pool,
        mark_utilized: record.mark_utilized,
        description: record.description,
        utilized_addresses: 0,
        usable_addresses: None,
        utilization_percent: None,
    }
}

fn map_tree_node(node: PrefixTreeNode) -> PrefixTreeNodeResponse {
    PrefixTreeNodeResponse {
        id: node.id,
        node_id: node.node_id,
        prefix: node.prefix,
        parent_id: node.parent_id,
        parent_prefix: node.parent_prefix,
        root_id: node.root_id,
        root_prefix: node.root_prefix,
        depth: node.depth,
        child_count: node.child_count,
        status: node.status,
        family: node.family,
        site_id: node.site_id,
        site_name: node.site_name,
        vrf_id: node.vrf_id,
        vrf_name: node.vrf_name,
        tenant_id: node.tenant_id,
        tenant_name: node.tenant_name,
        mark_utilized: node.mark_utilized,
        utilized_addresses: node.utilized_addresses,
        usable_addresses: node.usable_addresses,
        utilization_percent: node.utilization_percent,
        breadcrumbs: node.breadcrumbs,
    }
}

fn map_tree(model: PrefixTreeReadModel) -> PrefixTreeResponse {
    PrefixTreeResponse {
        nodes: model.nodes.into_iter().map(map_tree_node).collect(),
        total: model.total,
        limit: model.limit,
        applied_filters: PrefixTreeAppliedFiltersResponse {
            search: model.applied.search,
            root_id: model.applied.root_id,
            root_prefix: model.applied.root_prefix,
            family: model.applied.family,
            site_id: model.applied.site_id,
            vrf_id: model.applied.vrf_id,
            tenant_id: model.applied.tenant_id,
            status: model.applied.status,
            limit: model.limit,
        },
    }
}

fn map_planning_preview(preview: PrefixPlanningPreview) -> PrefixPlanningPreviewResponse {
    PrefixPlanningPreviewResponse {
        parent_id: preview.parent_id,
        parent_prefix: preview.parent_prefix,
        requested_prefix_length: preview.requested_prefix_length,
        status: preview.status,
        candidate_prefix: preview.candidate_prefix,
        reason: preview.reason,
        conflicts: preview.conflicts,
        searched_candidates: preview.searched_candidates,
    }
}

fn map_bulk_prefix_result(result: BulkPrefixResult) -> BulkPrefixResponse {
    BulkPrefixResponse {
        created: result.created.into_iter().map(map_prefix).collect(),
        errors: result.errors,
    }
}

/// List prefixes.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/prefixes",
    tag = "inventory",
    params(PrefixListQuery),
    responses(
        (status = 200, description = "Prefixes", body = InventoryListEnvelope<PrefixListItemResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_prefixes(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PrefixListQuery>,
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
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_csv_uuids(query.vrf_id.as_deref(), "vrf_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_csv_uuids(query.role_id.as_deref(), "role_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PREFIX_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let filter = PrefixFilter {
        ids,
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        site_id,
        vrf_id,
        role_id,
        tenant_id,
        status,
        is_pool: parse_csv_strings(query.is_pool.as_deref()),
        mark_utilized: parse_csv_strings(query.mark_utilized.as_deref()),
        sort,
    };

    match state
        .repositories
        .prefixes
        .list_hydrated(filter, query.limit, query.offset)
        .await
    {
        Ok((records, total)) => {
            let mut items: Vec<PrefixListItemResponse> =
                records.into_iter().map(map_prefix_list_item).collect();
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

/// List the prefix tree.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/prefixes/tree",
    tag = "inventory",
    params(PrefixTreeQuery),
    responses(
        (status = 200, description = "Prefix tree", body = PrefixTreeResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_prefix_tree(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PrefixTreeQuery>,
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

    let root_id = match parse_optional_uuid(query.root_id, "root_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = match parse_csv_uuids(query.site_id.as_deref(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_csv_uuids(query.vrf_id.as_deref(), "vrf_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let family = match query.family {
        Some(4) => Some(4),
        Some(6) => Some(6),
        Some(other) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid family {other}: expected 4 or 6"
            )));
        }
        None => None,
    };
    let root_prefix = query
        .root_prefix
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if root_id.is_some() && root_prefix.is_some() {
        return handle_app_error(AppError::Validation(
            "Use either root_id or root_prefix, not both".to_string(),
        ));
    }

    let filter = PrefixTreeFilter {
        search: crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
        root_id,
        root_prefix,
        family,
        site_id,
        vrf_id,
        tenant_id,
        status: parse_csv_strings(query.status.as_deref()),
    };
    match state
        .repositories
        .prefixes
        .list_tree(filter, query.limit)
        .await
    {
        Ok(model) => (StatusCode::OK, Json(map_tree(model))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a prefix.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/prefixes/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Prefix ID"),
    ),
    responses(
        (status = 200, description = "Prefix", body = PrefixResponse),
        (status = 404, description = "Prefix not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_prefix(
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    match state.repositories.prefixes.get_by_id(id).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, map_prefix(record))
                .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Prefix not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Preview the next available IPv4 child prefix under a parent prefix.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/prefixes/{id}/next-available-prefix/preview",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Parent prefix ID"),
        PrefixPlanningPreviewQuery,
    ),
    responses(
        (status = 200, description = "Next available prefix preview", body = PrefixPlanningPreviewResponse),
        (status = 404, description = "Parent prefix not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_next_available_prefix(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PrefixPlanningPreviewQuery>,
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };

    match state
        .repositories
        .prefixes
        .preview_next_available_prefix(id, query.prefix_length)
        .await
    {
        Ok(preview) => (StatusCode::OK, Json(map_planning_preview(preview))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Validate one prefix create payload and build its row. The FK fields are
/// parsed in one place for both the single and bulk routes; a bulk call passes
/// the item `index` so a failure names the offending row.
fn build_new_prefix(
    index: Option<usize>,
    payload: PrefixCreatePayload,
) -> Result<NewPrefix, AppError> {
    let context = |error: AppError| match index {
        Some(index) => AppError::Validation(format!("item {index}: {error:?}")),
        None => error,
    };
    let prefix = parse_required_string(&payload.prefix, "prefix").map_err(&context)?;
    let site_id = parse_optional_uuid(payload.site_id, "site_id").map_err(&context)?;
    let tenant_id = parse_optional_uuid(payload.tenant_id, "tenant_id").map_err(&context)?;
    let vlan_id = parse_optional_uuid(payload.vlan_id, "vlan_id").map_err(&context)?;
    let vrf_id = parse_optional_uuid(payload.vrf_id, "vrf_id").map_err(&context)?;
    let role_id = parse_optional_uuid(payload.role_id, "role_id").map_err(&context)?;
    Ok(NewPrefix {
        id: Uuid::now_v7(),
        prefix,
        status: payload.status,
        site_id,
        vrf_id,
        tenant_id,
        vlan_id,
        role_id,
        is_pool: payload.is_pool,
        mark_utilized: payload.mark_utilized,
        description: payload.description,
    })
}

/// Create a prefix.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/prefixes",
    tag = "inventory",
    request_body = PrefixCreatePayload,
    responses(
        (status = 201, description = "Prefix created", body = PrefixResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_prefix(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PrefixCreatePayload>,
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

    let new_prefix = match build_new_prefix(None, payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state.repositories.prefixes.create(&new_prefix).await {
        Ok(record) => {
            super::super::masked_inventory_response(&state, StatusCode::CREATED, map_prefix(record))
                .await
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a prefix.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/prefixes/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Prefix ID"),
    ),
    request_body = PrefixUpdatePayload,
    responses(
        (status = 200, description = "Prefix updated", body = PrefixResponse),
        (status = 404, description = "Prefix not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_prefix(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PrefixUpdatePayload>,
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    let site_id = match parse_optional_uuid(payload.site_id, "site_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id, "tenant_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let vlan_id = match parse_optional_uuid(payload.vlan_id, "vlan_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let vrf_id = match parse_optional_uuid(payload.vrf_id, "vrf_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_optional_uuid(payload.role_id, "role_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdatePrefix {
        prefix: payload.prefix,
        status: payload.status,
        site_id,
        vrf_id,
        tenant_id,
        vlan_id,
        role_id,
        is_pool: payload.is_pool,
        mark_utilized: payload.mark_utilized,
        description: payload.description,
    };

    match state.repositories.prefixes.update(id, &changes).await {
        Ok(Some(record)) => {
            super::super::masked_inventory_response(&state, StatusCode::OK, map_prefix(record))
                .await
        }
        Ok(None) => handle_app_error(AppError::NotFound("Prefix not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a prefix.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/prefixes/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Prefix ID"),
    ),
    responses(
        (status = 204, description = "Prefix deleted"),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_prefix(
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
    let id = match id.parse::<Uuid>() {
        Ok(v) => v,
        Err(_) => return handle_app_error(AppError::Validation("invalid id".to_string())),
    };
    match state.repositories.prefixes.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create prefixes.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/prefixes/bulk",
    tag = "inventory",
    request_body = PrefixBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk prefix create result", body = BulkPrefixResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_prefixes(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PrefixBulkCreatePayload>,
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
        match build_new_prefix(Some(index), p) {
            Ok(item) => items.push(item),
            Err(error) => return handle_app_error(error),
        }
    }

    match state.repositories.prefixes.bulk_create(&items).await {
        Ok(result) => {
            let mut body = map_bulk_prefix_result(result);
            match super::super::mask_inventory_projection_page(&state, &mut body.created).await {
                Ok(()) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}
