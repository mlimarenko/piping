//! Profile-assignment coverage.
//!
//! Read-only: it reuses the service-discovery read model and render path, so
//! the fleet view reports exactly the hosts, scrapes and labels that
//! `/api/v1/monitoring/sd/preview` renders for the same filters. The extra
//! value is provenance (`assignment_id` / `assignment_name` per target),
//! duplicate detection, and a stable reason for every host without a scrape.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::connection_target_visibility::resolve_connection_target_visibility;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::VisibilityPredicate;
use crate::contexts::monitoring::application::coverage::{
    CoverageAssignmentInfo, CoverageAssignmentSummary, CoverageDuplicate, CoverageHostEvidence,
    CoverageHostResult, CoverageReason, CoverageResult, CoverageTarget, CoverageTotals,
    build_coverage,
};
use crate::contexts::monitoring::application::enrich_hosts_with_monitoring;
use crate::contexts::monitoring::application::read_model::{
    assignment_matches_host, collect_scope_ids, effective_profile_assignments_for_host,
    fetch_discovery_hosts,
};
use crate::contexts::monitoring::domain::{DiscoveryHost, MonitoringScrapeConfig};
use crate::contexts::monitoring::infrastructure::persistence::{
    self as monitoring_persistence, HostScopeIds, ProfileAssignment,
};
use crate::errors::AppError;

use super::discovery::{
    DiscoveryRequestOptions, apply_sd_device_status_filter, apply_sd_site_status_filter,
    build_discovered_targets_for_scrape,
};
use super::*;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CoverageTargetResponse {
    pub assignment_id: Uuid,
    /// Operator-visible profile name the assignment selected, nullable when the
    /// profile has no name.
    pub assignment_name: Option<String>,
    pub exporter_slug: String,
    pub assignment_scope_type: String,
    pub assignment_scope_id: Option<Uuid>,
    pub job: Option<String>,
    pub target: String,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CoverageHostResponse {
    pub host_id: Uuid,
    pub host_name: String,
    /// Inventory kind of the host: `device` or `virtual_machine`.
    pub kind: String,
    pub status: Option<String>,
    /// `covered`, `no_scrape`, `duplicate` or `filtered`.
    pub state: String,
    /// Stable snake_case explanation: `no_assignment`, `disabled_assignment`,
    /// `status_filtered`, `site_status_filtered` or `no_target`. Null for
    /// covered and duplicate hosts.
    pub reason: Option<String>,
    pub scrapes: usize,
    pub targets: Vec<CoverageTargetResponse>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CoverageDuplicateOccurrenceResponse {
    pub assignment_id: Uuid,
    pub exporter_slug: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CoverageDuplicateResponse {
    pub job: Option<String>,
    pub target: String,
    pub assignments: Vec<CoverageDuplicateOccurrenceResponse>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CoverageAssignmentSummaryResponse {
    pub assignment_id: Uuid,
    /// Operator-visible profile name (`monitoring_profile.name`), nullable when
    /// the profile has no name.
    pub name: Option<String>,
    pub profile_slug: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub matched_hosts: usize,
    pub targets: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CoverageTotalsResponse {
    pub hosts_total: usize,
    pub covered: usize,
    pub no_scrape: usize,
    pub duplicate: usize,
    pub filtered: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MonitoringCoverageResponse {
    pub hosts: Vec<CoverageHostResponse>,
    pub totals: CoverageTotalsResponse,
    pub assignments: Vec<CoverageAssignmentSummaryResponse>,
    pub duplicates: Vec<CoverageDuplicateResponse>,
    /// Number of hosts matching the request before host pagination.
    pub total: usize,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CoverageHostQuery {
    /// Restrict the fleet view to one connection target.
    pub host_id: Option<Uuid>,
}

struct CoverageComputation {
    result: CoverageResult,
}

/// Fleet coverage for profile assignments.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/coverage",
    tag = "monitoring",
    params(ServiceDiscoveryQuery, CoverageHostQuery),
    responses(
        (status = 200, description = "Per-host scrape coverage, assignment summaries and duplicate job/target pairs", body = MonitoringCoverageResponse),
        (status = "4XX", description = "Coverage request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Coverage request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn monitoring_coverage_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: axum::http::HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query = match parse_service_discovery_query(raw_query.clone()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let params = parse_repeated_query_params(raw_query);
    let host_id = match parse_optional_query_param::<Uuid>(&params, "host_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }
    let visibility = match resolve_connection_target_visibility(&state, &request_context).await {
        Ok(value) => value.into_filter(),
        Err(error) => return handle_app_error(error),
    };
    let options = match DiscoveryRequestOptions::from_query(&query, &state) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match compute_coverage(&state, &options, host_id, visibility.as_ref()).await {
        Ok(computation) => {
            let (hosts, total) = page_hosts(&computation.result.hosts, &options);
            (
                StatusCode::OK,
                Json(MonitoringCoverageResponse {
                    hosts,
                    totals: totals_response(computation.result.totals),
                    assignments: computation
                        .result
                        .assignments
                        .iter()
                        .map(assignment_summary_response)
                        .collect(),
                    duplicates: computation
                        .result
                        .duplicates
                        .iter()
                        .map(duplicate_response)
                        .collect(),
                    total,
                    limit: options.page.limit,
                    offset: options.page.offset,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

async fn compute_coverage(
    state: &AppState,
    options: &DiscoveryRequestOptions,
    host_id: Option<Uuid>,
    visibility: Option<&VisibilityPredicate>,
) -> Result<CoverageComputation, AppError> {
    let label_settings = state
        .repositories
        .monitoring_sd_label_settings
        .get()
        .await?;

    let mut base_filters = options.filters.clone();
    base_filters.fetch_limit = u32::MAX;
    base_filters.fetch_offset = 0;
    if host_id.is_some() {
        base_filters.host_id = host_id;
    }

    // The scoped fleet including status-filtered hosts: status allow-lists are
    // deliberately not applied here, because the view must explain why a host
    // that the preview hides is not scraped.
    let candidate_hosts =
        fetch_discovery_hosts(&state.repositories.discovery, &base_filters, visibility).await?;

    let mut device_filters = base_filters.clone();
    apply_sd_device_status_filter(&mut device_filters, &label_settings);
    let mut site_filters = base_filters.clone();
    apply_sd_site_status_filter(&mut site_filters, &label_settings);

    let (device_ids, site_ids) = if candidate_hosts.is_empty() {
        (HashMap::new(), HashMap::new())
    } else {
        // Two bounded membership queries separate "excluded by the device
        // status allow-list" from "excluded by the site status allow-list".
        let device_hosts =
            fetch_discovery_hosts(&state.repositories.discovery, &device_filters, visibility)
                .await?;
        let site_hosts =
            fetch_discovery_hosts(&state.repositories.discovery, &site_filters, visibility).await?;
        (
            device_hosts
                .into_iter()
                .map(|host| (host.host_id, ()))
                .collect::<HashMap<_, _>>(),
            site_hosts
                .into_iter()
                .map(|host| (host.host_id, ()))
                .collect::<HashMap<_, _>>(),
        )
    };

    let mut filtered_reasons = HashMap::new();
    let mut active_hosts = Vec::new();
    for host in &candidate_hosts {
        if !device_ids.contains_key(&host.host_id) {
            filtered_reasons.insert(host.host_id, CoverageReason::StatusFiltered);
        } else if !site_ids.contains_key(&host.host_id) {
            filtered_reasons.insert(host.host_id, CoverageReason::SiteStatusFiltered);
        } else {
            active_hosts.push(host.clone());
        }
    }

    let active_hosts =
        enrich_hosts_with_monitoring(&state.db_pool, &state.settings.monitoring, active_hosts)
            .await?;

    let candidate_assignments = if candidate_hosts.is_empty() {
        Vec::new()
    } else {
        let (host_ids, site_ids, tenant_ids, role_ids) = collect_scope_ids(&candidate_hosts);
        monitoring_persistence::fetch_profile_assignment_candidates_for_hosts(
            &state.db_pool,
            HostScopeIds {
                host_ids: &host_ids,
                site_ids: &site_ids,
                tenant_ids: &tenant_ids,
                role_ids: &role_ids,
            },
        )
        .await?
    };

    let assignments = candidate_assignments
        .iter()
        .map(|assignment| CoverageAssignmentInfo {
            assignment_id: assignment.id,
            name: Some(assignment.profile_name.clone()),
            profile_slug: assignment.profile_slug.clone(),
            scope_type: assignment.scope_type.clone(),
            scope_id: assignment.scope_id,
        })
        .collect::<Vec<_>>();

    let active_by_id = active_hosts
        .into_iter()
        .map(|host| (host.host_id, host))
        .collect::<HashMap<_, _>>();

    let mut evidence = Vec::with_capacity(candidate_hosts.len());
    for host in &candidate_hosts {
        let mut matching_assignment_ids = Vec::new();
        for assignment in &candidate_assignments {
            if assignment_matches_host(host, &assignment.scope_type, assignment.scope_id)
                && !matching_assignment_ids.contains(&assignment.id)
            {
                matching_assignment_ids.push(assignment.id);
            }
        }
        let filtered_reason = filtered_reasons.get(&host.host_id).copied();
        let Some(active_host) = active_by_id.get(&host.host_id) else {
            evidence.push(CoverageHostEvidence {
                host_id: host.host_id,
                host_name: host.host_name.clone(),
                kind: host.kind.clone(),
                status: host.status.clone(),
                filtered_reason,
                matching_assignment_ids,
                enabled_assignment_ids: Vec::new(),
                targets: Vec::new(),
            });
            continue;
        };

        let effective_assignments =
            effective_profile_assignments_for_host(active_host, &candidate_assignments);
        if effective_assignments.len() != active_host.monitoring.scrapes.len() {
            return Err(AppError::Internal(
                "Coverage assignment resolution diverged from the service-discovery read model"
                    .to_string(),
            ));
        }
        // Render every effective profile item through the same conversion the
        // service-discovery read model applies, so coverage cannot drift.
        let mut targets = Vec::new();
        let mut enabled_assignment_ids = Vec::new();
        for assignment in &effective_assignments {
            if !enabled_assignment_ids.contains(&assignment.id) {
                enabled_assignment_ids.push(assignment.id);
            }
            targets.extend(coverage_targets_for_scrape(
                active_host,
                assignment,
                &assignment.to_scrape(),
                options,
            ));
        }

        evidence.push(CoverageHostEvidence {
            host_id: active_host.host_id,
            host_name: active_host.host_name.clone(),
            kind: active_host.kind.clone(),
            status: active_host.status.clone(),
            filtered_reason: None,
            matching_assignment_ids,
            enabled_assignment_ids,
            targets,
        });
    }

    let result = build_coverage(&evidence, &assignments);
    Ok(CoverageComputation { result })
}

fn coverage_targets_for_scrape(
    host: &DiscoveryHost,
    assignment: &ProfileAssignment,
    scrape: &MonitoringScrapeConfig,
    options: &DiscoveryRequestOptions,
) -> Vec<CoverageTarget> {
    build_discovered_targets_for_scrape(host, scrape, options)
        .into_iter()
        .map(|discovered| CoverageTarget {
            assignment_id: assignment.id,
            assignment_name: Some(assignment.profile_name.clone()),
            exporter_slug: assignment.exporter_slug.clone(),
            assignment_scope_type: assignment.scope_type.clone(),
            assignment_scope_id: assignment.scope_id,
            job: discovered.labels.get("job").cloned(),
            target: discovered.target,
            labels: discovered.labels,
        })
        .collect()
}

fn page_hosts(
    hosts: &[CoverageHostResult],
    options: &DiscoveryRequestOptions,
) -> (Vec<CoverageHostResponse>, usize) {
    let total = hosts.len();
    let offset = options.page.offset as usize;
    let limit = options.page.limit as usize;
    let page = hosts
        .iter()
        .skip(offset.min(total))
        .take(limit)
        .map(host_response)
        .collect();
    (page, total)
}

fn host_response(host: &CoverageHostResult) -> CoverageHostResponse {
    CoverageHostResponse {
        host_id: host.host_id,
        host_name: host.host_name.clone(),
        kind: host.kind.clone(),
        status: host.status.clone(),
        state: host.state.as_str().to_string(),
        reason: host.reason.map(|reason| reason.as_str().to_string()),
        scrapes: host.scrapes,
        targets: host.targets.iter().map(target_response).collect(),
    }
}

fn target_response(target: &CoverageTarget) -> CoverageTargetResponse {
    CoverageTargetResponse {
        assignment_id: target.assignment_id,
        assignment_name: target.assignment_name.clone(),
        exporter_slug: target.exporter_slug.clone(),
        assignment_scope_type: target.assignment_scope_type.clone(),
        assignment_scope_id: target.assignment_scope_id,
        job: target.job.clone(),
        target: target.target.clone(),
        labels: target.labels.clone(),
    }
}

fn duplicate_response(duplicate: &CoverageDuplicate) -> CoverageDuplicateResponse {
    CoverageDuplicateResponse {
        job: duplicate.job.clone(),
        target: duplicate.target.clone(),
        assignments: duplicate
            .assignments
            .iter()
            .map(|occurrence| CoverageDuplicateOccurrenceResponse {
                assignment_id: occurrence.assignment_id,
                exporter_slug: occurrence.exporter_slug.clone(),
            })
            .collect(),
    }
}

fn assignment_summary_response(
    summary: &CoverageAssignmentSummary,
) -> CoverageAssignmentSummaryResponse {
    CoverageAssignmentSummaryResponse {
        assignment_id: summary.assignment_id,
        name: summary.name.clone(),
        profile_slug: summary.profile_slug.clone(),
        scope_type: summary.scope_type.clone(),
        scope_id: summary.scope_id,
        matched_hosts: summary.matched_hosts,
        targets: summary.targets,
        duplicates: summary.duplicates,
    }
}

fn totals_response(totals: CoverageTotals) -> CoverageTotalsResponse {
    CoverageTotalsResponse {
        hosts_total: totals.hosts_total,
        covered: totals.covered,
        no_scrape: totals.no_scrape,
        duplicate: totals.duplicate,
        filtered: totals.filtered,
    }
}
