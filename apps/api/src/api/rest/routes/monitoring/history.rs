//! Monitoring history routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    Extension, Json,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::app::AppState;
use crate::errors::AppError;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;

use crate::contexts::monitoring::application::{redacted_url, resolve_metric_query_source};
use crate::contexts::monitoring::domain::{
    DiscoveryHost, LatestMetricSourceInfo, MonitoringMetricDescriptor, ResolvedMetricQuerySource,
    build_host_correlation_index, correlation_label_for_source, correlation_match_for_source,
    descriptor_source_type, is_list_capable_query, is_zabbix_source_type, render_host_metric_query,
    render_zabbix_host_selector,
};
use crate::contexts::monitoring::infrastructure::providers::{
    metric_query_http_client, resolve_zabbix_item, zabbix_rpc,
};

// ─────────────────────────────────────────────────────────────────────────────
// Deliverable B: metric history (Prometheus range query proxy)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum allowed time range for a history request (31 days in seconds).
pub(crate) const HISTORY_MAX_RANGE_SECS: f64 = 31.0 * 24.0 * 3600.0;

/// Minimum allowed step size in seconds.
pub(crate) const HISTORY_MIN_STEP_SECS: u64 = 1;

/// Maximum number of data points per series before auto-clamping the step.
pub(crate) const HISTORY_MAX_POINTS: u64 = 11_000;

/// Maximum number of series returned in a single response.
const HISTORY_MAX_SERIES: usize = 100;

/// Extra Zabbix samples fetched before downsampling to preserve narrow peaks.
const ZABBIX_HISTORY_OVERSAMPLE_FACTOR: usize = 4;

/// IntoParams stub for the metric history endpoint.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct MetricHistoryQuery {
    /// Connection target UUID to query history for.
    pub target_id: Uuid,
    /// Metric query string (from `monitoring/metrics`).
    pub query: String,
    /// Start of the time range as a Unix timestamp (seconds, float).
    pub start: f64,
    /// End of the time range as a Unix timestamp (seconds, float).
    pub end: f64,
    /// Step size in seconds. Auto-clamped to fit within the point budget.
    pub step: Option<u64>,
}

/// A single time-series returned in a history response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricHistorySeries {
    /// Prometheus label set attached to this series.
    pub labels: HashMap<String, String>,
    // dynamic: raw Prometheus range-matrix point pairs [[timestamp, "value"], ...]
    // rendered unchanged by the frontend chart component.
    pub points: Vec<Value>,
}

/// Response for `GET /api/v1/monitoring/history`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricHistoryResponse {
    pub configured: bool,
    pub query: String,
    pub executed_query: Option<String>,
    pub unit: Option<String>,
    pub value_type: Option<String>,
    pub source: Option<LatestMetricSourceInfo>,
    pub series: Vec<MetricHistorySeries>,
    pub message: Option<String>,
    pub error: Option<String>,
}

/// A configured provider that was reached and answered with an error.
///
/// Both provider paths return this shape, and they were each building it by
/// hand. They diverged on one field: the Zabbix path carried `clamp_message`,
/// the Prometheus path passed `None`. So when the requested step had been
/// auto-raised to stay inside the point budget, the user was told about it on
/// one backend and not the other — the chart came back coarser than asked for,
/// with no explanation, depending only on which provider the host happened to
/// use. Building the response in one place is what stops the two paths from
/// drifting apart again.
pub(crate) struct ProviderErrorHistory<'a> {
    pub(crate) query: &'a str,
    pub(crate) executed_query: String,
    pub(crate) unit: Option<String>,
    pub(crate) value_type: Option<String>,
    pub(crate) source: LatestMetricSourceInfo,
    pub(crate) clamp_message: Option<String>,
    pub(crate) error: String,
}

pub(crate) fn provider_error_history(request: ProviderErrorHistory<'_>) -> MetricHistoryResponse {
    MetricHistoryResponse {
        configured: true,
        query: request.query.to_string(),
        executed_query: Some(request.executed_query),
        unit: request.unit,
        value_type: request.value_type,
        source: Some(request.source),
        series: Vec::new(),
        message: request.clamp_message,
        error: Some(request.error),
    }
}

pub(crate) fn parse_metric_history_query(
    raw_query: Option<String>,
) -> Result<MetricHistoryQuery, AppError> {
    let params = parse_repeated_query_params(raw_query);
    let target_id = parse_optional_query_param::<Uuid>(&params, "target_id")?.ok_or_else(|| {
        AppError::Validation("Query parameter `target_id` is required".to_string())
    })?;
    let query = query_scalar(&params, "query")
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| AppError::Validation("Query parameter `query` is required".to_string()))?;
    let start = parse_optional_query_param::<f64>(&params, "start")?
        .ok_or_else(|| AppError::Validation("Query parameter `start` is required".to_string()))?;
    let end = parse_optional_query_param::<f64>(&params, "end")?
        .ok_or_else(|| AppError::Validation("Query parameter `end` is required".to_string()))?;
    let step = parse_optional_query_param::<u64>(&params, "step")?;
    Ok(MetricHistoryQuery {
        target_id,
        query,
        start,
        end,
        step,
    })
}

/// Derive a Prometheus range-query URL from an instant-query URL.
///
/// - If the URL ends with `/query` → replace the suffix with `/query_range`.
/// - If the URL already ends with `/query_range` → return as-is.
/// - Otherwise strip a trailing `/` and append `/api/v1/query_range`.
pub(crate) fn prometheus_range_url(query_url: &str) -> String {
    let suffix_start = query_url.find(['?', '#']).unwrap_or(query_url.len());
    let (base, suffix) = query_url.split_at(suffix_start);
    let trimmed_base = base.trim_end_matches('/');
    let range_base = if trimmed_base.ends_with("/query") {
        format!("{trimmed_base}_range")
    } else if trimmed_base.ends_with("/query_range") {
        trimmed_base.to_string()
    } else {
        format!("{trimmed_base}/api/v1/query_range")
    };
    format!("{range_base}{suffix}")
}

pub(crate) fn prometheus_range_request_url(
    query_url: &str,
    executed_query: &str,
    start: f64,
    end: f64,
    step: u64,
) -> Result<reqwest::Url, String> {
    let range_url = prometheus_range_url(query_url);
    let mut url = reqwest::Url::parse(&range_url).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("query", executed_query)
        .append_pair("start", &start.to_string())
        .append_pair("end", &end.to_string())
        .append_pair("step", &step.to_string());
    Ok(url)
}

pub(crate) fn decode_prometheus_range_response(
    status: StatusCode,
    body: &[u8],
) -> Result<Value, String> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| error.to_string())?;
    if !status.is_success() {
        let error_message = value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(error_message);
    }
    Ok(value)
}

/// Parse a Prometheus range-query matrix response into typed series.
pub(crate) fn parse_prometheus_range_matrix(prom_json: &Value) -> Vec<MetricHistorySeries> {
    let result = prom_json
        .pointer("/data/result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    result
        .iter()
        .map(|item| {
            let labels: HashMap<String, String> = item
                .get("metric")
                .and_then(Value::as_object)
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            let points: Vec<Value> = item
                .get("values")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            MetricHistorySeries { labels, points }
        })
        .collect()
}

/// Scope fleet-capable range queries to one host while preserving every
/// matching sub-series (for example, per-interface or per-mountpoint data).
///
/// `descriptor_query` is the configured descriptor template, not the rendered
/// query: list capability must be decided before `$host*` substitution, or a
/// per-host-templated range query would be wrongly treated as fleet-capable.
pub(crate) fn parse_prometheus_range_matrix_for_host(
    mut prom_json: Value,
    source: &ResolvedMetricQuerySource,
    host: &DiscoveryHost,
    descriptor_query: &str,
) -> Vec<MetricHistorySeries> {
    if is_list_capable_query(descriptor_query) {
        let correlation_label = correlation_label_for_source(&source.options);
        let match_strategy = correlation_match_for_source(&source.options);
        let correlation_key =
            build_host_correlation_index(std::slice::from_ref(host), &match_strategy)
                .into_keys()
                .next();

        if let Some(result) = prom_json
            .pointer_mut("/data/result")
            .and_then(Value::as_array_mut)
        {
            result.retain(|series| {
                correlation_key.as_deref().is_some_and(|key| {
                    series
                        .get("metric")
                        .and_then(|metric| metric.get(&correlation_label))
                        .and_then(Value::as_str)
                        .is_some_and(|label_value| label_value == key)
                })
            });
        }
    }

    parse_prometheus_range_matrix(&prom_json)
}

/// Execute a Prometheus range query and return the raw JSON response.
async fn fetch_prometheus_range(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    executed_query: &str,
    start: f64,
    end: f64,
    step: u64,
) -> Result<Value, String> {
    let url = prometheus_range_request_url(&source.query_url, executed_query, start, end, step)?;

    let mut request = client.get(url);
    if let Some(token) = source
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        request = request.bearer_auth(token);
    }

    let response = request
        .timeout(Duration::from_millis(source.timeout_ms))
        .send()
        .await
        .map_err(|error| error.to_string())?;

    let status = response.status();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    decode_prometheus_range_response(status, &body)
}

pub(crate) fn zabbix_history_params(
    item: &Value,
    start: f64,
    end: f64,
    limit: usize,
) -> Result<Value, String> {
    let item_id = item
        .get("itemid")
        .and_then(Value::as_str)
        .ok_or_else(|| "Zabbix item did not include an itemid".to_string())?;
    let value_type = item
        .get("value_type")
        .and_then(Value::as_str)
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let zabbix_limit = zabbix_history_fetch_limit(limit).saturating_add(1);

    Ok(serde_json::json!({
        "output": "extend",
        "history": value_type,
        "itemids": [item_id],
        "time_from": start as i64,
        "time_till": end as i64,
        "sortfield": "clock",
        "sortorder": "ASC",
        "limit": zabbix_limit,
    }))
}

fn zabbix_history_fetch_limit(max_points: usize) -> usize {
    max_points.saturating_mul(ZABBIX_HISTORY_OVERSAMPLE_FACTOR)
}

/// Execute a Zabbix `history.get` range query and return the raw history array.
///
/// Resolves the descriptor's item via [`resolve_zabbix_item`], reads its
/// `value_type` (Zabbix history table selector, string `"0".."4"`, default `0`
/// = numeric float) and `itemid`, then fetches the clock-ordered history rows
/// within `[start, end]`.
struct ZabbixRangeRequest<'a> {
    client: &'a reqwest::Client,
    source: &'a ResolvedMetricQuerySource,
    descriptor: &'a MonitoringMetricDescriptor,
    host: &'a DiscoveryHost,
    start: f64,
    end: f64,
    limit: usize,
}

async fn fetch_zabbix_range(request: ZabbixRangeRequest<'_>) -> Result<Value, String> {
    let (item, _executed_query, _host_id) = resolve_zabbix_item(
        request.client,
        request.source,
        request.descriptor,
        request.host,
    )
    .await?;
    let params = zabbix_history_params(&item, request.start, request.end, request.limit)?;

    zabbix_rpc(request.client, request.source, "history.get", params).await
}

/// Parse a Zabbix `history.get` array into a single time-series.
///
/// Maps `[{clock, value}, ...]` into one [`MetricHistorySeries`] whose points
/// are `[[clock_as_f64, value_as_string], ...]` — the same `[ts, "value"]`
/// pair shape a Prometheus range query yields, so the frontend renders both
/// identically. The series is labelled with the host correlation tags
/// (`piping_host_id`, `piping_host_name`). When the history exceeds
/// `max_points`, it is downsampled evenly while always keeping the first and
/// last points. A non-array response or a response exceeding the fetch budget
/// is rejected so an incomplete window cannot be reported as complete.
pub(crate) fn parse_zabbix_history(
    history_json: &Value,
    host: &DiscoveryHost,
    max_points: usize,
) -> Result<Vec<MetricHistorySeries>, String> {
    let rows = history_json
        .as_array()
        .ok_or_else(|| "Zabbix history response was not an array".to_string())?;
    let fetch_limit = zabbix_history_fetch_limit(max_points);
    if rows.len() > fetch_limit {
        return Err(format!(
            "Zabbix history exceeds the {fetch_limit}-sample fetch budget; the requested time window could not be returned completely"
        ));
    }

    let points: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            // Zabbix `clock` is a stringified Unix timestamp; `value` is the raw
            // string sample. Both are emitted as strings by the API.
            let clock = row
                .get("clock")
                .and_then(Value::as_str)
                .and_then(|raw| raw.trim().parse::<f64>().ok())?;
            let value = row
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_string)?;
            Some(Value::Array(vec![
                serde_json::json!(clock),
                Value::String(value),
            ]))
        })
        .collect();

    if points.is_empty() {
        return Ok(Vec::new());
    }

    let points = downsample_points(points, max_points);

    let mut labels = HashMap::new();
    labels.insert("piping_host_id".to_string(), host.host_id.to_string());
    labels.insert("piping_host_name".to_string(), host.host_name.clone());

    Ok(vec![MetricHistorySeries { labels, points }])
}

/// Evenly downsample `points` to at most `max_points`, always keeping the
/// first and last samples. Returns the input unchanged when it already fits.
pub(crate) fn downsample_points(points: Vec<Value>, max_points: usize) -> Vec<Value> {
    if max_points == 0 || points.len() <= max_points {
        return points;
    }
    if max_points == 1 {
        return points.into_iter().take(1).collect();
    }

    let last_index = points.len() - 1;
    let target_inner = max_points - 1;
    let mut sampled = Vec::with_capacity(max_points);
    for slot in 0..max_points {
        // Map slot 0..=target_inner across 0..=last_index so the final slot
        // lands exactly on the last point.
        let index = slot * last_index / target_inner;
        sampled.push(points[index].clone());
    }
    sampled
}

#[derive(Debug, PartialEq)]
pub(crate) struct MetricHistoryWindow {
    pub(crate) step: u64,
    pub(crate) clamp_message: Option<String>,
}

pub(crate) fn metric_history_window(
    start: f64,
    end: f64,
    requested_step: Option<u64>,
) -> Result<MetricHistoryWindow, AppError> {
    if start >= end {
        return Err(AppError::Validation(
            "`start` must be less than `end`".to_string(),
        ));
    }
    let range_secs = end - start;
    if range_secs > HISTORY_MAX_RANGE_SECS {
        return Err(AppError::Validation(format!(
            "Requested time range exceeds the maximum of 31 days ({range_secs:.0}s > {HISTORY_MAX_RANGE_SECS}s)"
        )));
    }
    let raw_step = requested_step.unwrap_or(HISTORY_MIN_STEP_SECS);
    if raw_step < HISTORY_MIN_STEP_SECS {
        return Err(AppError::Validation(format!(
            "`step` must be at least {HISTORY_MIN_STEP_SECS}s"
        )));
    }
    let min_step_for_budget = (range_secs / HISTORY_MAX_POINTS as f64).ceil() as u64;
    let step = raw_step.max(min_step_for_budget);
    let clamp_message = (step > raw_step).then(|| {
        format!(
            "Step was auto-raised from {raw_step}s to {step}s to stay within the {HISTORY_MAX_POINTS}-point budget"
        )
    });
    Ok(MetricHistoryWindow {
        step,
        clamp_message,
    })
}

async fn metric_history_service(
    state: &AppState,
    target_id: Uuid,
    metric_query: &str,
    start: f64,
    end: f64,
    requested_step: Option<u64>,
) -> Result<MetricHistoryResponse, AppError> {
    let window = metric_history_window(start, end, requested_step)?;
    let step = window.step;
    let clamp_message = window.clamp_message;

    // Resolve host
    let host = match fetch_host_monitoring_config(state, target_id).await {
        Ok(h) => h,
        Err(AppError::NotFound(_)) => {
            return Ok(MetricHistoryResponse {
                configured: false,
                query: metric_query.to_string(),
                executed_query: None,
                unit: None,
                value_type: None,
                source: None,
                series: Vec::new(),
                message: Some("Connection target not found".to_string()),
                error: None,
            });
        }
        Err(error) => return Err(error),
    };

    // Validate query against configured descriptors
    let all_descriptors: Vec<MonitoringMetricDescriptor> = host
        .monitoring
        .metrics
        .iter()
        .chain(host.monitoring.inspector_metrics.iter())
        .cloned()
        .collect();

    let selected = match select_latest_metric_descriptors(
        &[metric_query.to_string()],
        &all_descriptors,
        1,
        "No metrics configured for this host",
        "Metric query is not configured for this host",
    ) {
        Ok(q) => q,
        Err(error) => return Err(error),
    };

    let descriptor = selected.descriptors.first().ok_or_else(|| {
        AppError::Validation("Metric query is not configured for this host".to_string())
    })?;

    let source_type = descriptor_source_type(descriptor);

    // Zabbix sources: fetch history via the Zabbix JSON-RPC `history.get` API.
    if is_zabbix_source_type(source_type) {
        let resolved_source = match resolve_metric_query_source(
            &state.settings.monitoring,
            state.services.secret_store.as_ref(),
            &host.monitoring.query_sources,
            source_type,
        )
        .await?
        {
            Some(s) => s,
            None => {
                return Ok(MetricHistoryResponse {
                    configured: false,
                    query: metric_query.to_string(),
                    executed_query: None,
                    unit: descriptor.unit.clone(),
                    value_type: descriptor.value_type.clone(),
                    source: None,
                    series: Vec::new(),
                    message: Some("No Zabbix query source configured".to_string()),
                    error: None,
                });
            }
        };

        let executed_query = render_zabbix_host_selector(&descriptor.query, &host);
        let source_info = LatestMetricSourceInfo {
            name: resolved_source.info.name.clone(),
            source_type: Some(resolved_source.source_type.clone()),
            configured_from: resolved_source.info.configured_from.clone(),
            query_url: redacted_url(&resolved_source.query_url),
            credential_ref: resolved_source.info.credential_ref.clone(),
        };

        let client = metric_query_http_client(resolved_source.pinned_resolution.as_ref()).map_err(
            |error| {
                AppError::Internal(format!("Failed to build metric query HTTP client: {error}"))
            },
        )?;

        let history_json = match fetch_zabbix_range(ZabbixRangeRequest {
            client: &client,
            source: &resolved_source,
            descriptor,
            host: &host,
            start,
            end,
            limit: HISTORY_MAX_POINTS as usize,
        })
        .await
        {
            Ok(value) => value,
            Err(error_msg) => {
                return Ok(provider_error_history(ProviderErrorHistory {
                    query: metric_query,
                    executed_query,
                    unit: descriptor.unit.clone(),
                    value_type: descriptor.value_type.clone(),
                    source: source_info,
                    clamp_message,
                    error: error_msg,
                }));
            }
        };

        let (series, error) =
            match parse_zabbix_history(&history_json, &host, HISTORY_MAX_POINTS as usize) {
                Ok(series) => (series, None),
                Err(error) => (Vec::new(), Some(error)),
            };

        return Ok(MetricHistoryResponse {
            configured: true,
            query: metric_query.to_string(),
            executed_query: Some(executed_query),
            unit: descriptor.unit.clone(),
            value_type: descriptor.value_type.clone(),
            source: Some(source_info),
            series,
            message: clamp_message,
            error,
        });
    }

    // Resolve Prometheus-compatible source
    let resolved_source = match resolve_metric_query_source(
        &state.settings.monitoring,
        state.services.secret_store.as_ref(),
        &host.monitoring.query_sources,
        source_type,
    )
    .await?
    {
        Some(s) => s,
        None => {
            return Ok(MetricHistoryResponse {
                configured: false,
                query: metric_query.to_string(),
                executed_query: None,
                unit: descriptor.unit.clone(),
                value_type: descriptor.value_type.clone(),
                source: None,
                series: Vec::new(),
                message: Some(
                    "No Prometheus-compatible query source configured for this host".to_string(),
                ),
                error: None,
            });
        }
    };

    // Render host-specific query (inject labels)
    let executed_query = render_host_metric_query(&descriptor.query, &host);

    // Build HTTP client reusing SSRF-pinned resolution from the resolved source
    let client =
        metric_query_http_client(resolved_source.pinned_resolution.as_ref()).map_err(|error| {
            AppError::Internal(format!("Failed to build metric query HTTP client: {error}"))
        })?;

    // Execute range query
    let prom_json =
        match fetch_prometheus_range(&client, &resolved_source, &executed_query, start, end, step)
            .await
        {
            Ok(value) => value,
            Err(error_msg) => {
                return Ok(provider_error_history(ProviderErrorHistory {
                    query: metric_query,
                    executed_query,
                    unit: descriptor.unit.clone(),
                    value_type: descriptor.value_type.clone(),
                    source: LatestMetricSourceInfo {
                        name: resolved_source.info.name.clone(),
                        source_type: Some(resolved_source.source_type.clone()),
                        configured_from: resolved_source.info.configured_from.clone(),
                        query_url: redacted_url(&resolved_source.query_url),
                        credential_ref: resolved_source.info.credential_ref.clone(),
                    },
                    clamp_message,
                    error: error_msg,
                }));
            }
        };

    // Parse matrix
    let mut series = parse_prometheus_range_matrix_for_host(
        prom_json,
        &resolved_source,
        &host,
        &descriptor.query,
    );

    // Cap series count
    if series.len() > HISTORY_MAX_SERIES {
        series.truncate(HISTORY_MAX_SERIES);
    }

    Ok(MetricHistoryResponse {
        configured: true,
        query: metric_query.to_string(),
        executed_query: Some(executed_query),
        unit: descriptor.unit.clone(),
        value_type: descriptor.value_type.clone(),
        source: Some(LatestMetricSourceInfo {
            name: resolved_source.info.name.clone(),
            source_type: Some(resolved_source.source_type.clone()),
            configured_from: resolved_source.info.configured_from.clone(),
            query_url: redacted_url(&resolved_source.query_url),
            credential_ref: resolved_source.info.credential_ref.clone(),
        }),
        series,
        message: clamp_message,
        error: None,
    })
}

/// Fetch time-series history for a metric from the configured Prometheus-compatible TSDB.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/history",
    tag = "monitoring",
    params(MetricHistoryQuery),
    responses(
        (status = 200, description = "Time-series history for the requested metric", body = MetricHistoryResponse),
        (status = "4XX", description = "Metric history request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Metric history request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn metric_history_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let query = match parse_metric_history_query(raw_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_read(&state, &request_context, query.target_id).await {
        return handle_app_error(error);
    }

    match metric_history_service(
        &state,
        query.target_id,
        &query.query,
        query.start,
        query.end,
        query.step,
    )
    .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}
