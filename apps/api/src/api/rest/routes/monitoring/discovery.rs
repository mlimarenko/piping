//! Monitoring discovery routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{Path, Query};
use crate::app::AppState;
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::contexts::inventory::infrastructure::persistence::DiscoveryRepository;
use crate::errors::AppError;
use crate::observability::metrics;

use super::helpers::{
    append_port, deduplicate_targets, first_non_empty, is_monitoring_request_authorized,
    is_service_discovery_request_authorized, normalize_metrics_path, normalize_string,
    parse_extra_labels, parse_group_fields, parse_grouping_mode, parse_kind_filter,
    parse_target_source, stable_label_key,
};
use crate::api::connection_target_visibility::{
    ConnectionTargetVisibility, resolve_connection_target_visibility,
};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::contexts::access_control::domain::VisibilityPredicate;
use crate::contexts::config::domain::MonitoringSdLabelGroups;

use crate::contexts::monitoring::application::{
    enrich_hosts_with_monitoring, fetch_discovery_hosts, prometheus_compatible_settings_query_url,
    read_host_states, redacted_url, resolve_metric_query_source,
};
use crate::contexts::monitoring::domain::{
    DiscoveryHost, HostAlert, LatestMetricSourceInfo, MonitoringMetricDescriptor,
    MonitoringQuerySourceConfig, MonitoringScrapeConfig, ResolvedMetricQuerySource,
    descriptor_source_type, is_prometheus_compatible_source_type, is_zabbix_source_type,
    render_host_metric_query, source_type_matches_descriptor,
};
use crate::contexts::monitoring::infrastructure::persistence as monitoring_persistence;
use crate::contexts::monitoring::infrastructure::providers::{
    fetch_latest_prometheus_metric_for_host, fetch_latest_zabbix_metric, metric_query_http_client,
};

#[derive(Debug, Default, Deserialize)]
pub struct MonitoringAuthQuery {
    pub token: Option<String>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ServiceDiscoveryQuery {
    pub token: Option<String>,
    pub kind: Option<String>,
    pub target: Option<String>,
    pub port: Option<u16>,
    pub with_port: Option<bool>,
    pub scheme: Option<String>,
    pub metrics_path: Option<String>,
    pub job: Option<String>,
    pub device_job: Option<String>,
    pub vm_job: Option<String>,
    pub group_by: Option<String>,
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub status: Option<String>,
    pub site_status: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub dedup: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_string_vec")]
    pub label: Vec<String>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct LatestMetricsQuery {
    #[serde(default, deserialize_with = "deserialize_string_vec")]
    pub query: Vec<String>,
    pub target_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct HostLabelsQuery {
    /// Host ids (repeated) whose cached list labels are requested.
    #[serde(default)]
    pub host_id: Vec<Uuid>,
}

pub(crate) fn parse_service_discovery_query(
    raw_query: Option<String>,
) -> Result<ServiceDiscoveryQuery, AppError> {
    let params = parse_repeated_query_params(raw_query);
    Ok(ServiceDiscoveryQuery {
        token: query_scalar(&params, "token"),
        kind: query_scalar(&params, "kind"),
        target: query_scalar(&params, "target"),
        port: parse_optional_query_param(&params, "port")?,
        with_port: parse_optional_query_param(&params, "with_port")?,
        scheme: query_scalar(&params, "scheme"),
        metrics_path: query_scalar(&params, "metrics_path"),
        job: query_scalar(&params, "job"),
        device_job: query_scalar(&params, "device_job"),
        vm_job: query_scalar(&params, "vm_job"),
        group_by: query_scalar(&params, "group_by"),
        site_id: parse_optional_query_param(&params, "site_id")?,
        tenant_id: parse_optional_query_param(&params, "tenant_id")?,
        role_id: parse_optional_query_param(&params, "role_id")?,
        status: query_scalar(&params, "status"),
        site_status: query_scalar(&params, "site_status"),
        search: query_scalar(&params, "search"),
        limit: parse_optional_query_param(&params, "limit")?,
        offset: parse_optional_query_param(&params, "offset")?,
        dedup: parse_optional_query_param(&params, "dedup")?,
        label: query_values(&params, "label"),
    })
}

pub(crate) fn parse_latest_metrics_query(
    raw_query: Option<String>,
) -> Result<LatestMetricsQuery, AppError> {
    let params = parse_repeated_query_params(raw_query);
    Ok(LatestMetricsQuery {
        query: query_values(&params, "query"),
        target_id: parse_optional_query_param(&params, "target_id")?,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct DiscoveredTarget {
    pub(crate) target: String,
    pub(crate) labels: HashMap<String, String>,
    host: DiscoveryHost,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ServiceDiscoveryTargetGroup {
    pub targets: Vec<String>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ServiceDiscoveryHostPreview {
    pub host_id: Uuid,
    pub host_name: String,
    pub host_address: String,
    pub kind: String,
    pub site_name: Option<String>,
    pub tenant_name: Option<String>,
    pub role_name: Option<String>,
    pub status: Option<String>,
    pub scrapes: Vec<MonitoringScrapeConfig>,
    pub metrics: Vec<MonitoringMetricDescriptor>,
    pub inspector_metrics: Vec<MonitoringMetricDescriptor>,
    pub metric_sources: Vec<LatestMetricSourceInfo>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ServiceDiscoveryPreviewResponse {
    pub groups: Vec<ServiceDiscoveryTargetGroup>,
    pub hosts: Vec<ServiceDiscoveryHostPreview>,
    pub groups_count: usize,
    pub targets_count: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConnectionTargetObservabilityResponse {
    pub target: ServiceDiscoveryHostPreview,
    pub groups: Vec<ServiceDiscoveryTargetGroup>,
    pub targets_count: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LatestMetricsResponse {
    pub configured: bool,
    pub items: Vec<LatestMetricItem>,
    pub message: Option<String>,
    pub source: Option<LatestMetricSourceInfo>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LatestMetricItem {
    pub query: String,
    pub executed_query: Option<String>,
    pub name: Option<String>,
    pub label: Option<String>,
    pub unit: Option<String>,
    pub value_type: Option<String>,
    pub widget_type: Option<String>,
    pub source_type: Option<String>,
    pub status: String,
    pub value: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HostLabelsResponse {
    pub hosts: Vec<HostLabelEntry>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HostLabelEntry {
    pub host_id: Uuid,
    pub metrics: Vec<HostLabelMetric>,
    /// Active alerts correlated to this host, served from the cached alert
    /// state. Empty when no alert entry exists for the host; alert presence is
    /// independent of the metrics `stale` flag.
    pub alerts: Vec<HostAlert>,
    pub updated_at: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HostLabelMetric {
    pub query: String,
    // dynamic: raw promql passthrough — the cached Prometheus series value is
    // rendered unchanged by the frontend `metricDisplay`/`MetricWidgetCard`.
    pub value: Value,
    /// Display fields copied from the host-specific descriptor, so the value
    /// renders self-contained. Names and types mirror
    /// `MonitoringMetricDescriptor`.
    pub label: Option<String>,
    pub unit: Option<String>,
    pub value_type: Option<String>,
    pub widget_type: Option<String>,
    pub list_render: String,
    pub options: Value,
    /// Visual grouping copied from the descriptor.
    pub category: Option<String>,
    /// Optional backend semantic. `availability` marks the canonical host up/down metric.
    pub semantic: Option<String>,
}

#[derive(Debug)]
pub(crate) struct DiscoveryRequestOptions {
    pub(crate) kind_label: Option<&'static str>,
    pub(crate) target_source: TargetSource,
    pub(crate) grouping: GroupingMode,
    pub(crate) group_fields: Vec<GroupByField>,
    pub(crate) default_port: Option<u16>,
    pub(crate) default_scheme: String,
    pub(crate) default_metrics_path: String,
    pub(crate) default_job: Option<String>,
    pub(crate) device_job: Option<String>,
    pub(crate) vm_job: Option<String>,
    pub(crate) dedup_targets: bool,
    pub(crate) page: DiscoveryPage,
    pub(crate) filters: DiscoveryHostFilters,
    pub(crate) extra_labels: HashMap<String, String>,
    pub(crate) with_port: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DiscoveryPage {
    pub(crate) limit: u32,
    pub(crate) offset: u32,
}

#[derive(Debug)]
struct DiscoveryRender {
    groups: Vec<ServiceDiscoveryTargetGroup>,
    hosts: Vec<DiscoveryHost>,
    targets_count: usize,
}

#[derive(Debug)]
pub(crate) struct LatestMetricQueries {
    pub(crate) descriptors: Vec<MonitoringMetricDescriptor>,
    pub(crate) message: Option<String>,
}

impl DiscoveryRequestOptions {
    fn from_query(query: &ServiceDiscoveryQuery, state: &AppState) -> Result<Self, AppError> {
        let kind_label = parse_kind_filter(query.kind.as_deref())?;
        let target_source = parse_target_source(query.target.as_deref())?;
        let grouping = parse_grouping_mode(query.group_by.as_deref())?;
        let group_fields = parse_group_fields(query.group_by.as_deref())?;
        let extra_labels = parse_extra_labels(&query.label)?;

        let max_limit = state.settings.monitoring.sd_max_limit.max(1);
        let page = DiscoveryPage {
            limit: query.limit.unwrap_or(max_limit).clamp(1, max_limit),
            offset: query.offset.unwrap_or(0),
        };

        let with_port = query.with_port.unwrap_or(true);
        let default_port = if with_port {
            let selected_port = query
                .port
                .unwrap_or(state.settings.monitoring.sd_default_port);
            if selected_port == 0 {
                return Err(AppError::Validation(
                    "Query parameter `port` must be greater than 0".to_string(),
                ));
            }
            Some(selected_port)
        } else {
            None
        };

        let default_scheme = first_non_empty(
            query.scheme.as_deref(),
            Some(state.settings.monitoring.sd_default_scheme.as_str()),
        )
        .unwrap_or("http")
        .to_ascii_lowercase();

        let default_metrics_path = normalize_metrics_path(
            first_non_empty(
                query.metrics_path.as_deref(),
                Some(state.settings.monitoring.sd_default_metrics_path.as_str()),
            )
            .unwrap_or("/metrics"),
        );

        let default_job = first_non_empty(
            query.job.as_deref(),
            Some(state.settings.monitoring.sd_default_job.as_str()),
        )
        .map(str::to_string);

        let filters = DiscoveryHostFilters {
            host_id: None,
            kind: kind_label.map(str::to_string),
            site_id: query.site_id,
            tenant_id: query.tenant_id,
            role_id: query.role_id,
            status: normalize_string(query.status.as_deref()),
            site_status: normalize_string(query.site_status.as_deref()),
            allowed_statuses: None,
            allowed_site_statuses: None,
            search: normalize_string(query.search.as_deref()),
            fetch_limit: max_limit,
            fetch_offset: page.offset,
        };

        Ok(Self {
            kind_label,
            target_source,
            grouping,
            group_fields,
            default_port,
            default_scheme,
            default_metrics_path,
            default_job,
            device_job: first_non_empty(query.device_job.as_deref(), None).map(str::to_string),
            vm_job: first_non_empty(query.vm_job.as_deref(), None).map(str::to_string),
            dedup_targets: query.dedup.unwrap_or(true),
            page,
            filters,
            extra_labels,
            with_port,
        })
    }

    fn metric_kind_label(&self) -> &str {
        self.kind_label.unwrap_or("all")
    }
}

fn service_discovery_error_response(error: AppError) -> Response {
    metrics::record_service_discovery_error();
    handle_app_error(error)
}

pub async fn metrics_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MonitoringAuthQuery>,
) -> impl IntoResponse {
    if !is_monitoring_request_authorized(
        &headers,
        query.token.as_deref(),
        state.settings.monitoring.metrics_token.as_deref(),
    ) {
        return handle_app_error(AppError::Auth("Invalid monitoring token".to_string()));
    }

    match metrics::render_prometheus_text() {
        Ok(payload) => (
            StatusCode::OK,
            [(
                CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8".to_string(),
            )],
            payload,
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

pub async fn service_discovery_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let query = match parse_service_discovery_query(raw_query) {
        Ok(value) => value,
        Err(error) => return service_discovery_error_response(error),
    };

    if !is_service_discovery_request_authorized(
        &headers,
        query.token.as_deref(),
        state.settings.monitoring.sd_token.as_deref(),
        state.settings.monitoring.sd_public_enabled,
    ) {
        metrics::record_service_discovery_error();
        return handle_app_error(AppError::Auth(
            "Invalid service discovery token".to_string(),
        ));
    }

    let options = match DiscoveryRequestOptions::from_query(&query, &state) {
        Ok(value) => value,
        Err(error) => return service_discovery_error_response(error),
    };

    // No row visibility: this is the machine-identity feed (vmagent HTTP SD
    // authenticated by `sd_token`), a trusted-infrastructure consumer that must
    // see the whole fleet.
    match render_service_discovery(&state, &options, None).await {
        Ok(render) => {
            metrics::record_service_discovery_success(
                options.metric_kind_label(),
                render.targets_count,
            );

            (StatusCode::OK, Json(render.groups)).into_response()
        }
        Err(error) => service_discovery_error_response(error),
    }
}

/// Preview the services discovered for connection targets.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/sd/preview",
    tag = "monitoring",
    params(ServiceDiscoveryQuery),
    responses(
        (status = 200, description = "Rendered Prometheus HTTP service discovery preview", body = ServiceDiscoveryPreviewResponse),
        (status = "4XX", description = "Service discovery preview request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Service discovery preview request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn service_discovery_preview_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let query = match parse_service_discovery_query(raw_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let options = match DiscoveryRequestOptions::from_query(&query, &state) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    // Row-level visibility: the coarse `Read × Monitoring` gate
    // above admits the preview endpoint as a whole; WHICH hosts appear is
    // further scoped by the caller's `Read × CONNECTION_TARGET` policies, like
    // every other human-facing host list. No applicable grant → empty preview.
    let visibility = match resolve_connection_target_visibility(&state, &request_context).await {
        Ok(value) => value.into_filter(),
        Err(error) => return handle_app_error(error),
    };

    match render_service_discovery(&state, &options, visibility.as_ref()).await {
        Ok(render) => {
            let response = ServiceDiscoveryPreviewResponse {
                groups_count: render.groups.len(),
                targets_count: render.targets_count,
                groups: render.groups,
                hosts: render
                    .hosts
                    .iter()
                    .map(|host| target_preview_from_host(&state, host))
                    .collect(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get observability data for a connection target.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/targets/{id}",
    tag = "monitoring",
    params(
        ServiceDiscoveryQuery,
        ("id" = Uuid, Path, description = "Connection target id")
    ),
    responses(
        (status = 200, description = "Connection target observability discovery and metric descriptors", body = ConnectionTargetObservabilityResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Connection target observability request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Connection target observability request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn connection_target_observability_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_uuid): Path<Uuid>,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let query = match parse_service_discovery_query(raw_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_read(&state, &request_context, target_uuid).await {
        return handle_app_error(error);
    }

    let mut options = match DiscoveryRequestOptions::from_query(&query, &state) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    options.filters.host_id = Some(target_uuid);
    options.page = DiscoveryPage {
        limit: 1,
        offset: 0,
    };
    options.filters.fetch_offset = 0;

    // No row visibility: `ensure_monitoring_read` above already enforced the
    // caller's `Read × CONNECTION_TARGET` policies for THIS target id with its
    // resolved attributes — the row filter would re-derive the same decision.
    match render_service_discovery(&state, &options, None).await {
        Ok(render) => {
            let Some(host) = render.hosts.first() else {
                return handle_app_error(AppError::NotFound(
                    "Connection target not found".to_string(),
                ));
            };
            let response = ConnectionTargetObservabilityResponse {
                target: target_preview_from_host(&state, host),
                groups: render.groups,
                targets_count: render.targets_count,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get the latest collected metrics.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/latest",
    tag = "monitoring",
    params(LatestMetricsQuery),
    responses(
        (status = 200, description = "Latest selected metrics from the configured Prometheus-compatible query endpoint", body = LatestMetricsResponse),
        (status = "4XX", description = "Latest metrics request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Latest metrics request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn latest_metrics_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let query = match parse_latest_metrics_query(raw_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    let access_result = if let Some(target_uuid) = query.target_id {
        ensure_monitoring_read(&state, &request_context, target_uuid).await
    } else {
        ensure_monitoring_config_read(&state, &request_context).await
    };
    if let Err(error) = access_result {
        return handle_app_error(error);
    }

    match latest_metrics_response(&state, &query).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Read cached observability list labels for the requested hosts.
///
/// Serves only from the Redis cache populated by the background list-labels
/// poller — never performs external metric queries. Cache misses return a
/// `stale = true` entry rather than an error.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/host-labels",
    tag = "monitoring",
    params(HostLabelsQuery),
    responses(
        (status = 200, description = "Cached per-host observability list labels", body = HostLabelsResponse),
        (status = "4XX", description = "Host labels request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Host labels request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn host_labels_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let host_ids = match parse_host_labels_query(raw_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    // Row-level visibility: `host_id` is caller-supplied and the
    // coarse `Read × Monitoring` gate above does not scope WHICH hosts may be
    // probed — without this filter a constrained caller could read cached
    // metrics/alerts for an arbitrary host UUID. Hidden ids come back as the
    // same `stale = true` placeholder a cache miss produces, so the response
    // shape is unchanged and row existence is not disclosed.
    let visible_ids = match resolve_connection_target_visibility(&state, &request_context).await {
        Ok(ConnectionTargetVisibility::All) => host_ids.clone(),
        Ok(ConnectionTargetVisibility::None) => Vec::new(),
        Ok(ConnectionTargetVisibility::Restricted(predicate)) => {
            match state
                .repositories
                .discovery
                .list_visible_host_ids(&host_ids, &predicate)
                .await
            {
                Ok(ids) => ids,
                Err(error) => return handle_app_error(error),
            }
        }
        Err(error) => return handle_app_error(error),
    };

    match fetch_host_labels(&state, &host_ids, &visible_ids).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

pub(crate) fn parse_host_labels_query(raw_query: Option<String>) -> Result<Vec<Uuid>, AppError> {
    let params = parse_repeated_query_params(raw_query);
    query_values(&params, "host_id")
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            Uuid::parse_str(value.trim()).map_err(|_| {
                AppError::Validation("Query parameter `host_id` has an invalid value".to_string())
            })
        })
        .collect()
}

async fn render_service_discovery(
    state: &AppState,
    options: &DiscoveryRequestOptions,
    visibility: Option<&VisibilityPredicate>,
) -> Result<DiscoveryRender, AppError> {
    let label_settings = state
        .repositories
        .monitoring_sd_label_settings
        .get()
        .await?;
    let mut filters = options.filters.clone();
    if filters.status.is_none() {
        filters.allowed_statuses = Some(label_settings.device_statuses.clone());
    }
    if filters.site_status.is_none() {
        filters.allowed_site_statuses = Some(label_settings.site_statuses.clone());
    }

    let raw_hosts =
        match fetch_discovery_hosts(&state.repositories.discovery, &filters, visibility).await {
            Ok(items) => items,
            Err(error) => return Err(error),
        };

    let hosts_with_monitoring =
        match enrich_hosts_with_monitoring(&state.db_pool, &state.settings.monitoring, raw_hosts)
            .await
        {
            Ok(hosts) => hosts,
            Err(error) => return Err(error),
        };

    let filtered_hosts = apply_post_filters(hosts_with_monitoring);
    let paged_hosts = filtered_hosts
        .into_iter()
        .take(options.page.limit as usize)
        .collect::<Vec<_>>();

    let discovered_targets = build_discovered_targets(paged_hosts.clone(), options);
    let hosts = paged_hosts;

    let groups = build_target_groups(
        discovered_targets,
        options.grouping,
        &options.group_fields,
        options.dedup_targets,
        &label_settings,
    );

    let targets_count: usize = groups.iter().map(|group| group.targets.len()).sum();

    Ok(DiscoveryRender {
        groups,
        hosts,
        targets_count,
    })
}

fn target_preview_from_host(state: &AppState, host: &DiscoveryHost) -> ServiceDiscoveryHostPreview {
    ServiceDiscoveryHostPreview {
        host_id: host.host_id,
        host_name: host.host_name.clone(),
        host_address: host.host_address.clone(),
        kind: host.kind.clone(),
        site_name: host.site_name.clone(),
        tenant_name: host.tenant_name.clone(),
        role_name: host.role_name.clone(),
        status: host.status.clone(),
        scrapes: host.monitoring.scrapes.clone(),
        metrics: host.monitoring.metrics.clone(),
        inspector_metrics: host.monitoring.inspector_metrics.clone(),
        metric_sources: metric_source_infos_for_host(state, host),
    }
}

fn metric_source_infos_for_host(
    state: &AppState,
    host: &DiscoveryHost,
) -> Vec<LatestMetricSourceInfo> {
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for descriptor in &host.monitoring.metrics {
        let source_type = descriptor_source_type(descriptor).to_ascii_lowercase();
        if !seen.insert(source_type.clone()) {
            continue;
        }
        if let Some(info) =
            resolve_metric_query_source_info(state, &host.monitoring.query_sources, &source_type)
        {
            sources.push(info);
        }
    }
    sources
}

fn resolve_metric_query_source_info(
    state: &AppState,
    sources: &[MonitoringQuerySourceConfig],
    requested_source_type: &str,
) -> Option<LatestMetricSourceInfo> {
    let selected_source = sources
        .iter()
        .find(|source| source_type_matches_descriptor(&source.source_type, requested_source_type));

    let query_url = selected_source
        .and_then(|source| source.query_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            prometheus_compatible_settings_query_url(
                &state.settings.monitoring,
                requested_source_type,
            )
        })?;

    let configured_from = if selected_source
        .and_then(|source| source.query_url.as_deref())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        "monitoring_domain".to_string()
    } else {
        "global_settings".to_string()
    };

    let source_type = selected_source
        .map(|source| source.source_type.clone())
        .unwrap_or_else(|| {
            if is_prometheus_compatible_source_type(requested_source_type) {
                "prometheus".to_string()
            } else {
                requested_source_type.to_ascii_lowercase()
            }
        });

    Some(LatestMetricSourceInfo {
        name: selected_source.and_then(|source| source.name.clone()),
        source_type: Some(source_type),
        configured_from,
        query_url: redacted_url(&query_url),
        credential_ref: selected_source
            .and_then(|source| source.credential_secret_id)
            .map(|secret_id| secret_id.to_string()),
    })
}

async fn select_latest_metric_inputs(
    state: &AppState,
    query: &LatestMetricsQuery,
) -> Result<
    (
        LatestMetricQueries,
        Vec<MonitoringQuerySourceConfig>,
        Option<DiscoveryHost>,
    ),
    AppError,
> {
    let max_queries = state.settings.monitoring.query_max_queries.max(1) as usize;
    if let Some(target_id) = query.target_id {
        let host = fetch_host_monitoring_config(state, target_id).await?;
        let selected_queries = select_latest_metric_descriptors(
            &query.query,
            &host.monitoring.metrics,
            max_queries,
            "Connection target has no configured metric descriptors",
            "Connection-target metric queries must match configured connection target metric descriptors",
        )?;
        Ok((
            selected_queries,
            host.monitoring.query_sources.clone(),
            Some(host),
        ))
    } else {
        let descriptors = fetch_global_monitoring_descriptors(state).await?;
        let selected_queries = select_latest_metric_descriptors(
            &query.query,
            &descriptors,
            max_queries,
            "No configured metric descriptors are available",
            "Global metric queries must match configured metric descriptors",
        )?;
        Ok((
            selected_queries,
            monitoring_persistence::fetch_global_query_sources(&state.db_pool).await?,
            None,
        ))
    }
}

async fn execute_latest_metric(
    source: &ResolvedMetricQuerySource,
    descriptor: &MonitoringMetricDescriptor,
    host: Option<&DiscoveryHost>,
) -> Result<(Value, String), String> {
    let client = metric_query_http_client(source.pinned_resolution.as_ref())?;
    if is_zabbix_source_type(&source.source_type) {
        return match host {
            Some(host) => fetch_latest_zabbix_metric(&client, source, descriptor, host).await,
            None => Err("Zabbix latest metrics require a target_id".to_string()),
        };
    }
    let executed_query = host
        .map(|host| render_host_metric_query(&descriptor.query, host))
        .unwrap_or_else(|| descriptor.query.clone());
    fetch_latest_prometheus_metric_for_host(
        &client,
        source,
        &executed_query,
        &descriptor.query,
        host,
    )
    .await
    .map(|value| (value, executed_query))
}

async fn latest_metrics_response(
    state: &AppState,
    query: &LatestMetricsQuery,
) -> Result<LatestMetricsResponse, AppError> {
    let (selected_queries, query_sources, host) = select_latest_metric_inputs(state, query).await?;

    for descriptor in &selected_queries.descriptors {
        if descriptor.query.len() > 512 {
            return Err(AppError::Validation(
                "Metric query length must not exceed 512 characters".to_string(),
            ));
        }
    }

    if selected_queries.descriptors.is_empty() {
        return Ok(LatestMetricsResponse {
            configured: true,
            items: Vec::new(),
            message: selected_queries.message,
            source: None,
        });
    }

    let mut resolved_sources = HashMap::<String, Option<ResolvedMetricQuerySource>>::new();
    let mut first_source_info = None;
    let mut any_source_configured = false;
    let mut items = Vec::with_capacity(selected_queries.descriptors.len());

    for descriptor in selected_queries.descriptors {
        let source_type = descriptor_source_type(&descriptor).to_string();
        let source = if let Some(source) = resolved_sources.get(&source_type) {
            source.clone()
        } else {
            let source = resolve_metric_query_source(
                &state.settings.monitoring,
                state.services.secret_store.as_ref(),
                &query_sources,
                &source_type,
            )
            .await?;
            resolved_sources.insert(source_type.clone(), source.clone());
            source
        };

        let Some(source) = source else {
            items.push(latest_metric_error_item(
                &descriptor,
                None,
                "Monitoring metric source is not configured for this provider",
            ));
            continue;
        };

        any_source_configured = true;
        if first_source_info.is_none() {
            first_source_info = Some(source.info.clone());
        }

        let value = execute_latest_metric(&source, &descriptor, host.as_ref()).await;

        items.push(match value {
            Ok((value, executed_query)) => latest_metric_success_item(
                &descriptor,
                Some(source.source_type.clone()),
                Some(executed_query),
                value,
            ),
            Err(error) => latest_metric_error_item(&descriptor, Some(source.source_type), &error),
        });
    }

    if !any_source_configured {
        return Ok(LatestMetricsResponse {
            configured: false,
            items: Vec::new(),
            message: Some(
                "Monitoring metric source is not configured; create a monitoring query source assignment or set monitoring.query_url".to_string(),
            ),
            source: None,
        });
    }

    Ok(LatestMetricsResponse {
        configured: true,
        items,
        message: selected_queries.message,
        source: first_source_info,
    })
}

/// Read cached list labels for a set of hosts directly from Redis (no external
/// calls). Returns one entry per requested host, in request order; cache misses
/// are marked stale. Only ids in `visible_ids` (the caller's row-visibility set)
/// touch the cache — hidden ids yield the same stale placeholder as
/// a miss, so they are indistinguishable from unknown hosts.
async fn fetch_host_labels(
    state: &AppState,
    host_ids: &[Uuid],
    visible_ids: &[Uuid],
) -> Result<HostLabelsResponse, AppError> {
    let visible: HashSet<Uuid> = visible_ids.iter().copied().collect();
    // Preserve request order (and duplicates) while dropping hidden ids, so the
    // cache results zip back positionally against the visible subsequence.
    let cached_ids: Vec<Uuid> = host_ids
        .iter()
        .copied()
        .filter(|host_id| visible.contains(host_id))
        .collect();

    // One MGET over the labels + alerts keys for the whole batch (the context
    // reader concatenates both key families), so a 50-id batch is a single
    // round trip instead of two sequential MGETs.
    let cached_states = if cached_ids.is_empty() {
        Vec::new()
    } else {
        let mut redis = state.redis.clone();
        read_host_states(&mut redis, &cached_ids).await?
    };

    let mut cached_entries = cached_states.into_iter();
    let hosts = host_ids
        .iter()
        .map(|host_id| {
            let cached = if visible.contains(host_id) {
                cached_entries.next()
            } else {
                None
            };
            let Some(cached_state) = cached else {
                return stale_host_label_entry(*host_id);
            };
            // Alerts are an additive, independent cache: a miss yields an empty
            // list and never marks the metrics entry stale.
            let alerts = cached_state.alerts;

            match cached_state.labels {
                Some(payload) => HostLabelEntry {
                    host_id: *host_id,
                    metrics: payload
                        .metrics
                        .into_iter()
                        .map(|metric| HostLabelMetric {
                            query: metric.query,
                            value: metric.value,
                            label: metric.label,
                            unit: metric.unit,
                            value_type: metric.value_type,
                            widget_type: metric.widget_type,
                            list_render: metric.list_render,
                            options: metric.options,
                            category: metric.category,
                            semantic: metric.semantic,
                        })
                        .collect(),
                    alerts,
                    updated_at: Some(payload.updated_at),
                    stale: false,
                },
                None => HostLabelEntry {
                    host_id: *host_id,
                    metrics: Vec::new(),
                    alerts,
                    updated_at: None,
                    stale: true,
                },
            }
        })
        .collect();
    Ok(HostLabelsResponse { hosts })
}

/// The placeholder entry a hidden or uncached host id resolves to — identical
/// in shape to a cache miss, so callers cannot distinguish "hidden by policy"
/// from "not cached yet".
fn stale_host_label_entry(host_id: Uuid) -> HostLabelEntry {
    HostLabelEntry {
        host_id,
        metrics: Vec::new(),
        alerts: Vec::new(),
        updated_at: None,
        stale: true,
    }
}

async fn fetch_global_monitoring_descriptors(
    state: &AppState,
) -> Result<Vec<MonitoringMetricDescriptor>, AppError> {
    monitoring_persistence::fetch_enabled_metric_descriptors(&state.db_pool).await
}

fn latest_metric_success_item(
    descriptor: &MonitoringMetricDescriptor,
    source_type: Option<String>,
    executed_query: Option<String>,
    value: Value,
) -> LatestMetricItem {
    LatestMetricItem {
        query: descriptor.query.clone(),
        executed_query,
        name: Some(descriptor.name.clone()),
        label: descriptor.label.clone(),
        unit: descriptor.unit.clone(),
        value_type: descriptor.value_type.clone(),
        widget_type: descriptor.widget_type.clone(),
        source_type,
        status: "success".to_string(),
        value: Some(value),
        error: None,
    }
}

fn latest_metric_error_item(
    descriptor: &MonitoringMetricDescriptor,
    source_type: Option<String>,
    error: &str,
) -> LatestMetricItem {
    LatestMetricItem {
        query: descriptor.query.clone(),
        executed_query: None,
        name: Some(descriptor.name.clone()),
        label: descriptor.label.clone(),
        unit: descriptor.unit.clone(),
        value_type: descriptor.value_type.clone(),
        widget_type: descriptor.widget_type.clone(),
        source_type,
        status: "error".to_string(),
        value: None,
        error: Some(error.to_string()),
    }
}

fn apply_post_filters(hosts: Vec<DiscoveryHost>) -> Vec<DiscoveryHost> {
    hosts
        .into_iter()
        .filter(|host| !host.monitoring.scrapes.is_empty() || !host.monitoring.metrics.is_empty())
        .collect::<Vec<_>>()
}

pub(crate) fn build_discovered_targets(
    hosts: Vec<DiscoveryHost>,
    options: &DiscoveryRequestOptions,
) -> Vec<DiscoveredTarget> {
    let mut targets = Vec::new();

    for host in hosts {
        for scrape in &host.monitoring.scrapes {
            targets.extend(build_discovered_targets_for_scrape(&host, scrape, options));
        }
    }

    targets
}

fn build_discovered_targets_for_scrape(
    host: &DiscoveryHost,
    scrape: &MonitoringScrapeConfig,
    options: &DiscoveryRequestOptions,
) -> Vec<DiscoveredTarget> {
    if scrape.enabled == Some(false) {
        return Vec::new();
    }

    let (labels, effective_port) = scrape_labels(host, scrape, options);

    let base_targets = base_targets_for_scrape(host, scrape, options.target_source);

    base_targets
        .into_iter()
        .filter_map(|base| discovered_target_from_base(host, &labels, effective_port, base))
        .collect()
}

fn scrape_labels(
    host: &DiscoveryHost,
    scrape: &MonitoringScrapeConfig,
    options: &DiscoveryRequestOptions,
) -> (HashMap<String, String>, Option<u16>) {
    let scheme = scrape
        .scheme
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| options.default_scheme.clone());
    let metrics_path = scrape
        .metrics_path
        .as_deref()
        .map(normalize_metrics_path)
        .unwrap_or_else(|| options.default_metrics_path.clone());
    let port = options
        .with_port
        .then(|| scrape.port.or(options.default_port))
        .flatten();
    let job = scrape
        .job
        .clone()
        .or_else(|| default_job_for_host(host, options));

    let mut labels = HashMap::new();
    labels.insert("__scheme__".to_string(), scheme);
    labels.insert("__metrics_path__".to_string(), metrics_path);
    if let Some(job) = job {
        labels.insert("job".to_string(), job.clone());
        labels.insert("piping_job".to_string(), job);
    }
    labels.extend(options.extra_labels.clone());
    if let Some(name) = scrape
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        labels.insert("piping_scrape".to_string(), name.to_string());
    }
    labels.extend(scrape.labels.clone());
    (labels, port)
}

fn discovered_target_from_base(
    host: &DiscoveryHost,
    base_labels: &HashMap<String, String>,
    port: Option<u16>,
    (base_target, interface_name, channel_address): (String, Option<String>, Option<String>),
) -> Option<DiscoveredTarget> {
    if base_target.trim().is_empty() {
        return None;
    }
    let target = port
        .map(|port| append_port(&base_target, port))
        .unwrap_or_else(|| base_target.clone());
    let mut labels = base_labels.clone();
    labels.insert("piping_target_address".to_string(), base_target);
    if let Some(interface_name) = interface_name {
        labels.insert("piping_interface".to_string(), interface_name);
    }
    if let Some(channel_address) = channel_address.filter(|value| !value.trim().is_empty()) {
        labels.insert("piping_channel_address".to_string(), channel_address);
    }
    Some(DiscoveredTarget {
        target,
        labels,
        host: host.clone(),
    })
}

fn base_targets_for_scrape(
    host: &DiscoveryHost,
    scrape: &MonitoringScrapeConfig,
    default_source: TargetSource,
) -> Vec<(String, Option<String>, Option<String>)> {
    if let Some(target) = scrape
        .target
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty())
    {
        return vec![(target.to_string(), None, None)];
    }

    match target_source_for_scrape(scrape, default_source) {
        TargetSource::Address => vec![(host.host_address.clone(), None, None)],
        TargetSource::Name => vec![(host.host_name.clone(), None, None)],
        TargetSource::Id => vec![(host.host_id.to_string(), None, None)],
        TargetSource::InterfaceAddresses => host
            .interface_addresses
            .iter()
            .filter(|item| !item.address.trim().is_empty() && !item.name.trim().is_empty())
            .map(|item| (item.address.clone(), Some(item.name.clone()), None))
            .collect(),
        TargetSource::TunnelAddresses => host
            .tunnel_addresses
            .iter()
            .filter(|item| !item.address.trim().is_empty() && !item.name.trim().is_empty())
            .map(|item| {
                (
                    item.address.clone(),
                    Some(item.name.clone()),
                    item.channel_address.clone(),
                )
            })
            .collect(),
    }
}

pub(crate) fn target_source_for_scrape(
    scrape: &MonitoringScrapeConfig,
    default_source: TargetSource,
) -> TargetSource {
    match scrape.target_source.as_deref() {
        Some("host_name") | Some("name") => TargetSource::Name,
        Some("host_id") | Some("id") => TargetSource::Id,
        Some("host_address") | Some("address") => TargetSource::Address,
        Some("interface_addresses") | Some("interfaces") => TargetSource::InterfaceAddresses,
        Some("tunnel_addresses") | Some("tunnels") => TargetSource::TunnelAddresses,
        _ => default_source,
    }
}

fn default_job_for_host(host: &DiscoveryHost, options: &DiscoveryRequestOptions) -> Option<String> {
    let mut effective_job = options.default_job.clone();

    if host.kind == "device" {
        if let Some(job) = options.device_job.as_deref() {
            effective_job = Some(job.to_string());
        }
    } else if host.kind == "virtual_machine"
        && let Some(job) = options.vm_job.as_deref()
    {
        effective_job = Some(job.to_string());
    }

    effective_job
}

pub(crate) fn build_target_groups(
    targets: Vec<DiscoveredTarget>,
    grouping: GroupingMode,
    group_fields: &[GroupByField],
    dedup_targets: bool,
    label_groups: &MonitoringSdLabelGroups,
) -> Vec<ServiceDiscoveryTargetGroup> {
    let mut groups_map: HashMap<String, ServiceDiscoveryTargetGroup> = HashMap::new();

    for discovered in targets {
        let mut labels = discovered.labels.clone();

        match grouping {
            GroupingMode::PerTarget => {
                merge_host_labels(&mut labels, &discovered.host, label_groups);
            }
            GroupingMode::SingleGroup => {
                labels.remove("piping_target_address");
                labels.remove("piping_interface");
                labels.remove("piping_channel_address");
            }
            GroupingMode::ByFields => {
                for (key, value) in group_labels_from_host(&discovered.host, group_fields) {
                    labels.insert(key, value);
                }
            }
        }

        let key = stable_label_key(&labels);
        groups_map
            .entry(key)
            .and_modify(|group| group.targets.push(discovered.target.clone()))
            .or_insert_with(|| ServiceDiscoveryTargetGroup {
                targets: vec![discovered.target.clone()],
                labels,
            });
    }

    let mut groups = groups_map.into_values().collect::<Vec<_>>();
    if dedup_targets {
        for group in &mut groups {
            deduplicate_targets(&mut group.targets);
        }
    }

    groups
}

/// Inserts a label only when the value is present and non-empty.
fn insert_present(labels: &mut HashMap<String, String>, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        let value = value.trim();
        if !value.is_empty() {
            labels.insert(key.to_string(), value.to_string());
        }
    }
}

fn merge_host_labels(
    labels: &mut HashMap<String, String>,
    host: &DiscoveryHost,
    groups: &MonitoringSdLabelGroups,
) {
    // Host basics are always emitted so metric and alert expressions can
    // correlate samples back to a stable Piping target.
    labels.insert("piping_target_id".to_string(), host.host_id.to_string());
    labels.insert("piping_host_name".to_string(), host.host_name.clone());
    labels.insert("piping_target_kind".to_string(), host.kind.clone());
    labels.insert("piping_host_address".to_string(), host.host_address.clone());
    insert_present(labels, "piping_contacts", &host.contacts);

    if groups.location {
        insert_present(labels, "piping_site", &host.site_name);
        insert_present(labels, "piping_site_facility", &host.site_facility);
        insert_present(labels, "piping_site_address", &host.site_address);
        insert_present(labels, "piping_site_region", &host.site_region);
        insert_present(labels, "piping_site_group", &host.site_group);
    }

    if groups.tenant {
        insert_present(labels, "piping_tenant", &host.tenant_name);
        insert_present(labels, "piping_tenant_group", &host.tenant_group);
    }

    if groups.role {
        insert_present(labels, "piping_role", &host.role_name);
    }

    if groups.identifiers {
        labels.insert("piping_host_id".to_string(), host.host_id.to_string());
        if let Some(site_id) = host.site_id {
            labels.insert("piping_site_id".to_string(), site_id.to_string());
        }
        insert_present(labels, "piping_site_slug", &host.site_slug);
        if let Some(tenant_id) = host.tenant_id {
            labels.insert("piping_tenant_id".to_string(), tenant_id.to_string());
        }
        insert_present(labels, "piping_tenant_slug", &host.tenant_slug);
        if let Some(role_id) = host.role_id {
            labels.insert("piping_role_id".to_string(), role_id.to_string());
        }
        insert_present(labels, "piping_role_slug", &host.role_slug);
    }

    if groups.status {
        insert_present(labels, "piping_status", &host.status);
    }

    if groups.hardware {
        insert_present(labels, "piping_platform", &host.platform_name);
        insert_present(labels, "piping_model", &host.model);
        insert_present(labels, "piping_manufacturer", &host.manufacturer);
        insert_present(labels, "piping_serial", &host.serial);
        insert_present(labels, "piping_asset_tag", &host.asset_tag);
        insert_present(labels, "piping_rack", &host.rack);
        insert_present(labels, "piping_cluster", &host.cluster);
    }

    if groups.network {
        insert_present(labels, "piping_primary_ip4", &host.primary_ip4);
        insert_present(labels, "piping_primary_ip6", &host.primary_ip6);
        insert_present(labels, "piping_interfaces", &host.interfaces);
    }

    if groups.description {
        insert_present(labels, "piping_description", &host.description);
        insert_present(labels, "piping_comments", &host.comments);
    }
}

fn group_labels_from_host(
    host: &DiscoveryHost,
    group_fields: &[GroupByField],
) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    for field in group_fields {
        match field {
            GroupByField::Kind => {
                labels.insert("piping_target_kind".to_string(), host.kind.clone());
            }
            GroupByField::Site => {
                insert_name_or_id(
                    &mut labels,
                    "piping_site",
                    host.site_name.as_deref(),
                    "piping_site_id",
                    host.site_id,
                );
            }
            GroupByField::Tenant => {
                insert_name_or_id(
                    &mut labels,
                    "piping_tenant",
                    host.tenant_name.as_deref(),
                    "piping_tenant_id",
                    host.tenant_id,
                );
            }
            GroupByField::Role => {
                insert_name_or_id(
                    &mut labels,
                    "piping_role",
                    host.role_name.as_deref(),
                    "piping_role_id",
                    host.role_id,
                );
            }
            GroupByField::Status => {
                if let Some(status) = &host.status {
                    labels.insert("piping_status".to_string(), status.clone());
                }
            }
        }
    }
    labels
}

fn insert_name_or_id(
    labels: &mut HashMap<String, String>,
    name_key: &str,
    name: Option<&str>,
    id_key: &str,
    id: Option<Uuid>,
) {
    if let Some(name) = name {
        labels.insert(name_key.to_string(), name.to_string());
    } else if let Some(id) = id {
        labels.insert(id_key.to_string(), id.to_string());
    }
}
