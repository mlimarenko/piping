//! Monitoring metric sets routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::app::AppState;
use crate::errors::AppError;

use super::helpers::normalize_string;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::contexts::access_control::domain::Action;

const MONITORING_LIST_RENDER_TYPES: [&str; 5] = ["auto", "dot", "value", "boolean", "gauge"];

fn metric_set_assignment_list_filter(
    query: &MonitoringScopeListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::METRIC_SET_ASSIGNMENT_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("metric_set_id", query.metric_set_id.as_deref()),
            ("scope_type", query.scope_type.as_deref()),
            ("scope_id", query.scope_id.as_deref()),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

// --- metric sets (+ items) --------------------------------------------------

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricSetItemEntry {
    pub id: Uuid,
    pub metric_set_id: Uuid,
    pub metric_query_id: Uuid,
    pub metric_query_slug: String,
    pub metric_query_name: String,
    pub show_in_inspector: bool,
    pub show_in_list: bool,
    pub show_in_hover: bool,
    pub list_render: String,
    pub sort_order: i32,
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricSetDetail {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub items: Vec<MetricSetItemEntry>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricSetListResponse {
    pub items: Vec<MetricSetDetail>,
    pub total: usize,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MetricSetListQuery {
    pub sort: Option<String>,
    pub order: Option<String>,
    pub enabled: Option<String>,
}

fn metric_set_list_filter(
    query: &MetricSetListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::METRIC_SET_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[("enabled", query.enabled.as_deref())],
    )
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricSetRequest {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn default_list_render() -> String {
    "auto".to_string()
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricSetItemRequest {
    pub metric_query_id: Uuid,
    #[serde(default)]
    pub show_in_inspector: Option<bool>,
    #[serde(default)]
    pub show_in_list: bool,
    #[serde(default)]
    pub show_in_hover: bool,
    #[serde(default = "default_list_render")]
    pub list_render: String,
    #[serde(default)]
    pub sort_order: Option<i32>,
    #[serde(default)]
    pub label_override: Option<String>,
    #[serde(default)]
    pub unit_override: Option<String>,
}

fn metric_set_item_entry(record: repository::MetricSetItemRecord) -> MetricSetItemEntry {
    MetricSetItemEntry {
        id: record.id,
        metric_set_id: record.metric_set_id,
        metric_query_id: record.metric_query_id,
        metric_query_slug: record.metric_query_slug,
        metric_query_name: record.metric_query_name,
        show_in_inspector: record.show_in_inspector,
        show_in_list: record.show_in_list,
        show_in_hover: record.show_in_hover,
        list_render: record.list_render,
        sort_order: record.sort_order,
        label_override: record.label_override,
        unit_override: record.unit_override,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn metric_set_detail(
    record: repository::MetricSetRecord,
    items: Vec<MetricSetItemEntry>,
) -> MetricSetDetail {
    MetricSetDetail {
        id: record.id,
        name: record.name,
        slug: record.slug,
        description: record.description,
        enabled: record.enabled,
        items,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn validate_metric_set(payload: MetricSetRequest) -> Result<repository::MetricSetInput, AppError> {
    let name = normalize_string(Some(&payload.name))
        .ok_or_else(|| AppError::Validation("name must not be empty".to_string()))?;
    let slug = normalize_string(Some(&payload.slug))
        .ok_or_else(|| AppError::Validation("slug must not be empty".to_string()))?;
    Ok(repository::MetricSetInput {
        name,
        slug,
        description: normalize_string(payload.description.as_deref()),
        enabled: payload.enabled.unwrap_or(true),
    })
}

fn validate_metric_set_item(
    payload: MetricSetItemRequest,
) -> Result<repository::MetricSetItemInput, AppError> {
    let list_render = validate_enum_value(
        "list_render",
        &payload.list_render,
        &MONITORING_LIST_RENDER_TYPES,
    )?;
    Ok(repository::MetricSetItemInput {
        metric_query_id: payload.metric_query_id,
        show_in_inspector: payload.show_in_inspector.unwrap_or(true),
        show_in_list: payload.show_in_list,
        show_in_hover: payload.show_in_hover,
        list_render,
        sort_order: payload.sort_order.unwrap_or(0),
        label_override: normalize_string(payload.label_override.as_deref()),
        unit_override: normalize_string(payload.unit_override.as_deref()),
    })
}

/// List monitoring metric sets, including their nested items.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/metric-sets",
    tag = "monitoring",
    params(MetricSetListQuery),
    responses(
        (status = 200, description = "Monitoring metric sets with nested items", body = MetricSetListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_metric_sets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<MetricSetListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match metric_set_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let sets = match repository::list_metric_sets(&state.db_pool, &filter).await {
        Ok(records) => records,
        Err(error) => return handle_app_error(error),
    };
    let set_ids = sets.iter().map(|set| set.id).collect::<Vec<_>>();
    let item_records = match repository::list_metric_set_items(&state.db_pool, &set_ids).await {
        Ok(records) => records,
        Err(error) => return handle_app_error(error),
    };

    let mut items_by_set = HashMap::<Uuid, Vec<MetricSetItemEntry>>::new();
    for record in item_records {
        items_by_set
            .entry(record.metric_set_id)
            .or_default()
            .push(metric_set_item_entry(record));
    }

    let items: Vec<MetricSetDetail> = sets
        .into_iter()
        .map(|set| {
            let entries = items_by_set.remove(&set.id).unwrap_or_default();
            metric_set_detail(set, entries)
        })
        .collect();

    (
        StatusCode::OK,
        Json(MetricSetListResponse {
            total: items.len(),
            items,
        }),
    )
        .into_response()
}

async fn metric_set_item_with_items(
    state: &AppState,
    record: repository::MetricSetRecord,
) -> Result<MetricSetDetail, AppError> {
    let item_records = repository::list_metric_set_items(&state.db_pool, &[record.id]).await?;
    let entries = item_records
        .into_iter()
        .map(metric_set_item_entry)
        .collect();
    Ok(metric_set_detail(record, entries))
}

/// Create a monitoring metric set.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/metric-sets",
    tag = "monitoring",
    request_body = MetricSetRequest,
    responses(
        (status = 201, description = "Metric set created", body = MetricSetDetail),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_metric_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MetricSetRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_set(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let record = match repository::create_metric_set(&state.db_pool, &input).await {
        Ok(record) => record,
        Err(error) => return handle_app_error(error),
    };
    match metric_set_item_with_items(&state, record).await {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a monitoring metric set.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/metric-sets/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric set id")),
    request_body = MetricSetRequest,
    responses(
        (status = 200, description = "Metric set updated", body = MetricSetDetail),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Metric set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_metric_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<MetricSetRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_set(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let record = match repository::update_metric_set(&state.db_pool, id, &input).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Metric set not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    };
    match metric_set_item_with_items(&state, record).await {
        Ok(item) => (StatusCode::OK, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring metric set.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/metric-sets/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric set id")),
    responses(
        (status = 204, description = "Metric set deleted"),
        (status = 404, description = "Metric set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_metric_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_metric_set(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Metric set not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Add an item to a monitoring metric set.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/metric-sets/{id}/items",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric set id")),
    request_body = MetricSetItemRequest,
    responses(
        (status = 201, description = "Metric set item created", body = MetricSetItemEntry),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Metric set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_metric_set_item(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<MetricSetItemRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_set_item(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::metric_set_exists(&state.db_pool, id).await {
        Ok(true) => {}
        Ok(false) => {
            return handle_app_error(AppError::NotFound("Metric set not found".to_string()));
        }
        Err(error) => return handle_app_error(error),
    }

    match repository::create_metric_set_item(&state.db_pool, id, &input).await {
        Ok(record) => (StatusCode::CREATED, Json(metric_set_item_entry(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an item in a monitoring metric set.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/metric-sets/{id}/items/{item_id}",
    tag = "monitoring",
    params(
        ("id" = Uuid, Path, description = "Metric set id"),
        ("item_id" = Uuid, Path, description = "Metric set item id"),
    ),
    request_body = MetricSetItemRequest,
    responses(
        (status = 200, description = "Metric set item updated", body = MetricSetItemEntry),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Metric set item not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_metric_set_item(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
    EnvelopeJson(payload): EnvelopeJson<MetricSetItemRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_set_item(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::update_metric_set_item(&state.db_pool, id, item_id, &input).await {
        Ok(Some(record)) => (StatusCode::OK, Json(metric_set_item_entry(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Metric set item not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an item from a monitoring metric set.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/metric-sets/{id}/items/{item_id}",
    tag = "monitoring",
    params(
        ("id" = Uuid, Path, description = "Metric set id"),
        ("item_id" = Uuid, Path, description = "Metric set item id"),
    ),
    responses(
        (status = 204, description = "Metric set item deleted"),
        (status = 404, description = "Metric set item not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_metric_set_item(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_metric_set_item(&state.db_pool, id, item_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Metric set item not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

// --- metric set assignments -------------------------------------------------

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricSetAssignmentItem {
    pub id: Uuid,
    pub metric_set_id: Uuid,
    pub metric_set_slug: String,
    pub metric_set_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricSetAssignmentListResponse {
    pub items: Vec<MetricSetAssignmentItem>,
    pub total: usize,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricSetAssignmentRequest {
    pub metric_set_id: Uuid,
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<Uuid>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn metric_set_assignment_item(
    record: repository::MetricSetAssignmentRecord,
) -> MetricSetAssignmentItem {
    MetricSetAssignmentItem {
        id: record.id,
        metric_set_id: record.metric_set_id,
        metric_set_slug: record.metric_set_slug,
        metric_set_name: record.metric_set_name,
        scope_type: record.scope_type,
        scope_id: record.scope_id,
        weight: record.weight,
        enabled: record.enabled,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn validate_metric_set_assignment(
    payload: MetricSetAssignmentRequest,
) -> Result<repository::MetricSetAssignmentInput, AppError> {
    let scope_type = validate_monitoring_scope(&payload.scope_type, payload.scope_id)?;
    Ok(repository::MetricSetAssignmentInput {
        metric_set_id: payload.metric_set_id,
        scope_type,
        scope_id: payload.scope_id,
        weight: payload.weight.unwrap_or(0),
        enabled: payload.enabled.unwrap_or(true),
    })
}

/// List monitoring metric set assignments.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/metric-set-assignments",
    tag = "monitoring",
    params(MonitoringScopeListQuery),
    responses(
        (status = 200, description = "Metric set assignments matching the scope filters", body = MetricSetAssignmentListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_metric_set_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<MonitoringScopeListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match metric_set_assignment_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::list_metric_set_assignments(&state.db_pool, &filter).await {
        Ok(records) => {
            let items: Vec<MetricSetAssignmentItem> = records
                .into_iter()
                .map(metric_set_assignment_item)
                .collect();
            (
                StatusCode::OK,
                Json(MetricSetAssignmentListResponse {
                    total: items.len(),
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a monitoring metric set assignment.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/metric-set-assignments",
    tag = "monitoring",
    request_body = MetricSetAssignmentRequest,
    responses(
        (status = 201, description = "Metric set assignment created", body = MetricSetAssignmentItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_metric_set_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MetricSetAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_set_assignment(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::create_metric_set_assignment(&state.db_pool, &input).await {
        Ok(record) => (
            StatusCode::CREATED,
            Json(metric_set_assignment_item(record)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a monitoring metric set assignment.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/metric-set-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric set assignment id")),
    request_body = MetricSetAssignmentRequest,
    responses(
        (status = 200, description = "Metric set assignment updated", body = MetricSetAssignmentItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Metric set assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_metric_set_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<MetricSetAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_set_assignment(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::update_metric_set_assignment(&state.db_pool, id, &input).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(metric_set_assignment_item(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "Metric set assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring metric set assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/metric-set-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric set assignment id")),
    responses(
        (status = 204, description = "Metric set assignment deleted"),
        (status = 404, description = "Metric set assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_metric_set_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_metric_set_assignment(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Metric set assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}
