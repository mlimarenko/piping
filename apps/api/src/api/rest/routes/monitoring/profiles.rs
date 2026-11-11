//! Monitoring profiles and profile assignments.
//!
//! A profile is the only scrape owner: its items select exporters with job,
//! scheme, port, path and labels; a profile assignment attaches the profile to
//! a scope. `GET /targets/{id}/profiles` and `GET /profiles/{id}/preview`
//! resolve the resulting scrape targets through the same discovery expansion
//! used by `GET /monitoring/sd`, so preview and runtime cannot drift.

use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::discovery::{
    DiscoveredTarget, DiscoveryPage, DiscoveryRequestOptions, build_discovered_targets,
    build_target_groups,
};
use super::helpers::{normalize_metrics_path, normalize_string};
use super::*;

use crate::api::connection_target_visibility::resolve_connection_target_visibility;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::Action;
use crate::contexts::config::domain::MonitoringSdLabelGroups;
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::contexts::monitoring::application::fetch_discovery_hosts;
use crate::contexts::monitoring::application::read_model::{
    collect_scope_ids, effective_profile_assignments_for_host,
};
use crate::contexts::monitoring::domain::{
    DiscoveryHost, HostMonitoringConfig, MonitoringScrapeConfig,
};
use crate::contexts::monitoring::infrastructure::persistence::{
    self as monitoring_persistence, HostScopeIds, ProfileAssignment, labels_from_value,
};
use crate::errors::AppError;

const MONITORING_PROFILE_DEFAULT_LIMIT: i64 = 50;
const MONITORING_PROFILE_MAX_LIMIT: i64 = 200;

// ===========================================================================
// DTOs
// ===========================================================================

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfileItemEntry {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub exporter_id: Uuid,
    pub exporter_slug: String,
    pub exporter_name: String,
    pub job: Option<String>,
    /// `item.job` when set, otherwise the exporter `default_job`.
    pub effective_job: String,
    pub scheme: Option<String>,
    pub port: Option<i32>,
    pub metrics_path: Option<String>,
    pub labels: Value,
    pub enabled: bool,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfileDetail {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub metric_set_id: Option<Uuid>,
    pub query_source_id: Option<Uuid>,
    pub alert_source_id: Option<Uuid>,
    pub enabled: bool,
    pub item_count: i64,
    pub assignment_count: i64,
    pub items: Vec<ProfileItemEntry>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfileListResponse {
    pub items: Vec<ProfileDetail>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProfileListQuery {
    pub sort: Option<String>,
    pub order: Option<String>,
    pub enabled: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProfileItemRequest {
    pub exporter_id: Uuid,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub port: Option<i32>,
    #[serde(default)]
    pub metrics_path: Option<String>,
    #[serde(default)]
    pub labels: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub sort_order: Option<i32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProfileRequest {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metric_set_id: Option<Uuid>,
    #[serde(default)]
    pub query_source_id: Option<Uuid>,
    #[serde(default)]
    pub alert_source_id: Option<Uuid>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub items: Vec<ProfileItemRequest>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfileAssignmentEntry {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub profile_slug: String,
    pub profile_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub target: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfileAssignmentListResponse {
    pub items: Vec<ProfileAssignmentEntry>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProfileAssignmentListQuery {
    pub profile_id: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub enabled: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProfileAssignmentRequest {
    pub profile_id: Uuid,
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<Uuid>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ProfileApplyRequest {
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<Uuid>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProfilePreviewQuery {
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    /// Unsaved assignment target. An empty value clears the saved override.
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfileConflict {
    /// Profile that already owns the same job/target pair.
    pub profile_id: Uuid,
    pub profile_slug: String,
    pub profile_name: String,
    pub job: String,
    pub target: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfilePreviewHost {
    pub host_id: Uuid,
    pub host_name: String,
    pub host_address: String,
    pub kind: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProfilePreviewResponse {
    pub groups: Vec<ServiceDiscoveryTargetGroup>,
    pub hosts: Vec<ProfilePreviewHost>,
    pub targets_count: usize,
    pub conflicts: Vec<ProfileConflict>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TargetProfileItem {
    pub item_id: Uuid,
    pub exporter_id: Uuid,
    pub exporter_slug: String,
    pub exporter_name: String,
    pub job: String,
    pub scheme: String,
    pub port: Option<i32>,
    pub metrics_path: String,
    pub labels: HashMap<String, String>,
    pub enabled: bool,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TargetProfile {
    pub profile_id: Uuid,
    pub profile_slug: String,
    pub profile_name: String,
    pub assignment_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub target: Option<String>,
    pub items: Vec<TargetProfileItem>,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TargetProfilesResponse {
    pub target_id: Uuid,
    pub profiles: Vec<TargetProfile>,
    pub conflicts: Vec<ProfileConflict>,
}

// ===========================================================================
// Validation
// ===========================================================================

fn profile_list_filter(
    query: &ProfileListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::PROFILE_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[("enabled", query.enabled.as_deref())],
    )
}

fn assignment_list_filter(
    query: &ProfileAssignmentListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::PROFILE_ASSIGNMENT_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("profile_id", query.profile_id.as_deref()),
            ("scope_type", query.scope_type.as_deref()),
            ("scope_id", query.scope_id.as_deref()),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

fn page_bounds(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    let limit = limit
        .unwrap_or(MONITORING_PROFILE_DEFAULT_LIMIT)
        .clamp(1, MONITORING_PROFILE_MAX_LIMIT);
    let offset = offset.unwrap_or(0).max(0);
    (limit, offset)
}

fn trim_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_profile_slug(value: &str) -> Result<String, AppError> {
    let slug = normalize_string(Some(value))
        .ok_or_else(|| AppError::Validation("slug must not be empty".to_string()))?;
    if slug.len() > 128 {
        return Err(AppError::Validation(
            "slug must be at most 128 characters".to_string(),
        ));
    }
    Ok(slug)
}

fn normalize_assignment_target(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn validate_profile_item_scheme(value: Option<&str>) -> Result<Option<String>, AppError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(scheme) => Ok(Some(validate_enum_value(
            "scheme",
            scheme,
            &["http", "https"],
        )?)),
        None => Ok(None),
    }
}

async fn validate_profile_items(
    state: &AppState,
    items: Vec<ProfileItemRequest>,
) -> Result<Vec<repository::ProfileItemInput>, AppError> {
    let exporter_ids = items
        .iter()
        .map(|item| item.exporter_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let exporters = repository::fetch_exporter_references(&state.db_pool, &exporter_ids).await?;
    let exporters = exporters
        .into_iter()
        .map(|exporter| (exporter.id, exporter))
        .collect::<HashMap<_, _>>();

    let mut identities = HashSet::<(Uuid, String)>::new();
    let mut jobs = HashMap::<String, Vec<String>>::new();
    let mut inputs = Vec::with_capacity(items.len());

    for item in items {
        let exporter = exporters.get(&item.exporter_id).ok_or_else(|| {
            AppError::Validation(format!("exporter `{}` does not exist", item.exporter_id))
        })?;
        if !exporter.enabled {
            return Err(AppError::Validation(format!(
                "exporter `{}` is disabled",
                exporter.slug
            )));
        }

        let job = normalize_string(item.job.as_deref());
        let identity = (item.exporter_id, job.clone().unwrap_or_default());
        if !identities.insert(identity) {
            return Err(AppError::Validation(format!(
                "exporter `{}` appears twice with job `{}`; item identity must be unique",
                exporter.slug,
                job.as_deref().unwrap_or("*")
            )));
        }
        let effective_job = job.clone().unwrap_or_else(|| exporter.default_job.clone());
        jobs.entry(effective_job)
            .or_default()
            .push(exporter.slug.clone());

        if let Some(port) = item.port
            && !(1..=65535).contains(&port)
        {
            return Err(AppError::Validation(
                "port must be between 1 and 65535".to_string(),
            ));
        }

        inputs.push(repository::ProfileItemInput {
            exporter_id: item.exporter_id,
            job,
            scheme: validate_profile_item_scheme(item.scheme.as_deref())?,
            port: item.port,
            metrics_path: item
                .metrics_path
                .as_deref()
                .and_then(|path| trim_optional(Some(path)))
                .map(|path| normalize_metrics_path(&path)),
            labels: validate_json_object("labels", item.labels)?,
            enabled: item.enabled.unwrap_or(true),
            sort_order: item.sort_order.unwrap_or(0),
        });
    }

    for (job, exporter_slugs) in jobs {
        if exporter_slugs.len() > 1 {
            return Err(AppError::Validation(format!(
                "job `{job}` is used by multiple exporters ({}); set a distinct job per item",
                exporter_slugs.join(", ")
            )));
        }
    }

    Ok(inputs)
}

/// Re-check stored items before an assignment turns them into live scrapes: a
/// profile can keep an item whose exporter was disabled afterwards.
fn validate_stored_profile_items(
    items: &[repository::ProfileItemRecord],
) -> Result<Vec<String>, AppError> {
    let mut jobs = HashMap::<String, Vec<String>>::new();
    for item in items {
        if !item.enabled {
            continue;
        }
        if !item.exporter_enabled {
            return Err(AppError::Validation(format!(
                "exporter `{}` is disabled",
                item.exporter_slug
            )));
        }
        let job = item.job.clone().unwrap_or_else(|| item.default_job.clone());
        jobs.entry(job)
            .or_default()
            .push(item.exporter_slug.clone());
    }
    for (job, exporter_slugs) in &jobs {
        if exporter_slugs.len() > 1 {
            return Err(AppError::Validation(format!(
                "job `{job}` is used by multiple exporters ({}); set a distinct job per item",
                exporter_slugs.join(", ")
            )));
        }
    }
    Ok(jobs.into_keys().collect())
}

async fn validate_profile_payload(
    state: &AppState,
    payload: ProfileRequest,
) -> Result<repository::ProfileInput, AppError> {
    let name = trim_optional(Some(&payload.name))
        .ok_or_else(|| AppError::Validation("name must not be empty".to_string()))?;
    let slug = normalize_profile_slug(&payload.slug)?;
    let items = validate_profile_items(state, payload.items).await?;

    Ok(repository::ProfileInput {
        name,
        slug,
        description: trim_optional(payload.description.as_deref()),
        metric_set_id: payload.metric_set_id,
        query_source_id: payload.query_source_id,
        alert_source_id: payload.alert_source_id,
        enabled: payload.enabled.unwrap_or(true),
        items,
    })
}

fn conflict_message(job: &str, target: Option<&str>, other_profile_slug: &str) -> String {
    let target = target.unwrap_or("*");
    format!(
        "job `{job}` and target `{target}` are already assigned by profile `{other_profile_slug}`; use a different job or target"
    )
}

/// Report the other profiles that already occupy `effective_jobs` at the same
/// scope with the same target (or without an explicit target).
fn detect_conflicts(
    excluded_assignment_id: Option<Uuid>,
    effective_jobs: &[String],
    target: Option<&str>,
    others: &[repository::ScopeProfileJob],
) -> Vec<ProfileConflict> {
    let mut seen = HashSet::new();
    let mut conflicts = Vec::new();
    for job in effective_jobs {
        for other in others {
            if Some(other.assignment_id) == excluded_assignment_id || &other.job != job {
                continue;
            }
            let target_matches =
                other.target.as_deref() == target || other.target.is_none() || target.is_none();
            if !target_matches {
                continue;
            }
            if !seen.insert((other.assignment_id, other.job.clone(), other.target.clone())) {
                continue;
            }
            conflicts.push(ProfileConflict {
                profile_id: other.profile_id,
                profile_slug: other.profile_slug.clone(),
                profile_name: other.profile_name.clone(),
                job: other.job.clone(),
                target: other.target.clone(),
                message: conflict_message(&other.job, other.target.as_deref(), &other.profile_slug),
            });
        }
    }
    conflicts
}

async fn validate_assignment(
    state: &AppState,
    payload: ProfileAssignmentRequest,
) -> Result<
    (
        repository::ProfileAssignmentInput,
        repository::ProfileRecord,
    ),
    AppError,
> {
    let scope_type = validate_monitoring_scope(&payload.scope_type, payload.scope_id)?;
    let profile = repository::fetch_profile_by_id(&state.db_pool, payload.profile_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Monitoring profile not found".to_string()))?;
    if !profile.enabled {
        return Err(AppError::Validation(format!(
            "monitoring profile `{}` is disabled",
            profile.slug
        )));
    }

    let target = normalize_assignment_target(payload.target.as_deref());
    let items = repository::list_profile_items(&state.db_pool, &[profile.id]).await?;
    validate_stored_profile_items(&items)?;

    let input = repository::ProfileAssignmentInput {
        profile_id: payload.profile_id,
        scope_type: scope_type.clone(),
        scope_id: payload.scope_id,
        weight: payload.weight.unwrap_or(0),
        enabled: payload.enabled.unwrap_or(true),
        target: target.clone(),
    };

    Ok((input, profile))
}

// ===========================================================================
// Mapping
// ===========================================================================

fn profile_item_entry(record: repository::ProfileItemRecord) -> ProfileItemEntry {
    let effective_job = record
        .job
        .clone()
        .unwrap_or_else(|| record.default_job.clone());
    ProfileItemEntry {
        id: record.id,
        profile_id: record.profile_id,
        exporter_id: record.exporter_id,
        exporter_slug: record.exporter_slug,
        exporter_name: record.exporter_name,
        job: record.job,
        effective_job,
        scheme: record.scheme,
        port: record.port,
        metrics_path: record.metrics_path,
        labels: record.labels,
        enabled: record.enabled,
        sort_order: record.sort_order,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn profile_detail(
    record: repository::ProfileRecord,
    items: Vec<ProfileItemEntry>,
) -> ProfileDetail {
    ProfileDetail {
        id: record.id,
        name: record.name,
        slug: record.slug,
        description: record.description,
        metric_set_id: record.metric_set_id,
        query_source_id: record.query_source_id,
        alert_source_id: record.alert_source_id,
        enabled: record.enabled,
        item_count: record.item_count,
        assignment_count: record.assignment_count,
        items,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn profile_assignment_entry(record: repository::ProfileAssignmentRecord) -> ProfileAssignmentEntry {
    ProfileAssignmentEntry {
        id: record.id,
        profile_id: record.profile_id,
        profile_slug: record.profile_slug,
        profile_name: record.profile_name,
        scope_type: record.scope_type,
        scope_id: record.scope_id,
        weight: record.weight,
        enabled: record.enabled,
        target: record.target,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

async fn profile_detail_for(
    state: &AppState,
    record: repository::ProfileRecord,
) -> Result<ProfileDetail, AppError> {
    let items = repository::list_profile_items(&state.db_pool, &[record.id]).await?;
    Ok(profile_detail(
        record,
        items.into_iter().map(profile_item_entry).collect(),
    ))
}

/// Build the canonical scrape config for a stored profile item. Goes through
/// `ProfileAssignment::to_scrape` so preview and runtime share one conversion.
fn item_scrape(
    profile: &repository::ProfileRecord,
    item: &repository::ProfileItemRecord,
    target: Option<&str>,
) -> MonitoringScrapeConfig {
    ProfileAssignment {
        id: Uuid::nil(),
        profile_id: profile.id,
        profile_slug: profile.slug.clone(),
        profile_name: profile.name.clone(),
        scope_type: "global".to_string(),
        scope_id: None,
        weight: 0,
        enabled: true,
        profile_enabled: true,
        exporter_enabled: true,
        target: target.map(str::to_string),
        exporter_id: item.exporter_id,
        exporter_name: item.exporter_name.clone(),
        exporter_slug: item.exporter_slug.clone(),
        item_id: item.id,
        default_job: item.default_job.clone(),
        default_scheme: item.default_scheme.clone(),
        default_port: item.default_port,
        default_metrics_path: item.default_metrics_path.clone(),
        default_target_source: item.default_target_source.clone(),
        item_job: item.job.clone(),
        item_scheme: item.scheme.clone(),
        item_port: item.port,
        item_metrics_path: item.metrics_path.clone(),
        item_enabled: item.enabled,
        item_sort_order: item.sort_order,
        exporter_labels: labels_from_value(Some(item.exporter_labels.clone())),
        item_labels: labels_from_value(Some(item.labels.clone())),
    }
    .to_scrape()
}

fn preview_options(state: &AppState, filters: &DiscoveryHostFilters) -> DiscoveryRequestOptions {
    let default_port = state.settings.monitoring.sd_default_port;
    DiscoveryRequestOptions {
        kind_label: None,
        target_source: TargetSource::Address,
        grouping: GroupingMode::PerTarget,
        group_fields: Vec::new(),
        default_port: (default_port > 0).then_some(default_port),
        default_scheme: state.settings.monitoring.sd_default_scheme.clone(),
        default_metrics_path: state.settings.monitoring.sd_default_metrics_path.clone(),
        default_job: trim_optional(Some(&state.settings.monitoring.sd_default_job)),
        device_job: None,
        vm_job: None,
        dedup_targets: true,
        page: DiscoveryPage {
            limit: 0,
            offset: 0,
        },
        filters: filters.clone(),
        extra_labels: HashMap::new(),
        with_port: true,
    }
}

fn scope_host_filters(
    scope_type: &str,
    scope_id: Option<Uuid>,
    fetch_limit: u32,
    label_settings: &MonitoringSdLabelGroups,
) -> DiscoveryHostFilters {
    let mut filters = DiscoveryHostFilters {
        fetch_limit,
        allowed_statuses: Some(label_settings.device_statuses.clone()),
        allowed_site_statuses: Some(label_settings.site_statuses.clone()),
        ..DiscoveryHostFilters::default()
    };
    match scope_type {
        "site" => filters.site_id = scope_id,
        "tenant" => filters.tenant_id = scope_id,
        "role" => filters.role_id = scope_id,
        "connection_target" | "device" | "virtual_machine" | "vm" => filters.host_id = scope_id,
        _ => {}
    }
    filters
}

fn target_profile_item(assignment: &ProfileAssignment) -> TargetProfileItem {
    TargetProfileItem {
        item_id: assignment.item_id,
        exporter_id: assignment.exporter_id,
        exporter_slug: assignment.exporter_slug.clone(),
        exporter_name: assignment.exporter_name.clone(),
        job: assignment
            .item_job
            .clone()
            .unwrap_or_else(|| assignment.default_job.clone()),
        scheme: assignment
            .item_scheme
            .clone()
            .unwrap_or_else(|| assignment.default_scheme.clone()),
        port: assignment.item_port.or(assignment.default_port),
        metrics_path: assignment
            .item_metrics_path
            .clone()
            .unwrap_or_else(|| assignment.default_metrics_path.clone()),
        labels: assignment.merged_labels(),
        enabled: assignment.item_enabled,
        sort_order: assignment.item_sort_order,
    }
}

fn profile_targets(
    host: &DiscoveryHost,
    assignments: &[ProfileAssignment],
    options: &DiscoveryRequestOptions,
) -> Vec<String> {
    profile_discovered_targets(host, assignments, options)
        .into_iter()
        .map(|target| target.target)
        .collect()
}

fn profile_discovered_targets(
    host: &DiscoveryHost,
    assignments: &[ProfileAssignment],
    options: &DiscoveryRequestOptions,
) -> Vec<DiscoveredTarget> {
    let mut scoped = host.clone();
    scoped.monitoring = HostMonitoringConfig {
        scrapes: assignments
            .iter()
            .filter(|assignment| assignment.item_enabled)
            .map(ProfileAssignment::to_scrape)
            .collect(),
        ..HostMonitoringConfig::default()
    };
    build_discovered_targets(vec![scoped], options)
}

// ===========================================================================
// Profiles
// ===========================================================================

/// List monitoring profiles with their items.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/profiles",
    tag = "monitoring",
    params(ProfileListQuery),
    responses(
        (status = 200, description = "Monitoring profiles with nested items", body = ProfileListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_profiles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ProfileListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match profile_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (limit, offset) = page_bounds(query.limit, query.offset);
    let search = normalize_string(query.search.as_deref());

    let (records, total) =
        match repository::list_profiles(&state.db_pool, &filter, search.as_deref(), limit, offset)
            .await
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let ids = records.iter().map(|record| record.id).collect::<Vec<_>>();
    let items = match repository::list_profile_items(&state.db_pool, &ids).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mut items_by_profile = HashMap::<Uuid, Vec<ProfileItemEntry>>::new();
    for item in items {
        items_by_profile
            .entry(item.profile_id)
            .or_default()
            .push(profile_item_entry(item));
    }

    let items = records
        .into_iter()
        .map(|record| {
            let entries = items_by_profile.remove(&record.id).unwrap_or_default();
            profile_detail(record, entries)
        })
        .collect::<Vec<_>>();

    (
        StatusCode::OK,
        Json(ProfileListResponse {
            total,
            limit,
            offset,
            items,
        }),
    )
        .into_response()
}

/// Get one monitoring profile with its items.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/profiles/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Monitoring profile id")),
    responses(
        (status = 200, description = "Monitoring profile with nested items", body = ProfileDetail),
        (status = 404, description = "Monitoring profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn get_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let record = match repository::fetch_profile_by_id(&state.db_pool, id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Monitoring profile not found".to_string(),
            ));
        }
        Err(error) => return handle_app_error(error),
    };
    match profile_detail_for(&state, record).await {
        Ok(detail) => (StatusCode::OK, Json(detail)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a monitoring profile.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/profiles",
    tag = "monitoring",
    request_body = ProfileRequest,
    responses(
        (status = 201, description = "Monitoring profile created", body = ProfileDetail),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Profile slug already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ProfileRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_profile_payload(&state, payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let record = match repository::create_profile(&state.db_pool, &input).await {
        Ok(record) => record,
        Err(error) => return handle_app_error(error),
    };
    match profile_detail_for(&state, record).await {
        Ok(detail) => (StatusCode::CREATED, Json(detail)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Replace a monitoring profile and its items.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/profiles/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Monitoring profile id")),
    request_body = ProfileRequest,
    responses(
        (status = 200, description = "Monitoring profile replaced", body = ProfileDetail),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Monitoring profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Profile slug already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ProfileRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_profile_payload(&state, payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let record = match repository::update_profile(&state.db_pool, id, &input).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Monitoring profile not found".to_string(),
            ));
        }
        Err(error) => return handle_app_error(error),
    };
    match profile_detail_for(&state, record).await {
        Ok(detail) => (StatusCode::OK, Json(detail)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring profile. Refused with 409 while any assignment exists.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/profiles/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Monitoring profile id")),
    responses(
        (status = 204, description = "Monitoring profile deleted"),
        (status = 404, description = "Monitoring profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Monitoring profile is still assigned", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_profile(
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

    let scopes = match repository::profile_assignment_scopes(&state.db_pool, id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if !scopes.is_empty() {
        return handle_app_error(AppError::Conflict(format!(
            "monitoring profile is still assigned at: {}; remove those assignments first",
            scopes.join(", ")
        )));
    }

    match repository::delete_profile(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Monitoring profile not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Apply a profile to a scope. Idempotent per `(profile, scope)`.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/profiles/{id}/apply",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Monitoring profile id")),
    request_body = ProfileApplyRequest,
    responses(
        (status = 201, description = "Profile assignment created", body = ProfileAssignmentEntry),
        (status = 200, description = "Profile assignment already existed and is unchanged", body = ProfileAssignmentEntry),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Monitoring profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Another profile owns the same job and target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn apply_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ProfileApplyRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let scope_type = match validate_monitoring_scope(&payload.scope_type, payload.scope_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request = ProfileAssignmentRequest {
        profile_id: id,
        scope_type,
        scope_id: payload.scope_id,
        weight: payload.weight,
        enabled: payload.enabled,
        target: payload.target,
    };
    let (input, profile) = match validate_assignment(&state, request).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::apply_profile_to_scope(&state.db_pool, &profile, &input).await {
        Ok((record, created)) => {
            let status = if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(profile_assignment_entry(record))).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Preview the targets a profile would produce on a scope, plus conflicts.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/profiles/{id}/preview",
    tag = "monitoring",
    params(
        ProfilePreviewQuery,
        ("id" = Uuid, Path, description = "Monitoring profile id")
    ),
    responses(
        (status = 200, description = "Expanded targets and conflicts for the profile on the scope", body = ProfilePreviewResponse),
        (status = 404, description = "Monitoring profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn preview_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<ProfilePreviewQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let scope_type = match validate_monitoring_scope(&query.scope_type, query.scope_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let profile = match repository::fetch_profile_by_id(&state.db_pool, id).await {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Monitoring profile not found".to_string(),
            ));
        }
        Err(error) => return handle_app_error(error),
    };
    let items = match repository::list_profile_items(&state.db_pool, &[profile.id]).await {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = validate_stored_profile_items(&items) {
        return handle_app_error(error);
    }

    let assignment = match repository::find_profile_assignment(
        &state.db_pool,
        profile.id,
        &scope_type,
        query.scope_id,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let target = match query.target.as_deref() {
        Some(value) => trim_optional(Some(value)),
        None => assignment
            .as_ref()
            .and_then(|assignment| assignment.target.clone()),
    };

    let label_settings = match state.repositories.monitoring_sd_label_settings.get().await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let max_hosts = state.settings.monitoring.sd_max_limit.max(1);
    let filters = scope_host_filters(&scope_type, query.scope_id, max_hosts, &label_settings);

    let visibility = match resolve_connection_target_visibility(&state, &request_context).await {
        Ok(value) => value.into_filter(),
        Err(error) => return handle_app_error(error),
    };
    let hosts =
        match fetch_discovery_hosts(&state.repositories.discovery, &filters, visibility.as_ref())
            .await
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let options = preview_options(&state, &filters);
    let mut hosts_preview = Vec::with_capacity(hosts.len());
    let mut all_targets = Vec::new();
    for host in hosts {
        let mut scoped = host.clone();
        scoped.monitoring = HostMonitoringConfig {
            scrapes: if profile.enabled {
                items
                    .iter()
                    .filter(|item| item.enabled)
                    .map(|item| item_scrape(&profile, item, target.as_deref()))
                    .collect()
            } else {
                Vec::new()
            },
            ..HostMonitoringConfig::default()
        };
        let targets = build_discovered_targets(vec![scoped.clone()], &options)
            .into_iter()
            .map(|target| target.target)
            .collect::<Vec<_>>();
        all_targets.extend(build_discovered_targets(vec![scoped], &options));
        hosts_preview.push(ProfilePreviewHost {
            host_id: host.host_id,
            host_name: host.host_name,
            host_address: host.host_address,
            kind: host.kind,
            targets,
        });
    }

    let groups = build_target_groups(
        all_targets,
        GroupingMode::PerTarget,
        &[],
        true,
        &label_settings,
    );
    let targets_count: usize = groups.iter().map(|group| group.targets.len()).sum();

    let conflicts = if profile.enabled {
        let effective_jobs = items
            .iter()
            .filter(|item| item.enabled)
            .map(|item| item.job.clone().unwrap_or_else(|| item.default_job.clone()))
            .collect::<Vec<_>>();
        match repository::list_scope_profile_jobs(&state.db_pool, &scope_type, query.scope_id).await
        {
            Ok(others) => {
                let others = others
                    .into_iter()
                    .filter(|other| other.profile_id != profile.id)
                    .collect::<Vec<_>>();
                detect_conflicts(None, &effective_jobs, target.as_deref(), &others)
            }
            Err(error) => return handle_app_error(error),
        }
    } else {
        Vec::new()
    };

    (
        StatusCode::OK,
        Json(ProfilePreviewResponse {
            groups,
            hosts: hosts_preview,
            targets_count,
            conflicts,
        }),
    )
        .into_response()
}

// ===========================================================================
// Profile assignments
// ===========================================================================

/// List profile assignments.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/profile-assignments",
    tag = "monitoring",
    params(ProfileAssignmentListQuery),
    responses(
        (status = 200, description = "Profile assignments matching the scope filters", body = ProfileAssignmentListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_profile_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ProfileAssignmentListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match assignment_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (limit, offset) = page_bounds(query.limit, query.offset);
    match repository::list_profile_assignments(&state.db_pool, &filter, limit, offset).await {
        Ok((records, total)) => {
            let items = records
                .into_iter()
                .map(profile_assignment_entry)
                .collect::<Vec<_>>();
            (
                StatusCode::OK,
                Json(ProfileAssignmentListResponse {
                    total,
                    limit,
                    offset,
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a profile assignment.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/profile-assignments",
    tag = "monitoring",
    request_body = ProfileAssignmentRequest,
    responses(
        (status = 201, description = "Profile assignment created", body = ProfileAssignmentEntry),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Monitoring profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Profile already assigned to the scope or another profile owns the same job and target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_profile_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ProfileAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let (input, _profile) = match validate_assignment(&state, payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match repository::create_profile_assignment(&state.db_pool, &input).await {
        Ok(record) => (StatusCode::CREATED, Json(profile_assignment_entry(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Replace a profile assignment.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/profile-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Profile assignment id")),
    request_body = ProfileAssignmentRequest,
    responses(
        (status = 200, description = "Profile assignment replaced", body = ProfileAssignmentEntry),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Profile assignment or profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Profile already assigned to the scope or another profile owns the same job and target", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_profile_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ProfileAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    match repository::fetch_profile_assignment_by_id(&state.db_pool, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Profile assignment not found".to_string(),
            ));
        }
        Err(error) => return handle_app_error(error),
    }

    let (input, _profile) = match validate_assignment(&state, payload).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match repository::update_profile_assignment(&state.db_pool, id, &input).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(profile_assignment_entry(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "Profile assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a profile assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/profile-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Profile assignment id")),
    responses(
        (status = 204, description = "Profile assignment deleted"),
        (status = 404, description = "Profile assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_profile_assignment(
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

    match repository::delete_profile_assignment(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Profile assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

// ===========================================================================
// Effective profiles for a connection target
// ===========================================================================

/// List the profiles that currently apply to one connection target, with their
/// expanded items, targets and conflicts.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/targets/{id}/profiles",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Connection target id")),
    responses(
        (status = 200, description = "Effective profiles, expanded items and conflicts for the target", body = TargetProfilesResponse),
        (status = 404, description = "Connection target not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn target_profiles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_read(&state, &request_context, target_id).await {
        return handle_app_error(error);
    }

    let filters = DiscoveryHostFilters {
        host_id: Some(target_id),
        fetch_limit: 1,
        ..DiscoveryHostFilters::default()
    };
    let mut hosts = match fetch_discovery_hosts(&state.repositories.discovery, &filters, None).await
    {
        Ok(hosts) => hosts,
        Err(error) => return handle_app_error(error),
    };
    let Some(host) = hosts.pop() else {
        return handle_app_error(AppError::NotFound(
            "Connection target not found".to_string(),
        ));
    };

    let (host_ids, site_ids, tenant_ids, role_ids) = collect_scope_ids(std::slice::from_ref(&host));
    let assignments = match monitoring_persistence::fetch_profile_assignments_for_hosts(
        &state.db_pool,
        HostScopeIds {
            host_ids: &host_ids,
            site_ids: &site_ids,
            tenant_ids: &tenant_ids,
            role_ids: &role_ids,
        },
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let assignments = effective_profile_assignments_for_host(&host, &assignments);

    let mut order = Vec::<(Uuid, Uuid)>::new();
    let mut by_profile = HashMap::<(Uuid, Uuid), Vec<ProfileAssignment>>::new();
    for assignment in &assignments {
        let key = (assignment.profile_id, assignment.id);
        if !by_profile.contains_key(&key) {
            order.push(key);
        }
        by_profile.entry(key).or_default().push(assignment.clone());
    }

    let options = preview_options(
        &state,
        &DiscoveryHostFilters {
            host_id: Some(target_id),
            ..DiscoveryHostFilters::default()
        },
    );

    let mut profiles = Vec::with_capacity(order.len());
    for key in order {
        let mut rows = by_profile.remove(&key).unwrap_or_default();
        rows.sort_by(|left, right| {
            (
                &left.exporter_slug,
                left.id,
                left.item_sort_order,
                left.item_id,
            )
                .cmp(&(
                    &right.exporter_slug,
                    right.id,
                    right.item_sort_order,
                    right.item_id,
                ))
        });
        let Some(first) = rows.first() else {
            continue;
        };
        profiles.push(TargetProfile {
            profile_id: first.profile_id,
            profile_slug: first.profile_slug.clone(),
            profile_name: first.profile_name.clone(),
            assignment_id: first.id,
            scope_type: first.scope_type.clone(),
            scope_id: first.scope_id,
            weight: first.weight,
            target: first.target.clone(),
            targets: profile_targets(&host, &rows, &options),
            items: rows.iter().map(target_profile_item).collect(),
        });
    }

    let conflicts = effective_conflicts(&host, &assignments, &options);
    (
        StatusCode::OK,
        Json(TargetProfilesResponse {
            target_id,
            profiles,
            conflicts,
        }),
    )
        .into_response()
}

fn effective_conflicts(
    host: &DiscoveryHost,
    assignments: &[ProfileAssignment],
    options: &DiscoveryRequestOptions,
) -> Vec<ProfileConflict> {
    let mut seen = HashSet::new();
    let mut endpoints = HashMap::<(String, String), Vec<&ProfileAssignment>>::new();
    let mut conflicts = Vec::new();
    for assignment in assignments {
        for resolved in profile_discovered_targets(host, std::slice::from_ref(assignment), options)
        {
            let Some(job) = resolved.labels.get("job").cloned() else {
                continue;
            };
            let target = resolved.target;
            let owners = endpoints.entry((job.clone(), target.clone())).or_default();
            for other in owners.iter() {
                if other.id == assignment.id
                    || !seen.insert((other.id, assignment.id, job.clone(), target.clone()))
                {
                    continue;
                }
                conflicts.push(ProfileConflict {
                    profile_id: assignment.profile_id,
                    profile_slug: assignment.profile_slug.clone(),
                    profile_name: assignment.profile_name.clone(),
                    job: job.clone(),
                    target: Some(target.clone()),
                    message: conflict_message(&job, Some(&target), &other.profile_slug),
                });
            }
            owners.push(assignment);
        }
    }
    conflicts
}
