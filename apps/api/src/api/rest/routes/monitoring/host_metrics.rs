//! Monitoring host metrics routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use axum::{
    Extension, Json,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::app::AppState;
use crate::errors::AppError;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;

use crate::contexts::monitoring::domain::MonitoringMetricDescriptor;

// ─────────────────────────────────────────────────────────────────────────────
// Deliverable A: host metrics discovery
// ─────────────────────────────────────────────────────────────────────────────

/// IntoParams stub for the host metrics discovery endpoint.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct HostMetricsQuery {
    /// Connection target UUID whose configured metrics are returned.
    pub target_id: Uuid,
}

/// A single metric descriptor returned by the host metrics discovery endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HostMetricDescriptor {
    pub query: String,
    pub label: Option<String>,
    pub unit: Option<String>,
    pub value_type: Option<String>,
    pub widget_type: Option<String>,
    pub source_type: Option<String>,
    /// Operator-facing grouping bucket of the metric query this descriptor came
    /// from. Exposed so tooling callers see the same grouping as the inspector
    /// path (`MonitoringMetricDescriptor.category`); `null` when the query has
    /// no category.
    pub category: Option<String>,
}

/// Response for `GET /api/v1/monitoring/metrics`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HostMetricsResponse {
    pub configured: bool,
    pub metrics: Vec<HostMetricDescriptor>,
    pub message: Option<String>,
}

fn parse_host_metrics_query(raw_query: Option<String>) -> Result<HostMetricsQuery, AppError> {
    let params = parse_repeated_query_params(raw_query);
    let target_id = parse_optional_query_param::<Uuid>(&params, "target_id")?.ok_or_else(|| {
        AppError::Validation("Query parameter `target_id` is required".to_string())
    })?;
    Ok(HostMetricsQuery { target_id })
}

async fn host_metrics_service(
    state: &AppState,
    target_id: Uuid,
) -> Result<HostMetricsResponse, AppError> {
    let host = match fetch_host_monitoring_config(state, target_id).await {
        Ok(h) => h,
        Err(AppError::NotFound(_)) => {
            return Ok(HostMetricsResponse {
                configured: false,
                metrics: Vec::new(),
                message: Some("Connection target not found".to_string()),
            });
        }
        Err(error) => return Err(error),
    };

    let all_descriptors: Vec<MonitoringMetricDescriptor> = host
        .monitoring
        .metrics
        .iter()
        .chain(host.monitoring.inspector_metrics.iter())
        .cloned()
        .collect();

    if all_descriptors.is_empty() {
        return Ok(HostMetricsResponse {
            configured: false,
            metrics: Vec::new(),
            message: Some("No metrics configured for this host".to_string()),
        });
    }

    let items: Vec<HostMetricDescriptor> = all_descriptors
        .iter()
        .map(|d| HostMetricDescriptor {
            query: d.query.clone(),
            label: d.label.clone(),
            unit: d.unit.clone(),
            value_type: d.value_type.clone(),
            widget_type: d.widget_type.clone(),
            source_type: d.source_type.clone(),
            category: d.category.clone(),
        })
        .collect();

    Ok(HostMetricsResponse {
        configured: true,
        metrics: items,
        message: None,
    })
}

/// List metrics configured for a monitored connection target.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/metrics",
    tag = "monitoring",
    params(HostMetricsQuery),
    responses(
        (status = 200, description = "List of metric descriptors configured for the host", body = HostMetricsResponse),
        (status = "4XX", description = "Host metrics request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Host metrics request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn host_metrics_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let query = match parse_host_metrics_query(raw_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_read(&state, &request_context, query.target_id).await {
        return handle_app_error(error);
    }

    match host_metrics_service(&state, query.target_id).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}
