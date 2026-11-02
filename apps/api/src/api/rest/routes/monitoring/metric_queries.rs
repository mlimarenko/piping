//! Monitoring metric queries routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::app::AppState;
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::contexts::inventory::infrastructure::persistence::DiscoveryRepository;
use crate::contexts::secrets::application::SecretStore;
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

use super::helpers::normalize_string;
use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::shared::request_context::RequestContext;

use crate::contexts::monitoring::application::{
    host_labels_cache_key, pin_monitoring_query_url, redacted_url,
};
use crate::contexts::monitoring::domain::{
    LatestMetricSourceInfo, MonitoringMetricDescriptor, MonitoringQuerySourceConfig,
    ResolvedMetricQuerySource, is_prometheus_compatible_source_type, is_zabbix_source_type,
    render_host_metric_query, render_zabbix_host_selector,
};
use crate::contexts::monitoring::infrastructure::persistence as monitoring_persistence;
use crate::contexts::monitoring::infrastructure::providers::{
    fetch_latest_prometheus_metric, fetch_latest_zabbix_metric, metric_query_http_client,
};

const MONITORING_QUERY_TYPES: [&str; 4] = [
    "promql",
    "zabbix_item_key",
    "zabbix_item_name",
    "zabbix_itemid",
];

const MONITORING_VALUE_TYPES: [&str; 10] = [
    "auto",
    "state",
    "number",
    "percent",
    "bytes",
    "duration",
    "text",
    "timestamp",
    "log",
    "table",
];

const MONITORING_WIDGET_TYPES: [&str; 6] = ["stat", "badge", "gauge", "sparkline", "table", "log"];

const MONITORING_METRIC_SEMANTICS: [&str; 1] = ["availability"];

fn metric_query_type_is_compatible(source_type: &str, query_type: &str) -> bool {
    if is_zabbix_source_type(source_type) {
        return matches!(
            query_type,
            "zabbix_item_key" | "zabbix_item_name" | "zabbix_itemid"
        );
    }
    if is_prometheus_compatible_source_type(source_type) {
        return query_type == "promql";
    }
    false
}

fn trim_required_case_sensitive(field: &str, value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Validation(format!("{field} must not be empty")));
    }
    Ok(value.to_string())
}

fn validate_optional_enum_value(
    field: &str,
    value: Option<&str>,
    allowed: &[&str],
) -> Result<Option<String>, AppError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| validate_enum_value(field, value, allowed))
        .transpose()
}

// --- metric query display config (typed thresholds) -------------------------

/// Numeric or state threshold value used by `MetricQueryDisplayStatus`.
///
/// In `mode = numeric` (or `mode = auto` with numeric metric data) thresholds
/// are a single number compared against the resolved metric value. In
/// `mode = state` thresholds are a list of literal tokens (e.g. `["up","1"]`)
/// matched case-insensitively against the resolved metric value.
///
/// The untagged shape mirrors the wire format consumed by the front-end
/// editor (`apps/web/src/features/observability/metricDisplay.ts`).
#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum MetricQueryDisplayThreshold {
    Numeric(f64),
    State(Vec<String>),
}

/// Status-rule block under `options.display.status`. Mirrors the shape
/// consumed by the front-end card status decoder.
#[derive(Debug, Clone, Default, Deserialize, Serialize, utoipa::ToSchema)]
pub struct MetricQueryDisplayStatus {
    /// Selector for the matching rule. One of:
    /// `auto` (default), `numeric`, `state`, `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// For `mode = numeric`, whether higher or lower numeric values are worse.
    /// One of: `higher_is_worse` (default), `lower_is_worse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// Threshold below which (or above, depending on `direction`) the card
    /// flips to `warning`. Numeric when `mode = numeric`, list of state
    /// literals when `mode = state`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<MetricQueryDisplayThreshold>,
    /// Threshold at which the card flips to `critical`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical: Option<MetricQueryDisplayThreshold>,
    /// State literals counted as `normal`. Only used in `mode = state`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<Vec<String>>,
}

/// Typed mirror of the `options.display` subtree of a `MetricQueryRequest`.
///
/// Wire-compatible with the free-form `options.display` JSON consumed by the
/// front-end editor. Malformed bodies are rejected before persistence.
#[derive(Debug, Clone, Default, Deserialize, Serialize, utoipa::ToSchema)]
pub struct MetricQueryDisplayConfig {
    /// Threshold/state rule block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MetricQueryDisplayStatus>,
}

/// Allowed `display.status.mode` values.
const METRIC_DISPLAY_STATUS_MODES: &[&str] = &["auto", "numeric", "state", "none"];

/// Allowed `display.status.direction` values.
const METRIC_DISPLAY_STATUS_DIRECTIONS: &[&str] = &["higher_is_worse", "lower_is_worse"];

fn validate_numeric_threshold(
    threshold: Option<&MetricQueryDisplayThreshold>,
    field: &str,
) -> Result<Option<f64>, AppError> {
    match threshold {
        None => Ok(None),
        Some(MetricQueryDisplayThreshold::Numeric(value)) => Ok(Some(*value)),
        Some(MetricQueryDisplayThreshold::State(_)) => Err(AppError::Validation(format!(
            "{field} must be a number when status.mode is `numeric`"
        ))),
    }
}

fn validate_numeric_status(
    status: &MetricQueryDisplayStatus,
    status_field: &str,
) -> Result<(), AppError> {
    let warning =
        validate_numeric_threshold(status.warning.as_ref(), &format!("{status_field}.warning"))?;
    let critical = validate_numeric_threshold(
        status.critical.as_ref(),
        &format!("{status_field}.critical"),
    )?;
    let Some((warning, critical)) = warning.zip(critical) else {
        return Ok(());
    };
    let direction = status
        .direction
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "higher_is_worse".to_string());
    if direction == "higher_is_worse" && warning >= critical {
        return Err(AppError::Validation(format!(
            "{status_field}: warning ({warning}) must be < critical ({critical}) when direction is `higher_is_worse`"
        )));
    }
    if direction == "lower_is_worse" && warning <= critical {
        return Err(AppError::Validation(format!(
            "{status_field}: warning ({warning}) must be > critical ({critical}) when direction is `lower_is_worse`"
        )));
    }
    Ok(())
}

fn validate_state_array(values: Option<&Vec<String>>, field: &str) -> Result<(), AppError> {
    let Some(values) = values else { return Ok(()) };
    let mut seen = HashSet::<String>::with_capacity(values.len());
    for value in values {
        if !seen.insert(value.trim().to_ascii_lowercase()) {
            return Err(AppError::Validation(format!(
                "{field} contains duplicate entry `{value}`"
            )));
        }
    }
    Ok(())
}

fn validate_state_status(
    status: &MetricQueryDisplayStatus,
    status_field: &str,
) -> Result<(), AppError> {
    if matches!(
        status.warning.as_ref(),
        Some(MetricQueryDisplayThreshold::Numeric(_))
    ) {
        return Err(AppError::Validation(format!(
            "{status_field}.warning must be a list of state tokens when status.mode is `state`"
        )));
    }
    if matches!(
        status.critical.as_ref(),
        Some(MetricQueryDisplayThreshold::Numeric(_))
    ) {
        return Err(AppError::Validation(format!(
            "{status_field}.critical must be a list of state tokens when status.mode is `state`"
        )));
    }
    Ok(())
}

/// Validate the `options.display` subtree of a monitoring metric query.
///
/// Parses the subtree as `MetricQueryDisplayConfig` and enforces the
/// invariants the FE editor and runtime cards both rely on:
/// - `mode` is one of `auto|numeric|state|none`,
/// - `direction` is one of `higher_is_worse|lower_is_worse`,
/// - in `mode = numeric`, `warning`/`critical` are numbers (not state arrays),
///   and `warning < critical` when `direction = higher_is_worse`
///   (resp. `warning > critical` for `lower_is_worse`) — strict inequality
///   so a `warning == critical` rule cannot persist as a no-trip,
/// - in `mode = state`, `warning`/`critical`/`normal` are string arrays with
///   no duplicates (case-insensitively).
///
/// `field_path` is used to namespace error messages, e.g. `"options"` for the
/// metric-query body, or `"options_override"` for the host-override body.
pub(crate) fn validate_display_options(field_path: &str, options: &Value) -> Result<(), AppError> {
    let Value::Object(map) = options else {
        return Ok(());
    };
    let Some(display) = map.get("display") else {
        return Ok(());
    };
    let display_field = format!("{field_path}.display");
    let config: MetricQueryDisplayConfig =
        serde_json::from_value(display.clone()).map_err(|e| {
            AppError::Validation(format!(
                "{display_field} must match the display-config schema: {e}"
            ))
        })?;
    let Some(status) = config.status.as_ref() else {
        return Ok(());
    };
    let status_field = format!("{display_field}.status");

    let mode_raw = status.mode.as_deref().unwrap_or("auto");
    let mode = mode_raw.trim().to_ascii_lowercase();
    if !METRIC_DISPLAY_STATUS_MODES.contains(&mode.as_str()) {
        return Err(AppError::Validation(format!(
            "{status_field}.mode must be one of: {}",
            METRIC_DISPLAY_STATUS_MODES.join(", ")
        )));
    }

    if let Some(direction) = status.direction.as_deref() {
        let direction = direction.trim().to_ascii_lowercase();
        if !METRIC_DISPLAY_STATUS_DIRECTIONS.contains(&direction.as_str()) {
            return Err(AppError::Validation(format!(
                "{status_field}.direction must be one of: {}",
                METRIC_DISPLAY_STATUS_DIRECTIONS.join(", ")
            )));
        }
    }

    if mode == "numeric" {
        validate_numeric_status(status, &status_field)?;
    }

    // State mode: warning/critical/normal must be string arrays without
    // duplicates (case-insensitive). Plain `auto` mode also enforces the
    // dup-check whenever those keys ARE string arrays, since the FE consumer
    // falls back to the default state rules only on empty arrays.
    let warning_states = match status.warning.as_ref() {
        Some(MetricQueryDisplayThreshold::State(arr)) => Some(arr),
        _ => None,
    };
    let critical_states = match status.critical.as_ref() {
        Some(MetricQueryDisplayThreshold::State(arr)) => Some(arr),
        _ => None,
    };
    let normal_states = status.normal.as_ref();
    if mode == "state" {
        validate_state_status(status, &status_field)?;
    }
    validate_state_array(warning_states, &format!("{status_field}.warning"))?;
    validate_state_array(critical_states, &format!("{status_field}.critical"))?;
    validate_state_array(normal_states, &format!("{status_field}.normal"))?;

    Ok(())
}

// --- metric queries ---------------------------------------------------------

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricQueryItem {
    pub id: Uuid,
    pub exporter_id: Option<Uuid>,
    pub source_type: String,
    pub query_type: String,
    pub value_type: String,
    pub widget_type: String,
    pub name: String,
    pub slug: String,
    pub label: Option<String>,
    pub query: String,
    pub unit: Option<String>,
    pub description: Option<String>,
    pub default_show_in_inspector: bool,
    pub default_show_in_list: bool,
    pub sort_order: i32,
    pub options: Value,
    pub enabled: bool,
    /// Optional visual grouping label (e.g. "System", "Containers"). NULL means
    /// the metric is uncategorized.
    pub category: Option<String>,
    /// Optional machine-readable metric meaning. `availability` drives workspace up/down.
    pub semantic: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricQueryListResponse {
    pub items: Vec<MetricQueryItem>,
    pub total: usize,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MetricQueryListQuery {
    pub exporter_id: Option<String>,
    pub source_type: Option<String>,
    pub query_type: Option<String>,
    pub value_type: Option<String>,
    pub widget_type: Option<String>,
    pub enabled: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

fn metric_query_list_filter(
    query: &MetricQueryListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::METRIC_QUERY_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("exporter_id", query.exporter_id.as_deref()),
            ("source_type", query.source_type.as_deref()),
            ("query_type", query.query_type.as_deref()),
            ("value_type", query.value_type.as_deref()),
            ("widget_type", query.widget_type.as_deref()),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricQueryRequest {
    #[serde(default)]
    pub exporter_id: Option<Uuid>,
    #[serde(default)]
    pub source_type: Option<String>,
    #[serde(default)]
    pub query_type: Option<String>,
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub widget_type: Option<String>,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub label: Option<String>,
    pub query: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_show_in_inspector: Option<bool>,
    #[serde(default)]
    pub default_show_in_list: bool,
    #[serde(default)]
    pub sort_order: Option<i32>,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub semantic: Option<String>,
}

fn metric_query_item(record: repository::MetricQueryRecord) -> MetricQueryItem {
    MetricQueryItem {
        id: record.id,
        exporter_id: record.exporter_id,
        source_type: record.source_type,
        query_type: record.query_type,
        value_type: record.value_type,
        widget_type: record.widget_type,
        name: record.name,
        slug: record.slug,
        label: record.label,
        query: record.query,
        unit: record.unit,
        description: record.description,
        default_show_in_inspector: record.default_show_in_inspector,
        default_show_in_list: record.default_show_in_list,
        sort_order: record.sort_order,
        options: record.options,
        enabled: record.enabled,
        category: record.category,
        semantic: record.semantic,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

pub(crate) fn validate_metric_query(
    payload: MetricQueryRequest,
) -> Result<repository::MetricQueryInput, AppError> {
    let name = normalize_string(Some(&payload.name))
        .ok_or_else(|| AppError::Validation("name must not be empty".to_string()))?;
    let slug = normalize_string(Some(&payload.slug))
        .ok_or_else(|| AppError::Validation("slug must not be empty".to_string()))?;
    let query = trim_required_case_sensitive("query", &payload.query)?;
    let source_type = validate_enum_value(
        "source_type",
        payload.source_type.as_deref().unwrap_or("prometheus"),
        &MONITORING_SOURCE_TYPES,
    )?;
    let query_type = validate_enum_value(
        "query_type",
        payload.query_type.as_deref().unwrap_or("promql"),
        &MONITORING_QUERY_TYPES,
    )?;
    if !metric_query_type_is_compatible(&source_type, &query_type) {
        return Err(AppError::Validation(format!(
            "query_type `{query_type}` is not compatible with source_type `{source_type}`"
        )));
    }
    let value_type = validate_enum_value(
        "value_type",
        payload.value_type.as_deref().unwrap_or("auto"),
        &MONITORING_VALUE_TYPES,
    )?;
    let widget_type = validate_enum_value(
        "widget_type",
        payload.widget_type.as_deref().unwrap_or("stat"),
        &MONITORING_WIDGET_TYPES,
    )?;
    let options = validate_json_object("options", payload.options)?;
    validate_display_options("options", &options)?;
    let semantic = validate_optional_enum_value(
        "semantic",
        payload.semantic.as_deref(),
        &MONITORING_METRIC_SEMANTICS,
    )?;

    Ok(repository::MetricQueryInput {
        exporter_id: payload.exporter_id,
        source_type,
        query_type,
        value_type,
        widget_type,
        name,
        slug,
        label: normalize_string(payload.label.as_deref()),
        query,
        unit: normalize_string(payload.unit.as_deref()),
        description: normalize_string(payload.description.as_deref()),
        default_show_in_inspector: payload.default_show_in_inspector.unwrap_or(false),
        default_show_in_list: payload.default_show_in_list,
        sort_order: payload.sort_order.unwrap_or(0),
        options,
        enabled: payload.enabled.unwrap_or(true),
        category: normalize_string(payload.category.as_deref()),
        semantic,
    })
}

/// List monitoring metric queries.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/metric-queries",
    tag = "monitoring",
    params(MetricQueryListQuery),
    responses(
        (status = 200, description = "Monitoring metric queries", body = MetricQueryListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_metric_queries(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<MetricQueryListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match metric_query_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::list_metric_queries(&state.db_pool, &filter).await {
        Ok(records) => {
            let items: Vec<MetricQueryItem> = records.into_iter().map(metric_query_item).collect();
            (
                StatusCode::OK,
                Json(MetricQueryListResponse {
                    total: items.len(),
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a monitoring metric query.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/metric-queries",
    tag = "monitoring",
    request_body = MetricQueryRequest,
    responses(
        (status = 201, description = "Metric query created", body = MetricQueryItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_metric_query(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MetricQueryRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_query(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::create_metric_query(&state.db_pool, &input).await {
        Ok(record) => (StatusCode::CREATED, Json(metric_query_item(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a monitoring metric query.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/metric-queries/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric query id")),
    request_body = MetricQueryRequest,
    responses(
        (status = 200, description = "Metric query updated", body = MetricQueryItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Metric query not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_metric_query(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<MetricQueryRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_metric_query(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::update_metric_query(&state.db_pool, id, &input).await {
        Ok(Some(record)) => (StatusCode::OK, Json(metric_query_item(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Metric query not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring metric query.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/metric-queries/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Metric query id")),
    responses(
        (status = 204, description = "Metric query deleted"),
        (status = 404, description = "Metric query not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_metric_query(
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

    match repository::delete_metric_query(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Metric query not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

// --- metric query host overrides --------------------------------------------

/// Per-host override of a metric query's label, unit, and display rules.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricQueryHostOverrideItem {
    pub id: Uuid,
    pub metric_query_id: Uuid,
    pub host_id: Uuid,
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
    /// `null` means "inherit the metric-query/set-item display rules".
    /// When present, replaces the `options.display` subtree wholesale.
    pub options_override: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricQueryHostOverrideListResponse {
    pub items: Vec<MetricQueryHostOverrideItem>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MetricQueryHostOverrideListQuery {
    /// Restrict to overrides defined for one host. Required to keep the
    /// scope minimal — callers cannot list every override on the platform
    /// in a single call, which avoids accidental cross-host data leak via
    /// monitoring config read.
    pub host_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricQueryHostOverrideRequest {
    pub metric_query_id: Uuid,
    pub host_id: Uuid,
    #[serde(default)]
    pub label_override: Option<String>,
    #[serde(default)]
    pub unit_override: Option<String>,
    /// Display-rule override block, validated against the
    /// `MetricQueryDisplayConfig` shape.
    #[serde(default)]
    pub options_override: Option<Value>,
}

fn host_override_item(record: repository::HostOverrideRecord) -> MetricQueryHostOverrideItem {
    MetricQueryHostOverrideItem {
        id: record.id,
        metric_query_id: record.metric_query_id,
        host_id: record.host_id,
        label_override: record.label_override,
        unit_override: record.unit_override,
        options_override: record.options_override,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

pub(crate) fn validate_host_override(
    payload: MetricQueryHostOverrideRequest,
) -> Result<repository::HostOverrideInput, AppError> {
    let label = normalize_string(payload.label_override.as_deref());
    let unit = normalize_string(payload.unit_override.as_deref());
    let options_override = match payload.options_override {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => {
            // The override IS the new `display` subtree. Wrap it so the
            // typed validator (which expects `{display: ...}`) can reuse the
            // same code path as `validate_metric_query`.
            let wrapped = Value::Object({
                let mut m = serde_json::Map::new();
                m.insert("display".to_string(), Value::Object(map.clone()));
                m
            });
            validate_display_options("options_override", &wrapped)?;
            Some(Value::Object(map))
        }
        Some(_) => {
            return Err(AppError::Validation(
                "options_override must be a JSON object or null".to_string(),
            ));
        }
    };
    Ok(repository::HostOverrideInput {
        metric_query_id: payload.metric_query_id,
        host_id: payload.host_id,
        label_override: label,
        unit_override: unit,
        options_override,
    })
}

/// List monitoring metric query host overrides, optionally filtered by host.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/host-overrides",
    tag = "monitoring",
    params(MetricQueryHostOverrideListQuery),
    responses(
        (status = 200, description = "Host overrides", body = MetricQueryHostOverrideListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_metric_query_host_overrides(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(filter): Query<MetricQueryHostOverrideListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }
    match repository::list_host_overrides(&state.db_pool, filter.host_id).await {
        Ok(records) => {
            let items: Vec<MetricQueryHostOverrideItem> =
                records.into_iter().map(host_override_item).collect();
            (
                StatusCode::OK,
                Json(MetricQueryHostOverrideListResponse { items }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a monitoring metric query host override.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/host-overrides",
    tag = "monitoring",
    request_body = MetricQueryHostOverrideRequest,
    responses(
        (status = 201, description = "Host override created", body = MetricQueryHostOverrideItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_metric_query_host_override(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MetricQueryHostOverrideRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }
    let input = match validate_host_override(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_host_exists(&state, input.host_id).await {
        return handle_app_error(error);
    }
    match repository::create_host_override(&state.db_pool, &input).await {
        Ok(record) => {
            // Invalidate the per-host list-label cache so the new override
            // shows up on the next list/tree fetch instead of waiting up to
            // one full poller tick for the cache to refresh on its own.
            invalidate_host_labels_cache(&state, record.host_id).await;
            (StatusCode::CREATED, Json(host_override_item(record))).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a monitoring metric query host override.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/host-overrides/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Host override id")),
    request_body = MetricQueryHostOverrideRequest,
    responses(
        (status = 200, description = "Host override updated", body = MetricQueryHostOverrideItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Host override not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_metric_query_host_override(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<MetricQueryHostOverrideRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }
    let input = match validate_host_override(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_host_exists(&state, input.host_id).await {
        return handle_app_error(error);
    }
    // Capture the previous host_id BEFORE the update so we can invalidate
    // its cache too — operators can in principle rebind an override from
    // one host to another, and we want both old and new caches to refresh.
    let previous_host_id = match repository::fetch_host_override_by_id(&state.db_pool, id).await {
        Ok(record) => record.map(|r| r.host_id),
        Err(error) => return handle_app_error(error),
    };
    match repository::update_host_override(&state.db_pool, id, &input).await {
        Ok(Some(record)) => {
            invalidate_host_labels_cache(&state, record.host_id).await;
            if let Some(prev) = previous_host_id
                && prev != record.host_id
            {
                invalidate_host_labels_cache(&state, prev).await;
            }
            (StatusCode::OK, Json(host_override_item(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Host override not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring metric query host override.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/host-overrides/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Host override id")),
    responses(
        (status = 204, description = "Host override deleted"),
        (status = 404, description = "Host override not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_metric_query_host_override(
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
    match repository::delete_host_override(&state.db_pool, id).await {
        Ok(Some(host_id)) => {
            invalidate_host_labels_cache(&state, host_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Host override not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Best-effort delete of the per-host list-label cache entry. Logged but
/// non-fatal — the next poller tick will repopulate either way.
/// Confirm a host id exists in the discovery view (device or VM).
///
/// `host_id` on `monitoring_metric_query_host_override` is intentionally NOT
/// constrained by a DB FK because the discovery view is polymorphic over
/// `dcim_device` + virtual-machine sources. The trade-off is that we must
/// gate writes here with an explicit existence check so create/update cannot
/// persist orphan rows that would linger after the underlying host is
/// removed from inventory.
async fn ensure_host_exists(state: &AppState, host_id: Uuid) -> Result<(), AppError> {
    let filters = DiscoveryHostFilters {
        host_id: Some(host_id),
        fetch_limit: 1,
        ..DiscoveryHostFilters::default()
    };
    let hosts = state
        .repositories
        .discovery
        .list_discovery_hosts(&filters, None)
        .await?;
    if hosts.is_empty() {
        return Err(AppError::NotFound(format!(
            "Host {host_id} not found in inventory"
        )));
    }
    Ok(())
}

async fn invalidate_host_labels_cache(state: &AppState, host_id: Uuid) {
    let mut redis = state.redis.clone();
    let key = host_labels_cache_key(host_id);
    if let Err(error) = crate::storage::cache::redis::cache::delete(&mut redis, &key).await {
        log_error_event(
            "monitoring.host_override.cache_invalidate_failed",
            "host_override_cache_invalidate_failed",
            error.to_string(),
            EventFields::default()
                .with_operation("host_override_cache_invalidate")
                .with_status("failed")
                .with_context("host_id", host_id.to_string()),
        );
    }
}

// --- metric query live test -------------------------------------------------

/// Request body for the live PromQL/Zabbix test endpoint.
///
/// `target_id` is optional. When present, `$host_*` template variables in
/// `query` are rendered against the target host and the response includes the
/// rendered query. When absent, the query is sent verbatim and Zabbix sources
/// are rejected (Zabbix item lookup needs a host context).
///
/// Credentials are NEVER taken from the request body. The endpoint only reuses
/// the credential persisted on an existing `monitoring_query_source` whose
/// URL exactly matches `query_url`; otherwise the upstream is called without
/// a bearer token. This keeps the endpoint at the same authority as `/latest`
/// and prevents arbitrary-secret exfiltration through caller-supplied refs.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricQueryTestRequest {
    /// Metric query string (PromQL, or Zabbix item key/name/itemid when
    /// `target_id` is set and `source_type = zabbix`).
    pub query: String,
    /// One of: `prometheus`, `victoriametrics`, `zabbix`. Defaults to
    /// `prometheus` when omitted.
    #[serde(default)]
    pub source_type: Option<String>,
    /// Source URL to query. When this matches the URL of an existing
    /// `monitoring_query_source` row, the persisted credential and timeout
    /// are reused. Otherwise the request builds a one-shot SSRF-pinned
    /// client and the upstream call is unauthenticated.
    pub query_url: String,
    /// Connection target id used to render `$host_*` placeholders and to
    /// look up scoped query sources. When omitted the query is run against
    /// global query sources only.
    #[serde(default)]
    pub target_id: Option<Uuid>,
}

/// Response from the live PromQL/Zabbix test endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricQueryTestResponse {
    /// The query string that was actually sent upstream (after `$host_*`
    /// template substitution when `target_id` was provided).
    pub executed_query: String,
    /// Raw JSON value returned by the upstream source on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Human-readable error when the upstream call failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True when a configured source matched the request (persisted source
    /// re-use OR one-shot path). False when no matching scope/global source
    /// was found AND no `query_url` was supplied — same shape as `/latest`.
    pub source_resolved: bool,
    /// Best-effort guess of a unit string for the result. Currently never
    /// populated; reserved for future shape detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_unit: Option<String>,
}

/// Provenance gate for the live metric-test SSRF guard.
///
/// The endpoint reuses a bound credential only when the request URL exactly
/// matches an admin-created `monitoring_query_source`, and that URL is trusted
/// only under the same match *and* on the `Monitoring:Update` branch
/// (`target_id` absent). That authority also creates sources, and the
/// source-creation validator rejects only private IP *literals*, not hostnames
/// that resolve privately — so a monitoring admin can already point a source at
/// a co-deployed internal backend and run arbitrary PromQL through it. The
/// `target_id` branch is gated by `ConnectionTarget:Update` alone; there a
/// target-scoped operator must not reach an internal backend with the source's
/// admin credential, so it keeps the strict guard and only honours the matched
/// source's explicit `allow_private_ips` allowlist.
pub(crate) fn metric_test_allows_private_resolution(
    matched_persisted_source: bool,
    target_id: Option<Uuid>,
) -> bool {
    matched_persisted_source && target_id.is_none()
}

async fn metric_query_test_service(
    state: &AppState,
    request_context: &RequestContext,
    payload: MetricQueryTestRequest,
) -> Result<MetricQueryTestResponse, AppError> {
    let query = trim_required_case_sensitive("query", &payload.query)?;
    if query.len() > 512 {
        return Err(AppError::Validation(
            "Metric query length must not exceed 512 characters".to_string(),
        ));
    }
    let source_type = validate_enum_value(
        "source_type",
        payload.source_type.as_deref().unwrap_or("prometheus"),
        &MONITORING_SOURCE_TYPES,
    )?;
    let query_url = trim_required_case_sensitive("query_url", &payload.query_url)?;

    // Resolve the host context (template rendering + zabbix host id).
    let host = if let Some(target_id) = payload.target_id {
        Some(fetch_host_monitoring_config(state, target_id).await?)
    } else {
        None
    };

    // Credential resolution: ONLY reuse the credential persisted on an
    // existing `monitoring_query_source` whose `query_url` exactly matches
    // the request. Caller-supplied refs are forbidden — see the
    // `MetricQueryTestRequest` doc comment. The candidate list is scoped:
    // host's effective sources first when `target_id` is set, then global
    // sources as a fallback so the endpoint matches `/latest`'s reach.
    let mut candidates: Vec<MonitoringQuerySourceConfig> = match host.as_ref() {
        Some(host) => host.monitoring.query_sources.clone(),
        None => monitoring_persistence::fetch_global_query_sources(&state.db_pool).await?,
    };
    if candidates.is_empty() && host.is_some() {
        candidates = monitoring_persistence::fetch_global_query_sources(&state.db_pool).await?;
    }
    let matching_source = candidates.into_iter().find(|candidate| {
        candidate
            .query_url
            .as_deref()
            .map(str::trim)
            .is_some_and(|url| url == query_url)
    });
    // A matched query source turns a caller-provided metric expression into an
    // authenticated request using its bound named secret. The route's
    // Monitoring/ConnectionTarget Update grant permits the test operation, but
    // it is not authority to disclose the source credential to that upstream.
    // Check the canonical per-secret policy before either secret lookup or the
    // outbound URL resolution so denied callers learn nothing about the bound
    // secret and cannot trigger a request with it.
    ensure_query_source_credential_use(
        state,
        request_context,
        matching_source
            .as_ref()
            .and_then(|source| source.credential_secret_id),
    )
    .await?;
    let source_options = matching_source
        .as_ref()
        .map(|source| source.options.clone())
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    // Same SSRF guard as the persisted path: validate the URL shape, then
    // pin the resolved IPs to block DNS rebinding before the client makes any
    // request. A URL that exactly matches an admin-created source inherits that
    // source's provenance, but only on the authority branch that owns
    // monitoring config; the target-scoped branch keeps the strict guard. See
    // `metric_test_allows_private_resolution`.
    let allow_private =
        metric_test_allows_private_resolution(matching_source.is_some(), payload.target_id);
    let pinned_resolution =
        pin_monitoring_query_url(&query_url, &source_options, allow_private).await?;
    let client = metric_query_http_client(Some(&pinned_resolution)).map_err(AppError::Internal)?;

    let (token, configured_from, credential_ref_display, timeout_override) =
        if let Some(source) = matching_source.as_ref() {
            let token = if let Some(secret_id) = source.credential_secret_id {
                Some(state.services.secret_store.get_secret(secret_id).await?)
            } else {
                None
            };
            (
                token,
                "monitoring_domain".to_string(),
                source
                    .credential_secret_id
                    .map(|secret_id| secret_id.to_string()),
                source.timeout_ms,
            )
        } else {
            (None, "test_request".to_string(), None, None)
        };

    // Bound the upstream timeout. The persisted-source path inherits the
    // configured value (typically a few seconds); the unauthenticated
    // one-shot path falls back to the global default. In either case we
    // clamp to `[100ms, 60s]` so a misconfigured source can't pin a worker
    // for minutes.
    const MAX_TEST_TIMEOUT_MS: u64 = 60_000;
    let timeout_ms = timeout_override
        .unwrap_or(state.settings.monitoring.query_timeout_ms)
        .clamp(100, MAX_TEST_TIMEOUT_MS);
    let resolved_source = ResolvedMetricQuerySource {
        info: LatestMetricSourceInfo {
            name: matching_source.as_ref().and_then(|s| s.name.clone()),
            source_type: Some(source_type.clone()),
            configured_from,
            query_url: redacted_url(&query_url),
            credential_ref: credential_ref_display,
        },
        source_type: source_type.clone(),
        query_url,
        token,
        timeout_ms,
        options: matching_source
            .as_ref()
            .map(|s| s.options.clone())
            .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
        pinned_resolution: Some(pinned_resolution),
    };

    if is_zabbix_source_type(&source_type) {
        let Some(host) = host.as_ref() else {
            return Err(AppError::Validation(
                "target_id is required when source_type is `zabbix`".to_string(),
            ));
        };
        let descriptor = MonitoringMetricDescriptor {
            name: "test".to_string(),
            label: None,
            query: query.clone(),
            unit: None,
            value_type: None,
            widget_type: None,
            description: None,
            source_type: Some(source_type.clone()),
            query_type: Some("zabbix_item_key".to_string()),
            options: Value::Object(serde_json::Map::new()),
            show_in_list: false,
            list_render: "auto".to_string(),
            // Synthetic descriptor for one-shot live test; not backed by a
            // persisted `monitoring_metric_query` row.
            metric_query_id: None,
            category: None,
            semantic: None,
        };
        let result = fetch_latest_zabbix_metric(&client, &resolved_source, &descriptor, host).await;
        return Ok(match result {
            Ok((value, executed_query)) => MetricQueryTestResponse {
                executed_query,
                value: Some(value),
                error: None,
                source_resolved: true,
                suggested_unit: None,
            },
            Err(error) => MetricQueryTestResponse {
                executed_query: render_zabbix_host_selector(&descriptor.query, host),
                value: None,
                error: Some(error),
                source_resolved: true,
                suggested_unit: None,
            },
        });
    }

    let executed_query = host
        .as_ref()
        .map(|host| render_host_metric_query(&query, host))
        .unwrap_or_else(|| query.clone());
    let result = fetch_latest_prometheus_metric(&client, &resolved_source, &executed_query).await;
    Ok(match result {
        Ok(value) => MetricQueryTestResponse {
            executed_query,
            value: Some(value),
            error: None,
            source_resolved: true,
            suggested_unit: None,
        },
        Err(error) => MetricQueryTestResponse {
            executed_query,
            value: None,
            error: Some(error),
            source_resolved: true,
            suggested_unit: None,
        },
    })
}

/// Run an ad-hoc PromQL/Zabbix query against a query source without saving
/// anything.
///
/// Authz: the endpoint actively performs an outbound network call and so is
/// treated as a side-effecting Write affordance — it requires
/// `Monitoring:Update` on the config (mirroring metric-query CRUD), or
/// `ConnectionTarget:Update` on the pinned target when `target_id` is set.
/// An audit event is recorded on every call so an operator can correlate
/// who tested which URL against which source family.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/metric-queries/test",
    tag = "monitoring",
    request_body = MetricQueryTestRequest,
    responses(
        (status = 200, description = "Test result (including upstream errors)", body = MetricQueryTestResponse),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Live query using a bound source credential is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn test_metric_query(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<MetricQueryTestRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    // A live test makes an outbound HTTP call, so it is treated as a Write
    // affordance (mirrors how metric-query CRUD is gated). When a target is
    // pinned the caller must hold Update on that connection target; otherwise
    // they need monitoring-config Update.
    let authz = if let Some(target_id) = payload.target_id {
        // Attribute-aware like every other per-target ConnectionTarget site,
        // so site/role-scoped Update policies (allow and deny) apply to the
        // live-test affordance the same way they do to proxy assignments.
        let target = connection_target_resource_id(target_id);
        match connection_target_authz_attributes_for(&state, target_id, None).await {
            Ok(attributes) => {
                build_policy_evaluation_service(&state)
                    .ensure_access_with_attributes(
                        &request_context,
                        Action::Update,
                        ResourceType::ConnectionTarget,
                        Some(target.as_str()),
                        Some(attributes),
                    )
                    .await
            }
            Err(error) => Err(error),
        }
    } else {
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    };
    if let Err(error) = authz {
        return handle_app_error(error);
    }

    // Capture the audit fingerprint BEFORE moving the payload into the
    // service so we always emit an event — even when the service errors
    // out. `query_url` is redacted to scheme://host (no path/query) so a
    // bearer-bearing URL placed in `query_url` (which the service refuses
    // to use anyway, but operators may still type) does not leak verbatim
    // into audit storage.
    let audit_query_url = redacted_url(payload.query_url.as_str());
    let audit_source_type = payload
        .source_type
        .clone()
        .unwrap_or_else(|| "prometheus".to_string());
    let audit_target_id = payload.target_id;

    let result = metric_query_test_service(&state, &request_context, payload).await;
    record_test_metric_query_audit(
        &state,
        &request_context,
        audit_target_id,
        &audit_source_type,
        &audit_query_url,
        result.is_ok(),
    )
    .await;
    match result {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Record an `monitoring.metric_query.test` audit event. Failures are logged
/// at warning level only — a degraded audit pipeline must not mask the test
/// result the operator is actively waiting on.
async fn record_test_metric_query_audit(
    state: &AppState,
    request_context: &RequestContext,
    target_id: Option<Uuid>,
    source_type: &str,
    redacted_query_url: &str,
    ok: bool,
) {
    let actor_id = request_context.user_id;
    let metadata = json!({
        "actor_kind": "user",
        "auth_method": request_context.auth_method,
        "api_token_id": request_context.api_token_id,
        "request_ip": request_context.request_ip,
        "correlation_id": request_context.correlation_id,
        "source_type": source_type,
        "query_url": redacted_query_url,
        "ok": ok,
    });
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "monitoring.metric_query.test".to_string(),
        actor_id,
        actor_kind: "user".to_string(),
        target_type: target_id.map(|_| "connection_target".to_string()),
        target_id,
        metadata,
        created_at: Utc::now(),
    };
    if let Err(error) = state.services.audit.record_event(event).await {
        log_error_event(
            "monitoring.metric_query.test.audit_failed",
            "metric_query_test_audit_failed",
            error.to_string(),
            EventFields::default()
                .with_operation("metric_query_test_audit")
                .with_status("failed"),
        );
    }
}
