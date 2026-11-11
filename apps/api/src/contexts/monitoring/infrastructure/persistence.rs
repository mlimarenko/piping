//! Read-model persistence: batch queries resolving the effective exporter /
//! query-source / alert-source / metric-set configuration for a set of
//! discovered hosts, scoped by host/site/tenant/role.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::{AppError, classify_sqlx_error_owned};

use super::super::domain::host::{
    MonitoringMetricDescriptor, MonitoringQuerySourceConfig, MonitoringScrapeConfig,
};

/// One effective profile item for a host: the assignment that selected the
/// profile, the profile identity, and the exporter defaults plus item overrides.
#[derive(Debug, Clone)]
pub struct ProfileAssignment {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub profile_slug: String,
    pub profile_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub profile_enabled: bool,
    pub exporter_enabled: bool,
    pub target: Option<String>,
    pub exporter_id: Uuid,
    pub exporter_name: String,
    pub exporter_slug: String,
    pub item_id: Uuid,
    pub default_job: String,
    pub default_scheme: String,
    pub default_port: Option<i32>,
    pub default_metrics_path: String,
    pub default_target_source: String,
    pub item_job: Option<String>,
    pub item_scheme: Option<String>,
    pub item_port: Option<i32>,
    pub item_metrics_path: Option<String>,
    pub item_enabled: bool,
    pub item_sort_order: i32,
    pub exporter_labels: HashMap<String, String>,
    pub item_labels: HashMap<String, String>,
}

impl ProfileAssignment {
    /// Exporter labels first, then item labels on top.
    #[must_use]
    pub fn merged_labels(&self) -> HashMap<String, String> {
        let mut labels = self.exporter_labels.clone();
        labels.extend(self.item_labels.clone());
        labels
    }

    /// Effective scrape configuration for this item: item overrides win over
    /// the exporter defaults, and the assignment target applies to every item.
    #[must_use]
    pub fn to_scrape(&self) -> MonitoringScrapeConfig {
        let port = self.item_port.or(self.default_port);
        let port = port.and_then(|value| u16::try_from(value).ok());
        MonitoringScrapeConfig {
            name: Some(self.profile_name.clone()),
            enabled: Some(true),
            target: self.target.clone(),
            target_source: Some(self.default_target_source.clone()),
            job: self
                .item_job
                .clone()
                .or_else(|| Some(self.default_job.clone())),
            scheme: self
                .item_scheme
                .clone()
                .or_else(|| Some(self.default_scheme.clone())),
            metrics_path: self
                .item_metrics_path
                .clone()
                .or_else(|| Some(self.default_metrics_path.clone())),
            port,
            labels: self.merged_labels(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricQueryRecord {
    pub metric_query_id: Uuid,
    pub descriptor: MonitoringMetricDescriptor,
    pub show_in_inspector: bool,
}

#[derive(Debug, Clone)]
pub struct QuerySourceAssignment {
    pub id: Uuid,
    pub source: MonitoringQuerySourceConfig,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
}

/// A scope-resolved alert source, reusing `MonitoringQuerySourceConfig` for the
/// connection parameters (URL/credential/timeout/options) since alert sources
/// share the same SSRF-pinned, bearer-auth query path as metric query sources.
#[derive(Debug, Clone)]
pub struct AlertSourceAssignment {
    pub id: Uuid,
    pub source: MonitoringQuerySourceConfig,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
}

#[derive(Debug, Clone)]
pub struct MetricSetAssignment {
    pub id: Uuid,
    pub metric_set_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
}

#[derive(Debug, Clone)]
pub struct MetricSetItem {
    pub metric_query_id: Uuid,
    pub descriptor: MonitoringMetricDescriptor,
    pub show_in_inspector: bool,
    /// Exporter that owns the underlying metric query, when it has one.
    ///
    /// The read model uses this to keep canonical availability selection tied
    /// to the exporters actually applied to a host: a metric-set or
    /// default-list descriptor whose exporter is not scraped must not win
    /// `semantic = 'availability'` over an applied exporter's descriptor.
    /// `None` for exporter-less queries (for example the Zabbix bundle).
    pub exporter_id: Option<Uuid>,
}

/// Resolved override row used by the descriptor merge step.
///
/// Identical in shape to the persisted record, minus row-management metadata,
/// so the descriptor merge step in the read model does not depend on the
/// CRUD record type.
#[derive(Debug, Clone, Default)]
pub struct HostOverrideResolved {
    pub label_override: Option<String>,
    pub unit_override: Option<String>,
    pub options_override: Option<Value>,
}

/// Batch-fetch enabled host overrides for a set of hosts, keyed by
/// `(host_id, metric_query_id)`. Disabled rows are skipped.
pub async fn fetch_host_overrides_for_hosts(
    pool: &PgPool,
    host_ids: &[Uuid],
) -> Result<HashMap<(Uuid, Uuid), HostOverrideResolved>, AppError> {
    if host_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT metric_query_id, host_id, label_override, unit_override, options_override
        FROM monitoring_metric_query_host_override
        WHERE enabled = true AND host_id = ANY($1::uuid[])
        "#,
    )
    .bind(host_ids)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let mut by_pair = HashMap::with_capacity(rows.len());
    for row in &rows {
        let host_id: Uuid = row.try_get("host_id").map_err(classify_sqlx_error_owned)?;
        let metric_query_id: Uuid = row
            .try_get("metric_query_id")
            .map_err(classify_sqlx_error_owned)?;
        by_pair.insert(
            (host_id, metric_query_id),
            HostOverrideResolved {
                label_override: row.try_get("label_override").ok(),
                unit_override: row.try_get("unit_override").ok(),
                options_override: row
                    .try_get::<Option<Value>, _>("options_override")
                    .ok()
                    .flatten(),
            },
        );
    }
    Ok(by_pair)
}

// ===========================================================================
// Service-discovery enrichment queries
//
// These power the `/service-discovery` host enrichment path in `monitoring.rs`.
// They resolve the *effective* exporter / query-source / metric-set
// configuration for a set of discovered hosts, scoped by host/site/tenant/role.
// ===========================================================================

/// Pre-collected scope ids for a batch of discovered hosts, used to resolve
/// scope-typed assignments in a single query without leaking the host type into
/// the persistence layer.
#[derive(Clone, Copy)]
pub struct HostScopeIds<'a> {
    pub host_ids: &'a [Uuid],
    pub site_ids: &'a [Uuid],
    pub tenant_ids: &'a [Uuid],
    pub role_ids: &'a [Uuid],
}

/// Project a JSON labels object onto the flat `String -> String` map the read
/// model resolves, dropping any non-scalar values.
///
/// Shared with the impact preview so an unsaved draft renders exactly the
/// labels a saved assignment renders.
pub fn labels_from_value(value: Option<Value>) -> HashMap<String, String> {
    let Some(Value::Object(map)) = value else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(key, value)| {
            let value = match value {
                Value::String(value) => value,
                Value::Number(value) => value.to_string(),
                Value::Bool(value) => value.to_string(),
                _ => return None,
            };
            Some((key, value))
        })
        .collect()
}

/// Shallow-merge `override_value` over `base`, keeping `base` keys not present
/// in the override. Non-object inputs are treated as empty objects.
fn merge_json_objects(base: Value, override_value: Value) -> Value {
    let mut merged = match base {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    if let Value::Object(map) = override_value {
        for (key, value) in map {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

fn timeout_from_database(timeout_ms: i32) -> Option<u64> {
    u64::try_from(timeout_ms).ok()
}

fn push_grouped<T>(groups: &mut HashMap<Uuid, Vec<T>>, group_id: Uuid, item: T) {
    groups.entry(group_id).or_default().push(item);
}

fn resolve_global_query_sources(
    sources: impl IntoIterator<Item = MonitoringQuerySourceConfig>,
) -> Vec<MonitoringQuerySourceConfig> {
    // The query orders candidates by ascending weight/id, so later candidates
    // deliberately replace earlier ones of the same source type.
    let mut seen_source_types = HashSet::new();
    let mut sources = sources.into_iter().collect::<Vec<_>>();
    sources.reverse();
    sources.retain(|source| seen_source_types.insert(source.source_type.to_ascii_lowercase()));
    sources
}

struct MetricSetItemPresentation {
    label: Option<String>,
    unit: Option<String>,
    show_in_list: bool,
}

fn resolve_metric_set_item_presentation(
    label_override: Option<String>,
    default_label: Option<String>,
    unit_override: Option<String>,
    default_unit: Option<String>,
    item_show_in_list: bool,
    default_show_in_list: bool,
) -> MetricSetItemPresentation {
    MetricSetItemPresentation {
        label: label_override.or(default_label),
        unit: unit_override.or(default_unit),
        show_in_list: item_show_in_list || default_show_in_list,
    }
}

/// Resolve all enabled profile items that apply to the given connection target
/// scopes. Assignment, profile, item and exporter must all be enabled; only
/// `monitoring_profile_assignment -> monitoring_profile ->
/// monitoring_profile_item -> monitoring_exporter` is read.
pub async fn fetch_profile_assignments_for_hosts(
    pool: &PgPool,
    scopes: HostScopeIds<'_>,
) -> Result<Vec<ProfileAssignment>, AppError> {
    fetch_profile_assignments(pool, scopes, true).await
}

/// Resolve every profile item that applies to the given connection target
/// scopes, including disabled assignments, profiles, items and exporters.
///
/// The coverage preview needs the disabled rows to distinguish "no assignment
/// applies" from "an assignment applies but is disabled".
pub async fn fetch_profile_assignment_candidates_for_hosts(
    pool: &PgPool,
    scopes: HostScopeIds<'_>,
) -> Result<Vec<ProfileAssignment>, AppError> {
    fetch_profile_assignments(pool, scopes, false).await
}

async fn fetch_profile_assignments(
    pool: &PgPool,
    scopes: HostScopeIds<'_>,
    only_enabled: bool,
) -> Result<Vec<ProfileAssignment>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT
            a.id,
            p.id AS profile_id,
            p.slug AS profile_slug,
            p.name AS profile_name,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            p.enabled AS profile_enabled,
            a.target,
            i.exporter_id,
            i.id AS item_id,
            i.job AS item_job,
            i.scheme AS item_scheme,
            i.port AS item_port,
            i.metrics_path AS item_metrics_path,
            i.labels AS item_labels,
            i.enabled AS item_enabled,
            i.sort_order AS item_sort_order,
            e.name AS exporter_name,
            e.slug AS exporter_slug,
            e.enabled AS exporter_enabled,
            e.default_job,
            e.default_scheme,
            e.default_port,
            e.default_metrics_path,
            e.default_target_source,
            e.labels AS exporter_labels
        FROM monitoring_profile_assignment a
        JOIN monitoring_profile p ON p.id = a.profile_id
        JOIN monitoring_profile_item i ON i.profile_id = p.id
        JOIN monitoring_exporter e ON e.id = i.exporter_id
        WHERE (
            NOT $5
            OR (a.enabled = true AND p.enabled = true AND i.enabled = true AND e.enabled = true)
          )
          AND (
            (a.scope_type = 'global' AND a.scope_id IS NULL)
            OR (a.scope_type IN ('connection_target', 'device', 'virtual_machine', 'vm') AND a.scope_id = ANY($1::uuid[]))
            OR (a.scope_type = 'site' AND a.scope_id = ANY($2::uuid[]))
            OR (a.scope_type = 'tenant' AND a.scope_id = ANY($3::uuid[]))
            OR (a.scope_type = 'role' AND a.scope_id = ANY($4::uuid[]))
          )
        "#,
    )
    .bind(scopes.host_ids)
    .bind(scopes.site_ids)
    .bind(scopes.tenant_ids)
    .bind(scopes.role_ids)
    .bind(only_enabled)
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    rows.into_iter()
        .map(|row| {
            Ok(ProfileAssignment {
                id: row.try_get("id").map_err(AppError::DatabaseSource)?,
                profile_id: row
                    .try_get("profile_id")
                    .map_err(AppError::DatabaseSource)?,
                profile_slug: row
                    .try_get("profile_slug")
                    .map_err(AppError::DatabaseSource)?,
                profile_name: row
                    .try_get("profile_name")
                    .map_err(AppError::DatabaseSource)?,
                scope_type: row
                    .try_get("scope_type")
                    .map_err(AppError::DatabaseSource)?,
                scope_id: row.try_get("scope_id").ok(),
                weight: row.try_get("weight").map_err(AppError::DatabaseSource)?,
                enabled: row.try_get("enabled").map_err(AppError::DatabaseSource)?,
                profile_enabled: row
                    .try_get("profile_enabled")
                    .map_err(AppError::DatabaseSource)?,
                exporter_enabled: row
                    .try_get("exporter_enabled")
                    .map_err(AppError::DatabaseSource)?,
                target: row.try_get("target").ok(),
                exporter_id: row
                    .try_get("exporter_id")
                    .map_err(AppError::DatabaseSource)?,
                exporter_name: row
                    .try_get("exporter_name")
                    .map_err(AppError::DatabaseSource)?,
                exporter_slug: row
                    .try_get("exporter_slug")
                    .map_err(AppError::DatabaseSource)?,
                item_id: row.try_get("item_id").map_err(AppError::DatabaseSource)?,
                default_job: row
                    .try_get("default_job")
                    .map_err(AppError::DatabaseSource)?,
                default_scheme: row
                    .try_get("default_scheme")
                    .map_err(AppError::DatabaseSource)?,
                default_port: row.try_get("default_port").ok(),
                default_metrics_path: row
                    .try_get("default_metrics_path")
                    .map_err(AppError::DatabaseSource)?,
                default_target_source: row
                    .try_get("default_target_source")
                    .map_err(AppError::DatabaseSource)?,
                item_job: row.try_get("item_job").ok(),
                item_scheme: row.try_get("item_scheme").ok(),
                item_port: row.try_get("item_port").ok(),
                item_metrics_path: row.try_get("item_metrics_path").ok(),
                item_enabled: row
                    .try_get("item_enabled")
                    .map_err(AppError::DatabaseSource)?,
                item_sort_order: row
                    .try_get("item_sort_order")
                    .map_err(AppError::DatabaseSource)?,
                exporter_labels: labels_from_value(row.try_get("exporter_labels").ok()),
                item_labels: labels_from_value(row.try_get("item_labels").ok()),
            })
        })
        .collect()
}

/// Resolve enabled metric queries for the given exporters, grouped by exporter id.
pub async fn fetch_metric_queries_for_exporters(
    pool: &PgPool,
    exporter_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<MetricQueryRecord>>, AppError> {
    if exporter_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, exporter_id, source_type, query_type, value_type, widget_type,
               name, label, query, unit, description, default_show_in_inspector,
               default_show_in_list, sort_order, options, category, semantic
        FROM monitoring_metric_query
        WHERE enabled = true
          AND exporter_id = ANY($1::uuid[])
        ORDER BY exporter_id, sort_order, name
        "#,
    )
    .bind(exporter_ids)
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    let mut by_exporter = HashMap::<Uuid, Vec<MetricQueryRecord>>::new();
    for row in rows {
        let exporter_id = row
            .try_get::<Uuid, _>("exporter_id")
            .map_err(AppError::DatabaseSource)?;
        let metric_query_id: Uuid = row.try_get("id").map_err(AppError::DatabaseSource)?;
        push_grouped(
            &mut by_exporter,
            exporter_id,
            MetricQueryRecord {
                metric_query_id,
                descriptor: MonitoringMetricDescriptor {
                    name: row.try_get("name").map_err(AppError::DatabaseSource)?,
                    label: row.try_get("label").ok(),
                    query: row.try_get("query").map_err(AppError::DatabaseSource)?,
                    unit: row.try_get("unit").ok(),
                    value_type: row.try_get("value_type").ok(),
                    widget_type: row.try_get("widget_type").ok(),
                    description: row.try_get("description").ok(),
                    source_type: row.try_get("source_type").ok(),
                    query_type: row.try_get("query_type").ok(),
                    options: row
                        .try_get("options")
                        .unwrap_or_else(|_| Value::Object(Default::default())),
                    // Exporter metric queries have no per-set override; the
                    // metric-level default is the resolved list flag and the
                    // render hint defaults to `auto`.
                    show_in_list: row
                        .try_get("default_show_in_list")
                        .map_err(AppError::DatabaseSource)?,
                    list_render: "auto".to_string(),
                    metric_query_id: Some(metric_query_id),
                    category: row.try_get("category").ok(),
                    semantic: row.try_get("semantic").ok(),
                },
                show_in_inspector: row
                    .try_get("default_show_in_inspector")
                    .map_err(AppError::DatabaseSource)?,
            },
        );
    }
    Ok(by_exporter)
}

/// Resolve all enabled metric descriptors regardless of exporter (global view).
pub async fn fetch_enabled_metric_descriptors(
    pool: &PgPool,
) -> Result<Vec<MonitoringMetricDescriptor>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT name, label, query, unit, value_type, widget_type, description, source_type, query_type,
               default_show_in_list, options, category, semantic
        FROM monitoring_metric_query
        WHERE enabled = true
        ORDER BY sort_order, name
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    rows.into_iter()
        .map(|row| {
            Ok(MonitoringMetricDescriptor {
                name: row.try_get("name").map_err(AppError::DatabaseSource)?,
                label: row.try_get("label").ok(),
                query: row.try_get("query").map_err(AppError::DatabaseSource)?,
                unit: row.try_get("unit").ok(),
                value_type: row.try_get("value_type").ok(),
                widget_type: row.try_get("widget_type").ok(),
                description: row.try_get("description").ok(),
                source_type: row.try_get("source_type").ok(),
                query_type: row.try_get("query_type").ok(),
                options: row
                    .try_get("options")
                    .unwrap_or_else(|_| Value::Object(Default::default())),
                show_in_list: row
                    .try_get("default_show_in_list")
                    .map_err(AppError::DatabaseSource)?,
                list_render: "auto".to_string(),
                // Global-view exporter does not surface the metric_query_id
                // join key; consumers of this path do not need per-host
                // override matching.
                metric_query_id: None,
                category: row.try_get("category").ok(),
                semantic: row.try_get("semantic").ok(),
            })
        })
        .collect()
}

/// Resolve the globally-scoped query sources (one per `source_type`).
pub async fn fetch_global_query_sources(
    pool: &PgPool,
) -> Result<Vec<MonitoringQuerySourceConfig>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT
            s.name,
            s.source_type,
            s.query_url,
            credential_binding.secret_id AS credential_secret_id,
            s.timeout_ms,
            s.options AS source_options,
            a.options AS assignment_options
        FROM monitoring_query_source_assignment a
        JOIN monitoring_query_source s ON s.id = a.query_source_id
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_query_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        WHERE a.enabled = true
          AND s.enabled = true
          AND a.scope_type = 'global'
          AND a.scope_id IS NULL
        ORDER BY a.weight, a.id
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    let sources = rows
        .into_iter()
        .map(|row| {
            let timeout_ms = row
                .try_get::<i32, _>("timeout_ms")
                .map_err(AppError::DatabaseSource)?;
            let source_type = row
                .try_get::<String, _>("source_type")
                .map_err(AppError::DatabaseSource)?;
            Ok(MonitoringQuerySourceConfig {
                name: row.try_get("name").ok(),
                source_type,
                query_url: row.try_get("query_url").ok(),
                // Only SQL NULL represents an intentionally unbound source.
                // Do not turn a joined-column/schema decoding failure into an
                // unauthenticated query request.
                credential_secret_id: row
                    .try_get::<Option<Uuid>, _>("credential_secret_id")
                    .map_err(AppError::DatabaseSource)?,
                timeout_ms: timeout_from_database(timeout_ms),
                options: merge_json_objects(
                    row.try_get("source_options").unwrap_or(Value::Null),
                    row.try_get("assignment_options").unwrap_or(Value::Null),
                ),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(resolve_global_query_sources(sources))
}

/// Resolve all enabled query-source assignments that apply to the given connection target scopes.
pub async fn fetch_query_source_assignments_for_hosts(
    pool: &PgPool,
    scopes: HostScopeIds<'_>,
) -> Result<Vec<QuerySourceAssignment>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT
            a.id,
            a.scope_type,
            a.scope_id,
            a.weight,
            s.name,
            s.source_type,
            s.query_url,
            credential_binding.secret_id AS credential_secret_id,
            s.timeout_ms,
            s.options AS source_options,
            a.options AS assignment_options
        FROM monitoring_query_source_assignment a
        JOIN monitoring_query_source s ON s.id = a.query_source_id
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_query_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        WHERE a.enabled = true
          AND s.enabled = true
          AND (
            (a.scope_type = 'global' AND a.scope_id IS NULL)
            OR (a.scope_type IN ('connection_target', 'device', 'virtual_machine', 'vm') AND a.scope_id = ANY($1::uuid[]))
            OR (a.scope_type = 'site' AND a.scope_id = ANY($2::uuid[]))
            OR (a.scope_type = 'tenant' AND a.scope_id = ANY($3::uuid[]))
            OR (a.scope_type = 'role' AND a.scope_id = ANY($4::uuid[]))
          )
        "#,
    )
    .bind(scopes.host_ids)
    .bind(scopes.site_ids)
    .bind(scopes.tenant_ids)
    .bind(scopes.role_ids)
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    rows.into_iter()
        .map(|row| {
            let timeout_ms = row
                .try_get::<i32, _>("timeout_ms")
                .map_err(AppError::DatabaseSource)?;
            Ok(QuerySourceAssignment {
                id: row.try_get("id").map_err(AppError::DatabaseSource)?,
                source: MonitoringQuerySourceConfig {
                    name: row.try_get("name").ok(),
                    source_type: row
                        .try_get("source_type")
                        .map_err(AppError::DatabaseSource)?,
                    query_url: row.try_get("query_url").ok(),
                    // Only SQL NULL represents an intentionally unbound source.
                    // Do not turn a joined-column/schema decoding failure into an
                    // unauthenticated query request.
                    credential_secret_id: row
                        .try_get::<Option<Uuid>, _>("credential_secret_id")
                        .map_err(AppError::DatabaseSource)?,
                    timeout_ms: timeout_from_database(timeout_ms),
                    options: merge_json_objects(
                        row.try_get("source_options").unwrap_or(Value::Null),
                        row.try_get("assignment_options").unwrap_or(Value::Null),
                    ),
                },
                scope_type: row
                    .try_get("scope_type")
                    .map_err(AppError::DatabaseSource)?,
                scope_id: row.try_get("scope_id").ok(),
                weight: row.try_get("weight").map_err(AppError::DatabaseSource)?,
            })
        })
        .collect()
}

/// Resolve all enabled alert-source assignments that apply to the given connection target scopes.
///
/// The alert-source assignment table carries no per-assignment `options`
/// (it mirrors `monitoring_metric_set_assignment`), so the resolved source
/// `options` come solely from the alert source row.
pub async fn fetch_alert_source_assignments_for_hosts(
    pool: &PgPool,
    scopes: HostScopeIds<'_>,
) -> Result<Vec<AlertSourceAssignment>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT
            a.id,
            a.scope_type,
            a.scope_id,
            a.weight,
            s.name,
            s.source_type,
            s.query_url,
            credential_binding.secret_id AS credential_secret_id,
            s.timeout_ms,
            s.options AS source_options
        FROM monitoring_alert_source_assignment a
        JOIN monitoring_alert_source s ON s.id = a.alert_source_id
        LEFT JOIN secret_usage_binding credential_binding
          ON credential_binding.consumer_type = 'monitoring_alert_source'
         AND credential_binding.consumer_id = s.id
         AND credential_binding.field_key = 'bearer_token'
        WHERE a.enabled = true
          AND s.enabled = true
          AND (
            (a.scope_type = 'global' AND a.scope_id IS NULL)
            OR (a.scope_type IN ('connection_target', 'device', 'virtual_machine', 'vm') AND a.scope_id = ANY($1::uuid[]))
            OR (a.scope_type = 'site' AND a.scope_id = ANY($2::uuid[]))
            OR (a.scope_type = 'tenant' AND a.scope_id = ANY($3::uuid[]))
            OR (a.scope_type = 'role' AND a.scope_id = ANY($4::uuid[]))
          )
        "#,
    )
    .bind(scopes.host_ids)
    .bind(scopes.site_ids)
    .bind(scopes.tenant_ids)
    .bind(scopes.role_ids)
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    rows.into_iter()
        .map(|row| {
            let timeout_ms = row
                .try_get::<i32, _>("timeout_ms")
                .map_err(AppError::DatabaseSource)?;
            Ok(AlertSourceAssignment {
                id: row.try_get("id").map_err(AppError::DatabaseSource)?,
                source: MonitoringQuerySourceConfig {
                    name: row.try_get("name").ok(),
                    source_type: row
                        .try_get("source_type")
                        .map_err(AppError::DatabaseSource)?,
                    query_url: row.try_get("query_url").ok(),
                    // Only SQL NULL represents an intentionally unbound source.
                    // Do not turn a joined-column/schema decoding failure into an
                    // unauthenticated alert request.
                    credential_secret_id: row
                        .try_get::<Option<Uuid>, _>("credential_secret_id")
                        .map_err(AppError::DatabaseSource)?,
                    timeout_ms: timeout_from_database(timeout_ms),
                    options: row
                        .try_get("source_options")
                        .unwrap_or(Value::Object(serde_json::Map::new())),
                },
                scope_type: row
                    .try_get("scope_type")
                    .map_err(AppError::DatabaseSource)?,
                scope_id: row.try_get("scope_id").ok(),
                weight: row.try_get("weight").map_err(AppError::DatabaseSource)?,
            })
        })
        .collect()
}

/// Resolve all enabled metric-set assignments that apply to the given connection target scopes.
pub async fn fetch_metric_set_assignments_for_hosts(
    pool: &PgPool,
    scopes: HostScopeIds<'_>,
) -> Result<Vec<MetricSetAssignment>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.metric_set_id, a.scope_type, a.scope_id, a.weight
        FROM monitoring_metric_set_assignment a
        JOIN monitoring_metric_set s ON s.id = a.metric_set_id
        WHERE a.enabled = true
          AND s.enabled = true
          AND (
            (a.scope_type = 'global' AND a.scope_id IS NULL)
            OR (a.scope_type IN ('connection_target', 'device', 'virtual_machine', 'vm') AND a.scope_id = ANY($1::uuid[]))
            OR (a.scope_type = 'site' AND a.scope_id = ANY($2::uuid[]))
            OR (a.scope_type = 'tenant' AND a.scope_id = ANY($3::uuid[]))
            OR (a.scope_type = 'role' AND a.scope_id = ANY($4::uuid[]))
          )
        "#,
    )
    .bind(scopes.host_ids)
    .bind(scopes.site_ids)
    .bind(scopes.tenant_ids)
    .bind(scopes.role_ids)
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    rows.into_iter()
        .map(|row| {
            Ok(MetricSetAssignment {
                id: row.try_get("id").map_err(AppError::DatabaseSource)?,
                metric_set_id: row
                    .try_get("metric_set_id")
                    .map_err(AppError::DatabaseSource)?,
                scope_type: row
                    .try_get("scope_type")
                    .map_err(AppError::DatabaseSource)?,
                scope_id: row.try_get("scope_id").ok(),
                weight: row.try_get("weight").map_err(AppError::DatabaseSource)?,
            })
        })
        .collect()
}

/// Resolve the enabled items of the given metric sets, grouped by metric-set id.
pub async fn fetch_metric_set_items(
    pool: &PgPool,
    metric_set_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<MetricSetItem>>, AppError> {
    if metric_set_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT
            i.metric_set_id,
            i.metric_query_id,
            i.show_in_inspector,
            i.show_in_list AS item_show_in_list,
            q.default_show_in_list,
            i.list_render,
            i.sort_order,
            i.label_override,
            q.label AS default_label,
            i.unit_override,
            q.unit AS default_unit,
            q.value_type,
            q.widget_type,
            q.name,
            q.query,
            q.description,
            q.source_type,
            q.query_type,
            q.options,
            q.category,
            q.semantic,
            q.exporter_id
        FROM monitoring_metric_set_item i
        JOIN monitoring_metric_query q ON q.id = i.metric_query_id
        WHERE q.enabled = true
          AND i.metric_set_id = ANY($1::uuid[])
        ORDER BY i.metric_set_id, i.sort_order, q.name
        "#,
    )
    .bind(metric_set_ids)
    .fetch_all(pool)
    .await
    .map_err(AppError::DatabaseSource)?;

    let mut by_set = HashMap::<Uuid, Vec<MetricSetItem>>::new();
    for row in rows {
        let metric_set_id = row
            .try_get::<Uuid, _>("metric_set_id")
            .map_err(AppError::DatabaseSource)?;
        let metric_query_id: Uuid = row
            .try_get("metric_query_id")
            .map_err(AppError::DatabaseSource)?;
        let presentation = resolve_metric_set_item_presentation(
            row.try_get("label_override").ok(),
            row.try_get("default_label").ok(),
            row.try_get("unit_override").ok(),
            row.try_get("default_unit").ok(),
            row.try_get("item_show_in_list")
                .map_err(AppError::DatabaseSource)?,
            row.try_get("default_show_in_list")
                .map_err(AppError::DatabaseSource)?,
        );
        push_grouped(
            &mut by_set,
            metric_set_id,
            MetricSetItem {
                metric_query_id,
                descriptor: MonitoringMetricDescriptor {
                    name: row.try_get("name").map_err(AppError::DatabaseSource)?,
                    label: presentation.label,
                    query: row.try_get("query").map_err(AppError::DatabaseSource)?,
                    unit: presentation.unit,
                    value_type: row.try_get("value_type").ok(),
                    widget_type: row.try_get("widget_type").ok(),
                    description: row.try_get("description").ok(),
                    source_type: row.try_get("source_type").ok(),
                    query_type: row.try_get("query_type").ok(),
                    options: row
                        .try_get("options")
                        .unwrap_or_else(|_| Value::Object(Default::default())),
                    // Resolved list flag = set-item override OR metric default;
                    // render hint comes from the set item.
                    show_in_list: presentation.show_in_list,
                    list_render: row
                        .try_get("list_render")
                        .map_err(AppError::DatabaseSource)?,
                    metric_query_id: Some(metric_query_id),
                    category: row.try_get("category").ok(),
                    semantic: row.try_get("semantic").ok(),
                },
                show_in_inspector: row
                    .try_get("show_in_inspector")
                    .map_err(AppError::DatabaseSource)?,
                exporter_id: row.try_get("exporter_id").ok(),
            },
        );
    }
    Ok(by_set)
}

/// Fetch the set of enabled metric queries that opt into the default list view
/// (`default_show_in_list = true`). Used by the default-set fallback in
/// `enrich_hosts_with_monitoring` when a host has no explicit metric set.
pub async fn fetch_default_list_descriptors(pool: &PgPool) -> Result<Vec<MetricSetItem>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, label, query, unit, value_type, widget_type, description,
               source_type, query_type, options, category, semantic, exporter_id
        FROM monitoring_metric_query
        WHERE enabled = true
          AND default_show_in_list = true
        ORDER BY sort_order, name
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let metric_query_id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
        items.push(MetricSetItem {
            metric_query_id,
            descriptor: MonitoringMetricDescriptor {
                name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
                label: row.try_get("label").ok(),
                query: row.try_get("query").map_err(classify_sqlx_error_owned)?,
                unit: row.try_get("unit").ok(),
                value_type: row.try_get("value_type").ok(),
                widget_type: row.try_get("widget_type").ok(),
                description: row.try_get("description").ok(),
                source_type: row.try_get("source_type").ok(),
                query_type: row.try_get("query_type").ok(),
                options: row
                    .try_get("options")
                    .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
                // Resolved from `default_show_in_list = true` (we filtered).
                show_in_list: true,
                list_render: "auto".to_string(),
                metric_query_id: Some(metric_query_id),
                category: row.try_get("category").ok(),
                semantic: row.try_get("semantic").ok(),
            },
            // Default fallback metrics never claim an inspector slot — the
            // inspector view stays explicitly assigned.
            show_in_inspector: false,
            exporter_id: row.try_get("exporter_id").ok(),
        });
    }

    Ok(items)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    fn unreachable_pool() -> PgPool {
        PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(100))
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid")
            .expect("lazy PostgreSQL pool")
    }

    fn query_source(source_type: &str, query_url: &str) -> MonitoringQuerySourceConfig {
        MonitoringQuerySourceConfig {
            source_type: source_type.to_string(),
            query_url: Some(query_url.to_string()),
            ..MonitoringQuerySourceConfig::default()
        }
    }

    #[test]
    fn labels_keep_scalars_and_drop_structured_values() {
        let labels = labels_from_value(Some(json!({
            "text": "node",
            "count": 2.5,
            "enabled": true,
            "nested": {"region": "eu"},
            "list": ["a"],
            "missing": null
        })));

        assert_eq!(
            labels,
            HashMap::from([
                ("count".to_string(), "2.5".to_string()),
                ("enabled".to_string(), "true".to_string()),
                ("text".to_string(), "node".to_string()),
            ])
        );
    }

    #[test]
    fn option_merge_is_shallow_and_ignores_non_object_sides() {
        let merged = merge_json_objects(
            json!({
                "display": {"warning": 70, "critical": 90},
                "step": 5,
                "shared": "source"
            }),
            json!({
                "display": {"warning": 80},
                "shared": "assignment"
            }),
        );
        assert_eq!(
            merged,
            json!({
                "display": {"warning": 80},
                "step": 5,
                "shared": "assignment"
            })
        );
        assert_eq!(
            merge_json_objects(Value::Null, json!({"assignment": true})),
            json!({"assignment": true})
        );
        assert_eq!(
            merge_json_objects(json!({"source": true}), Value::Bool(false)),
            json!({"source": true})
        );
    }

    #[test]
    fn database_timeout_rejects_negative_values_without_clamping() {
        assert_eq!(timeout_from_database(-1), None);
        assert_eq!(timeout_from_database(0), Some(0));
        assert_eq!(
            timeout_from_database(i32::MAX),
            Some(u64::try_from(i32::MAX).expect("i32::MAX fits u64"))
        );
    }

    #[test]
    fn global_sources_keep_last_candidate_and_preserve_precedence_order() {
        let sources = resolve_global_query_sources([
            query_source("prometheus", "https://prometheus.example/api/v1/query"),
            query_source("zabbix", "https://zabbix.example/old"),
            query_source("zabbix", "https://zabbix.example/new"),
            query_source(
                "victoriametrics",
                "https://victoriametrics.example/api/v1/query",
            ),
        ]);

        assert_eq!(
            sources
                .iter()
                .map(|source| source.source_type.as_str())
                .collect::<Vec<_>>(),
            vec!["victoriametrics", "zabbix", "prometheus"],
            "database weight/id precedence must survive cross-type deduplication"
        );
        assert_eq!(
            sources[1].query_url.as_deref(),
            Some("https://zabbix.example/new")
        );
    }

    #[test]
    fn grouped_batches_preserve_database_order_within_each_group() {
        let first_group = Uuid::from_u128(1);
        let second_group = Uuid::from_u128(2);
        let mut groups = HashMap::new();

        push_grouped(&mut groups, first_group, "first");
        push_grouped(&mut groups, second_group, "other");
        push_grouped(&mut groups, first_group, "second");

        assert_eq!(groups[&first_group], vec!["first", "second"]);
        assert_eq!(groups[&second_group], vec!["other"]);
    }

    #[test]
    fn metric_set_item_overrides_win_while_list_visibility_is_additive() {
        let presentation = resolve_metric_set_item_presentation(
            Some("Set label".to_string()),
            Some("Metric label".to_string()),
            None,
            Some("ms".to_string()),
            false,
            true,
        );

        assert_eq!(presentation.label.as_deref(), Some("Set label"));
        assert_eq!(presentation.unit.as_deref(), Some("ms"));
        assert!(presentation.show_in_list);
    }

    #[tokio::test]
    async fn empty_identifier_batches_skip_database_access() {
        let pool = unreachable_pool();

        assert!(
            fetch_host_overrides_for_hosts(&pool, &[])
                .await
                .expect("empty host batch")
                .is_empty()
        );
        assert!(
            fetch_metric_queries_for_exporters(&pool, &[])
                .await
                .expect("empty exporter batch")
                .is_empty()
        );
        assert!(
            fetch_metric_set_items(&pool, &[])
                .await
                .expect("empty metric-set batch")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn host_override_fetch_classifies_database_outage_as_unavailable() {
        let error = fetch_host_overrides_for_hosts(&unreachable_pool(), &[Uuid::from_u128(1)])
            .await
            .expect_err("unreachable PostgreSQL must fail the batch");

        assert!(matches!(error, AppError::DatabaseUnavailable(_)));
    }
}
