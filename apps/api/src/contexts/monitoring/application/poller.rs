//! Observability list-labels + alerts poller.
//!
//! Each tick enumerates in-scope hosts, batch-queries fleet
//! (non-`$host`-templated) list metrics once per distinct (source, query)
//! pair, demultiplexes each Prometheus result vector by the source's
//! correlation label (default `piping_target_id`, stamped by the SD target
//! labels), and caches the raw promql value per host in Redis so the
//! host-labels read endpoint serves cached labels without external calls.
//! An independent alert phase queries the effective alert sources and caches
//! per-host active alerts the same way.

use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::MonitoringConfig;
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::contexts::inventory::infrastructure::persistence::DiscoveryRepository;
use crate::contexts::secrets::application::SecretStore;
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_error_event, log_event};

use super::super::domain::alerts::{
    HostAlert, alert_provider_for_source_type, correlate_alerts_by_host, parse_alerts_for_provider,
};
use super::super::domain::correlation::{
    build_host_correlation_index, correlation_label_for_source, correlation_match_for_source,
    demux_vector_by_host_label, is_list_capable_query,
};
use super::super::domain::host::{
    DiscoveryHost, MonitoringQuerySourceConfig, descriptor_source_type, is_zabbix_source_type,
    source_type_matches_descriptor,
};
use super::super::domain::source::ResolvedMetricQuerySource;
use super::super::infrastructure::persistence::{self, AlertSourceAssignment, HostScopeIds};
use super::super::infrastructure::providers::{
    fetch_active_alerts, fetch_latest_prometheus_metric, metric_query_http_client,
};
use super::host_cache::{
    CachedHostAlerts, CachedHostLabels, CachedHostMetric, host_alerts_cache_key,
    host_labels_cache_key,
};
use super::read_model::{
    assignment_matches_host, collect_scope_ids, enrich_hosts_with_monitoring,
    fetch_discovery_hosts, scope_precedence,
};
use super::source_resolution::resolve_metric_query_source;

/// Maximum hosts enumerated per poller tick.
const OBSERVABILITY_POLLER_HOST_FETCH_LIMIT: u32 = 5000;

/// Maximum cached metrics retained per host per tick.
const OBSERVABILITY_POLLER_MAX_QUERIES_PER_HOST: usize = 64;

/// Maximum cached alerts retained per host per tick (bounds the Redis payload
/// and the per-host alert list shipped to the browser).
const OBSERVABILITY_POLLER_MAX_ALERTS_PER_HOST: usize = 64;

/// A distinct fleet query to execute once and demux across hosts.
#[derive(Debug, Clone)]
struct DistinctListQuery {
    source_type: String,
    query: String,
}

/// A fleet query paired with the effective source and hosts that selected it.
///
/// The same `(source_type, query)` may occur more than once when different
/// host scopes resolve to different source URLs. Each group is queried once,
/// and its result is correlated only within `host_ids`.
#[derive(Debug, Clone)]
struct ScopedListQuery {
    query: DistinctListQuery,
    source: Option<MonitoringQuerySourceConfig>,
    source_identity: String,
    host_ids: Vec<Uuid>,
}

type CachedMetricsByHost = HashMap<Uuid, BTreeMap<usize, CachedHostMetric>>;

/// Summary of a single poller tick, returned to the worker loop for logging.
#[derive(Debug, Clone, Default)]
pub struct ObservabilityPollSummary {
    pub hosts_scanned: usize,
    pub distinct_queries: usize,
    pub cache_keys_written: usize,
    pub skipped_templated: usize,
    pub skipped_zabbix: usize,
    pub alert_sources_queried: usize,
    pub alert_cache_keys_written: usize,
}

/// Run one observability list-labels poller tick.
///
/// Enumerates in-scope hosts, collects distinct list-capable metric queries,
/// executes each distinct (source, query) once, demultiplexes the result by
/// the source's correlation label, and writes per-host cached values to Redis
/// with a TTL of `3 * interval_seconds`. Never lets one bad source abort the
/// whole tick — per-source errors are logged and skipped.
pub async fn poll_observability_list_metrics(
    db_pool: &PgPool,
    redis: &redis::aio::ConnectionManager,
    discovery: &impl DiscoveryRepository,
    secret_store: &dyn SecretStore,
    monitoring_config: &MonitoringConfig,
) -> Result<ObservabilityPollSummary, AppError> {
    let mut summary = ObservabilityPollSummary::default();

    let filters = DiscoveryHostFilters {
        fetch_limit: OBSERVABILITY_POLLER_HOST_FETCH_LIMIT,
        ..DiscoveryHostFilters::default()
    };
    // Unfiltered fleet read: the poller is trusted infrastructure caching for
    // everyone; per-caller row visibility applies where the cache is READ.
    let raw_hosts = fetch_discovery_hosts(discovery, &filters, None).await?;
    let hosts = enrich_hosts_with_monitoring(db_pool, monitoring_config, raw_hosts).await?;
    summary.hosts_scanned = hosts.len();

    let (scoped_queries, skipped) = collect_scoped_list_queries(&hosts);
    summary.skipped_templated += skipped.templated;
    summary.skipped_zabbix += skipped.zabbix;

    if summary.skipped_templated > 0 {
        log_observability_poller_debug(
            "monitoring.observability.list_poll.skipped_templated",
            summary.skipped_templated,
        );
    }
    if summary.skipped_zabbix > 0 {
        log_observability_poller_debug(
            "monitoring.observability.list_poll.skipped_zabbix",
            summary.skipped_zabbix,
        );
    }

    summary.distinct_queries = scoped_queries.len();
    // No early return when `scoped_queries` is empty: the metrics query loop and the
    // per-host write loop below both no-op on an empty set, but the alert phase
    // (`poll_observability_alerts`) is independent of list metrics and must still
    // run. Returning here would couple alert polling to the presence of at least
    // one non-`$host`, non-zabbix `show_in_list` metric.

    // Execute each distinct query once and accumulate per-host metric values.
    let mut per_host = CachedMetricsByHost::new();
    let mut resolved_sources = HashMap::<String, Option<ResolvedMetricQuerySource>>::new();
    // Index hosts by id so an attributed metric can be enriched with that
    // host's descriptor display fields (per-set `label_override`/`unit_override`
    // make the descriptor host-specific).
    let host_by_id: HashMap<Uuid, &DiscoveryHost> =
        hosts.iter().map(|host| (host.host_id, host)).collect();

    for scoped_query in &scoped_queries {
        let Some((source, prom_json)) = fetch_list_query_metric(
            scoped_query,
            monitoring_config,
            secret_store,
            &mut resolved_sources,
        )
        .await
        else {
            continue;
        };

        let label = correlation_label_for_source(&source.options);
        let match_strategy = correlation_match_for_source(&source.options);
        let scoped_hosts = scoped_query
            .host_ids
            .iter()
            .filter_map(|host_id| host_by_id.get(host_id).copied())
            .cloned()
            .collect::<Vec<_>>();
        let host_index = build_host_correlation_index(&scoped_hosts, &match_strategy);

        cache_correlated_metrics(
            &scoped_query.query,
            demux_vector_by_host_label(&prom_json, &label),
            &host_index,
            &host_by_id,
            &mut per_host,
        );
    }

    // Write per-host cached payloads to Redis with a TTL of 3x the interval.
    let interval_secs = (monitoring_config.list_poll_interval_ms / 1000).max(1);
    // 6× interval keeps the cache key alive across a missed poller tick (e.g.
    // a transiently failing source) so the read endpoint never serves an
    // empty/stale entry while metrics are merely between refreshes.
    let ttl_secs = interval_secs.saturating_mul(6);
    let updated_at = Utc::now().to_rfc3339();
    let mut redis = redis.clone();
    // Serialize every payload first, then write through pipelined `SETEX`
    // batches: one Redis round trip per few hundred keys instead of one per
    // host. At the poller's 5,000-host cap that is ~10 round trips per phase
    // per tick instead of up to 5,000 sequential writes, every tick.
    let mut cache_entries = Vec::with_capacity(per_host.len());
    for (host_id, metrics) in per_host {
        let payload = CachedHostLabels {
            metrics: ordered_cached_metrics(metrics),
            updated_at: updated_at.clone(),
        };
        match serde_json::to_string(&payload) {
            Ok(serialized) => {
                cache_entries.push((host_labels_cache_key(host_id), serialized));
            }
            Err(error) => {
                log_observability_poller_source_error("cache_serialize", &error.to_string());
            }
        }
    }
    summary.cache_keys_written += report_cache_write_results(
        "cache_write",
        crate::storage::cache::redis::cache::set_many(&mut redis, &cache_entries, ttl_secs).await,
    );

    // Independent, resilient alert phase: a failure here never affects the
    // metrics cache write above and never aborts the tick.
    poll_observability_alerts(ObservabilityAlertPoll {
        db_pool,
        secret_store,
        monitoring_config,
        hosts: &hosts,
        redis: &mut redis,
        ttl_secs,
        updated_at: &updated_at,
        summary: &mut summary,
    })
    .await;

    Ok(summary)
}

fn ordered_cached_metrics(metrics: BTreeMap<usize, CachedHostMetric>) -> Vec<CachedHostMetric> {
    metrics
        .into_values()
        .take(OBSERVABILITY_POLLER_MAX_QUERIES_PER_HOST)
        .collect()
}

async fn fetch_list_query_metric(
    scoped_query: &ScopedListQuery,
    monitoring_config: &MonitoringConfig,
    secret_store: &dyn SecretStore,
    resolved_sources: &mut HashMap<String, Option<ResolvedMetricQuerySource>>,
) -> Option<(ResolvedMetricQuerySource, serde_json::Value)> {
    let source = resolve_list_query_source(
        scoped_query,
        monitoring_config,
        secret_store,
        resolved_sources,
    )
    .await?;

    let client = match metric_query_http_client(source.pinned_resolution.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            log_observability_poller_source_error(&scoped_query.query.source_type, &error);
            return None;
        }
    };
    match fetch_latest_prometheus_metric(&client, &source, &scoped_query.query.query).await {
        Ok(value) => Some((source, value)),
        Err(error) => {
            log_observability_poller_source_error(&scoped_query.query.source_type, &error);
            None
        }
    }
}

async fn resolve_list_query_source(
    scoped_query: &ScopedListQuery,
    monitoring_config: &MonitoringConfig,
    secret_store: &dyn SecretStore,
    resolved_sources: &mut HashMap<String, Option<ResolvedMetricQuerySource>>,
) -> Option<ResolvedMetricQuerySource> {
    if let Some(source) = resolved_sources.get(&scoped_query.source_identity) {
        return source.clone();
    }

    let query_sources = scoped_query.source.as_slice();

    let source = match resolve_metric_query_source(
        monitoring_config,
        secret_store,
        query_sources,
        &scoped_query.query.source_type,
    )
    .await
    {
        Ok(source) => source,
        Err(error) => {
            log_observability_poller_source_error(
                &scoped_query.query.source_type,
                &error.to_string(),
            );
            None
        }
    };
    resolved_sources.insert(scoped_query.source_identity.clone(), source.clone());
    source
}

/// Apply the host-attribution rule after a fleet response has been demultiplexed.
///
/// Correlation alone is insufficient: a value is cached only when that host's
/// resolved metric set contains the same list descriptor. Descriptor order is
/// significant. The descriptor source type is part of the match because the
/// same query text may deliberately be attached to multiple backends.
fn cache_correlated_metrics(
    query: &DistinctListQuery,
    demuxed: HashMap<String, serde_json::Value>,
    host_index: &HashMap<String, Uuid>,
    host_by_id: &HashMap<Uuid, &DiscoveryHost>,
    per_host: &mut CachedMetricsByHost,
) {
    for (correlation_key, value) in demuxed {
        let Some(host_id) = host_index.get(&correlation_key) else {
            continue;
        };
        let Some((descriptor_order, descriptor)) = host_by_id.get(host_id).and_then(|host| {
            host.monitoring
                .metrics
                .iter()
                .enumerate()
                .find(|(_, descriptor)| {
                    descriptor.query == query.query
                        && descriptor.show_in_list
                        && descriptor_source_type(descriptor)
                            .eq_ignore_ascii_case(&query.source_type)
                })
        }) else {
            continue;
        };
        let metric = CachedHostMetric {
            query: query.query.clone(),
            value,
            label: descriptor.label.clone(),
            unit: descriptor.unit.clone(),
            value_type: descriptor.value_type.clone(),
            widget_type: descriptor.widget_type.clone(),
            list_render: descriptor.list_render.clone(),
            options: descriptor.options.clone(),
            category: descriptor.category.clone(),
            semantic: descriptor.semantic.clone(),
        };
        per_host
            .entry(*host_id)
            .or_default()
            .insert(descriptor_order, metric);
    }
}

/// Borrowed inputs and output sinks for one observability alert poll phase.
struct ObservabilityAlertPoll<'a> {
    db_pool: &'a PgPool,
    secret_store: &'a dyn SecretStore,
    monitoring_config: &'a MonitoringConfig,
    hosts: &'a [DiscoveryHost],
    redis: &'a mut redis::aio::ConnectionManager,
    ttl_secs: u64,
    updated_at: &'a str,
    summary: &'a mut ObservabilityPollSummary,
}

/// Resolve, query, parse, correlate, and cache active alerts for the in-scope
/// hosts. Each alert source is queried once; per-source failures are logged and
/// skipped so one unreachable source never aborts the tick.
async fn poll_observability_alerts(poll: ObservabilityAlertPoll<'_>) {
    let ObservabilityAlertPoll {
        db_pool,
        secret_store,
        monitoring_config,
        hosts,
        redis,
        ttl_secs,
        updated_at,
        summary,
    } = poll;
    if hosts.is_empty() {
        return;
    }

    let (host_ids, site_ids, tenant_ids, role_ids) = collect_scope_ids(hosts);
    let scopes = HostScopeIds {
        host_ids: &host_ids,
        site_ids: &site_ids,
        tenant_ids: &tenant_ids,
        role_ids: &role_ids,
    };
    let assignments =
        match persistence::fetch_alert_source_assignments_for_hosts(db_pool, scopes).await {
            Ok(assignments) => assignments,
            Err(error) => {
                log_observability_poller_source_error("alert_assignments", &error.to_string());
                return;
            }
        };

    // Resolve the effective alert sources per host, mirroring
    // `effective_query_sources` scope resolution: each source is queried once but
    // its alerts are only attributed to the hosts the source is scoped to.
    let sources = effective_alert_sources(hosts, &assignments);

    let hosts_by_id = hosts
        .iter()
        .map(|host| (host.host_id, host))
        .collect::<HashMap<_, _>>();

    // Accumulate per-host alerts across all sources, then write once per host.
    let expected_sources = alert_source_counts_by_host(&sources);
    let mut completed_sources = HashMap::<Uuid, usize>::new();
    let mut per_host = HashMap::<Uuid, Vec<HostAlert>>::new();

    for scoped in &sources {
        let Some(fetched) =
            fetch_source_alerts(monitoring_config, secret_store, &scoped.source).await
        else {
            continue;
        };
        summary.alert_sources_queried += 1;
        for host_id in &scoped.host_ids {
            *completed_sources.entry(*host_id).or_default() += 1;
            per_host.entry(*host_id).or_default();
        }
        accumulate_scoped_alerts(scoped, &hosts_by_id, fetched, &mut per_host);
    }

    // Same serialize-first, pipelined-batch pattern as the metrics phase: a
    // handful of round trips per tick instead of one `SETEX` per host.
    let complete_alerts =
        complete_alerts_by_host(hosts, &expected_sources, &completed_sources, &mut per_host);
    let mut cache_entries = Vec::with_capacity(complete_alerts.len());
    for (host_id, alerts) in complete_alerts {
        let payload = CachedHostAlerts {
            alerts,
            updated_at: updated_at.to_string(),
        };
        match serde_json::to_string(&payload) {
            Ok(serialized) => {
                cache_entries.push((host_alerts_cache_key(host_id), serialized));
            }
            Err(error) => {
                log_observability_poller_source_error("alert_cache_serialize", &error.to_string());
            }
        }
    }
    summary.alert_cache_keys_written += report_cache_write_results(
        "alert_cache_write",
        crate::storage::cache::redis::cache::set_many(redis, &cache_entries, ttl_secs).await,
    );
}

struct FetchedSourceAlerts {
    match_strategy: String,
    alerts_by_correlation: HashMap<String, Vec<HostAlert>>,
}

async fn fetch_source_alerts(
    monitoring_config: &MonitoringConfig,
    secret_store: &dyn SecretStore,
    source_config: &MonitoringQuerySourceConfig,
) -> Option<FetchedSourceAlerts> {
    let provider = alert_provider_for_source_type(&source_config.source_type)?;
    let resolved = match resolve_metric_query_source(
        monitoring_config,
        secret_store,
        std::slice::from_ref(source_config),
        &source_config.source_type,
    )
    .await
    {
        Ok(Some(resolved)) => resolved,
        Ok(None) => return None,
        Err(error) => {
            log_observability_poller_source_error(&source_config.source_type, &error.to_string());
            return None;
        }
    };
    let client = match metric_query_http_client(resolved.pinned_resolution.as_ref()) {
        Ok(client) => client,
        Err(error) => {
            log_observability_poller_source_error(&source_config.source_type, &error);
            return None;
        }
    };
    let alerts_json = match fetch_active_alerts(&client, &resolved, provider).await {
        Ok(value) => value,
        Err(error) => {
            log_observability_poller_source_error(&source_config.source_type, &error);
            return None;
        }
    };

    let normalized = parse_alerts_for_provider(provider, &alerts_json);
    let label = correlation_label_for_source(&resolved.options);
    Some(FetchedSourceAlerts {
        match_strategy: correlation_match_for_source(&resolved.options),
        alerts_by_correlation: correlate_alerts_by_host(&normalized, &label),
    })
}

/// Attribute fetched alerts only to hosts for which this source is effective.
///
/// Building the correlation index from `scoped.host_ids` is the boundary that
/// prevents a valid correlation label from leaking a site/tenant/host-scoped
/// source's alerts onto an out-of-scope host. The existing per-host cap applies
/// across sources because `per_host` already contains earlier contributions.
fn accumulate_scoped_alerts(
    scoped: &ScopedAlertSource,
    hosts_by_id: &HashMap<Uuid, &DiscoveryHost>,
    fetched: FetchedSourceAlerts,
    per_host: &mut HashMap<Uuid, Vec<HostAlert>>,
) {
    let scoped_hosts = scoped
        .host_ids
        .iter()
        .filter_map(|host_id| hosts_by_id.get(host_id).copied())
        .cloned()
        .collect::<Vec<_>>();
    let host_index = build_host_correlation_index(&scoped_hosts, &fetched.match_strategy);

    for (correlation_key, alerts) in fetched.alerts_by_correlation {
        let Some(host_id) = host_index.get(&correlation_key) else {
            continue;
        };
        let entry = per_host.entry(*host_id).or_default();
        let remaining = OBSERVABILITY_POLLER_MAX_ALERTS_PER_HOST.saturating_sub(entry.len());
        if remaining == 0 {
            continue;
        }
        entry.extend(alerts.into_iter().take(remaining));
    }
}

fn alert_source_counts_by_host(sources: &[ScopedAlertSource]) -> HashMap<Uuid, usize> {
    let mut counts = HashMap::new();
    for source in sources {
        for host_id in &source.host_ids {
            *counts.entry(*host_id).or_default() += 1;
        }
    }
    counts
}

/// Return authoritative alert values only for hosts whose entire effective
/// source set completed. A successful empty source result becomes an empty
/// cache payload (clearing resolved alerts); any failed source keeps the host
/// out of this write batch so "unknown" is not rewritten as "no alerts".
fn complete_alerts_by_host(
    hosts: &[DiscoveryHost],
    expected_sources: &HashMap<Uuid, usize>,
    completed_sources: &HashMap<Uuid, usize>,
    per_host: &mut HashMap<Uuid, Vec<HostAlert>>,
) -> Vec<(Uuid, Vec<HostAlert>)> {
    hosts
        .iter()
        .filter_map(|host| {
            let expected = expected_sources.get(&host.host_id).copied().unwrap_or(0);
            let completed = completed_sources.get(&host.host_id).copied().unwrap_or(0);
            (completed == expected).then(|| {
                (
                    host.host_id,
                    per_host.remove(&host.host_id).unwrap_or_default(),
                )
            })
        })
        .collect()
}

/// A scope-resolved alert source paired with the set of hosts it is the
/// effective source for. The poller queries each source once and only attributes
/// the returned alerts to `host_ids`, so a scoped alert source contributes alerts
/// only to hosts within its scope (global → all hosts; site/tenant/role → hosts
/// in that scope; connection_target/device/vm → that host).
#[derive(Debug, Clone)]
struct ScopedAlertSource {
    source: MonitoringQuerySourceConfig,
    host_ids: Vec<Uuid>,
}

/// Resolve the effective alert sources for a set of hosts and assignments,
/// mirroring `effective_query_sources` exactly: per host, filter assignments by
/// `assignment_matches_host` (honoring `scope_id`), sort by
/// `(scope_precedence, weight, id)`, and keep the most-specific assignment per
/// source type (last-insert-wins). Hosts are then grouped by the resolved source
/// so each distinct source is queried once while alert attribution stays scoped:
/// a global source and a site-scoped source of the same type coexist, and each
/// host gets the alerts of whichever source is effective for it.
fn effective_alert_sources(
    hosts: &[DiscoveryHost],
    assignments: &[AlertSourceAssignment],
) -> Vec<ScopedAlertSource> {
    // Group hosts by the identity of their resolved source so each distinct
    // source is queried once. The identity carries the connection parameters
    // that drive the HTTP call; two assignments that resolve to the same source
    // for different hosts share one query and one correlation pass.
    let mut by_identity: HashMap<String, ScopedAlertSource> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for host in hosts {
        for source in host_effective_alert_sources(host, assignments) {
            let identity = alert_source_identity(&source);
            let entry = by_identity.entry(identity.clone()).or_insert_with(|| {
                order.push(identity.clone());
                ScopedAlertSource {
                    source,
                    host_ids: Vec::new(),
                }
            });
            if !entry.host_ids.contains(&host.host_id) {
                entry.host_ids.push(host.host_id);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|identity| by_identity.remove(&identity))
        .collect()
}

/// Resolve the effective alert source per source type for a single host,
/// mirroring the per-host resolution in `effective_query_sources`.
fn host_effective_alert_sources(
    host: &DiscoveryHost,
    assignments: &[AlertSourceAssignment],
) -> Vec<MonitoringQuerySourceConfig> {
    let mut selected = assignments
        .iter()
        .filter(|assignment| {
            assignment_matches_host(host, &assignment.scope_type, assignment.scope_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by_key(|assignment| {
        (
            scope_precedence(&assignment.scope_type),
            assignment.weight,
            assignment.id,
        )
    });

    let mut by_source_type = HashMap::<String, MonitoringQuerySourceConfig>::new();
    for assignment in selected {
        by_source_type.insert(
            assignment.source.source_type.to_ascii_lowercase(),
            assignment.source,
        );
    }
    let mut sources = by_source_type.into_values().collect::<Vec<_>>();
    sources.sort_by_key(|source| source.source_type.clone());
    sources
}

/// Build a stable identity string for an alert source so two resolved sources
/// that share connection parameters dedup into a single fleet query. Keyed by
/// the source type plus the connection parameters that drive the HTTP request.
fn alert_source_identity(source: &MonitoringQuerySourceConfig) -> String {
    serde_json::json!({
        "source_type": source.source_type.to_ascii_lowercase(),
        "query_url": source.query_url,
        "credential_secret_id": source.credential_secret_id,
        "timeout_ms": source.timeout_ms,
        "options": source.options,
    })
    .to_string()
}

fn log_observability_poller_debug(event: &str, count: usize) {
    log_event(
        LogLevel::Debug,
        event,
        EventFields::default()
            .with_operation("observability_list_poll")
            .with_status("ok")
            .with_context("count", count.to_string()),
    );
}

/// Count the keys written, and log each distinct failure ONCE with how many
/// keys it covered.
///
/// `set_many` pipelines up to `SET_MANY_CHUNK` (500) `SETEX` commands per round
/// trip and, when the round trip itself fails, honestly reports that failure
/// against every key in the chunk. Logging that result per key turns ONE
/// transport failure into up to 500 identical ERROR lines — on a 4k-host fleet,
/// thousands per tick. That is how a single timeout reads as a catastrophe, and
/// on 2026-08-06 that same amplification (then `MISCONF`, from a full disk) is
/// what filled the disk it was complaining about.
///
/// The per-key result contract stays as it is: each of those keys really did
/// fail, and `cache_keys_written` depends on counting them individually. Only
/// the log is collapsed.
/// How many descriptors the distinct-query scan declined to poll, and why.
#[derive(Debug, Default, PartialEq, Eq)]
struct SkippedDescriptors {
    templated: usize,
    zabbix: usize,
}

/// Merge one host into its source-scoped query group. A group keeps the first
/// descriptor encounter position, while repeated descriptors for the same
/// host do not duplicate its correlation work.
fn merge_scoped_list_query(
    group_indexes: &mut HashMap<String, usize>,
    groups: &mut Vec<ScopedListQuery>,
    group_identity: String,
    host_id: Uuid,
    new_group: impl FnOnce() -> ScopedListQuery,
) {
    if let Some(index) = group_indexes.get(&group_identity).copied() {
        if !groups[index].host_ids.contains(&host_id) {
            groups[index].host_ids.push(host_id);
        }
        return;
    }

    group_indexes.insert(group_identity, groups.len());
    groups.push(new_group());
}

/// Source-scoped `(source_type, query)` groups worth polling, plus what was
/// skipped.
///
/// The whole point of the poller is that a query shared by a thousand hosts is
/// executed once per effective source and demultiplexed. The returned `Vec`
/// preserves first descriptor encounter order; each host's final cache order
/// is independently restored from descriptor positions before truncation.
fn collect_scoped_list_queries(
    hosts: &[DiscoveryHost],
) -> (Vec<ScopedListQuery>, SkippedDescriptors) {
    let mut group_indexes = HashMap::<String, usize>::new();
    let mut groups = Vec::<ScopedListQuery>::new();
    let mut skipped = SkippedDescriptors::default();
    for host in hosts {
        for descriptor in host
            .monitoring
            .metrics
            .iter()
            .filter(|descriptor| descriptor.show_in_list)
        {
            if !is_list_capable_query(&descriptor.query) {
                skipped.templated += 1;
                continue;
            }
            let source_type = descriptor_source_type(descriptor).to_ascii_lowercase();
            if is_zabbix_source_type(&source_type) {
                // Zabbix is inherently per-host (no fleet vector); list support
                // arrives in a later phase.
                skipped.zabbix += 1;
                continue;
            }

            let source = host
                .monitoring
                .query_sources
                .iter()
                .find(|source| source_type_matches_descriptor(&source.source_type, &source_type))
                .cloned();
            let source_identity = list_source_identity(&source_type, source.as_ref());
            let group_identity = serde_json::json!({
                "source": source_identity,
                "query": descriptor.query,
            })
            .to_string();

            merge_scoped_list_query(
                &mut group_indexes,
                &mut groups,
                group_identity,
                host.host_id,
                || ScopedListQuery {
                    query: DistinctListQuery {
                        source_type,
                        query: descriptor.query.clone(),
                    },
                    source,
                    source_identity,
                    host_ids: vec![host.host_id],
                },
            );
        }
    }
    (groups, skipped)
}

fn list_source_identity(
    requested_source_type: &str,
    source: Option<&MonitoringQuerySourceConfig>,
) -> String {
    serde_json::json!({
        "requested_source_type": requested_source_type,
        "source": source.map(|source| serde_json::json!({
            "source_type": source.source_type.to_ascii_lowercase(),
            "query_url": source.query_url,
            "credential_secret_id": source.credential_secret_id,
            "timeout_ms": source.timeout_ms,
            "options": source.options,
        })),
    })
    .to_string()
}

fn summarize_cache_write_results(results: Vec<Result<(), String>>) -> (usize, Vec<String>) {
    let mut written = 0usize;
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    for result in results {
        match result {
            Ok(()) => written += 1,
            Err(error) => *failures.entry(error).or_default() += 1,
        }
    }
    let messages = failures
        .into_iter()
        .map(|(error, count)| {
            if count > 1 {
                format!("{error} (affected {count} cache keys in one batch)")
            } else {
                error
            }
        })
        .collect();
    (written, messages)
}

fn report_cache_write_results(source_type: &str, results: Vec<Result<(), String>>) -> usize {
    let (written, messages) = summarize_cache_write_results(results);
    for message in messages {
        log_observability_poller_source_error(source_type, &message);
    }
    written
}

fn log_observability_poller_source_error(source_type: &str, error: &str) {
    log_error_event(
        "monitoring.observability.list_poll.source_failed",
        "observability_list_poll_source_error",
        error.to_string(),
        EventFields::default()
            .with_operation("observability_list_poll")
            .with_status("failed")
            // Each source gets its own repetition budget. Sharing one would let
            // a source failing every tick exhaust the budget and bury the first
            // failure of a source that is usually fine — which is the one worth
            // reading.
            .with_noise_key(source_type)
            .with_context("source_type", source_type.to_string()),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::super::domain::host::{MonitoringMetricDescriptor, sample_host};
    use super::*;

    /// A list-capable descriptor. `sample_host()` carries no metrics, so this
    /// builds the descriptor directly — only the three fields the skip rules
    /// actually read vary.
    fn list_descriptor(
        query: &str,
        source: &str,
        show_in_list: bool,
    ) -> MonitoringMetricDescriptor {
        MonitoringMetricDescriptor {
            name: "m".to_string(),
            label: None,
            query: query.to_string(),
            unit: None,
            value_type: None,
            widget_type: None,
            description: None,
            source_type: Some(source.to_string()),
            query_type: Some("promql".to_string()),
            options: Value::Object(Default::default()),
            show_in_list,
            list_render: "value".to_string(),
            metric_query_id: None,
            category: None,
            semantic: None,
        }
    }

    fn host_with(metrics: Vec<MonitoringMetricDescriptor>) -> DiscoveryHost {
        let mut host = sample_host();
        host.monitoring.metrics = metrics;
        host
    }

    fn query_source(name: &str, source_type: &str, query_url: &str) -> MonitoringQuerySourceConfig {
        MonitoringQuerySourceConfig {
            name: Some(name.to_string()),
            source_type: source_type.to_string(),
            query_url: Some(query_url.to_string()),
            credential_secret_id: None,
            timeout_ms: Some(5000),
            options: Value::Object(Default::default()),
        }
    }

    #[test]
    fn a_query_shared_by_many_hosts_is_polled_once() {
        // This is the poller's whole reason to exist: one fleet-wide query
        // instead of one per host. If dedup regressed, the fleet would be
        // hammered N times over and nothing else here would notice.
        let hosts = vec![
            host_with(vec![list_descriptor(
                "up{job=\"node\"}",
                "prometheus",
                true,
            )]),
            host_with(vec![list_descriptor(
                "up{job=\"node\"}",
                "prometheus",
                true,
            )]),
            host_with(vec![list_descriptor(
                "up{job=\"node\"}",
                "PROMETHEUS",
                true,
            )]),
        ];

        let (distinct, skipped) = collect_scoped_list_queries(&hosts);

        assert_eq!(
            distinct.len(),
            1,
            "same query and source — including a differently-cased source — is one poll"
        );
        assert_eq!(skipped, SkippedDescriptors::default());
    }

    #[test]
    fn scoped_query_merge_deduplicates_hosts_without_reordering_groups() {
        let first_host = Uuid::from_u128(1);
        let second_host = Uuid::from_u128(2);
        let mut group_indexes = HashMap::new();
        let mut groups = Vec::new();
        let group = |query: &str, host_id| ScopedListQuery {
            query: DistinctListQuery {
                source_type: "prometheus".to_string(),
                query: query.to_string(),
            },
            source: None,
            source_identity: "global-prometheus".to_string(),
            host_ids: vec![host_id],
        };

        merge_scoped_list_query(
            &mut group_indexes,
            &mut groups,
            "first".to_string(),
            first_host,
            || group("up", first_host),
        );
        merge_scoped_list_query(
            &mut group_indexes,
            &mut groups,
            "second".to_string(),
            second_host,
            || group("load", second_host),
        );
        merge_scoped_list_query(
            &mut group_indexes,
            &mut groups,
            "first".to_string(),
            second_host,
            || group("up", second_host),
        );
        merge_scoped_list_query(
            &mut group_indexes,
            &mut groups,
            "first".to_string(),
            second_host,
            || group("up", second_host),
        );

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].query.query, "up");
        assert_eq!(groups[1].query.query, "load");
        assert_eq!(groups[0].host_ids, vec![first_host, second_host]);
    }

    #[test]
    fn per_host_and_non_list_descriptors_are_skipped_for_their_own_reasons() {
        // Three different "not polled" cases that must stay distinguishable:
        // not requested for the list at all, templated per host, and zabbix.
        let hosts = vec![host_with(vec![
            list_descriptor("up{job=\"node\"}", "prometheus", false),
            list_descriptor("cpu{host=\"$host\"}", "prometheus", true),
            list_descriptor("agent.ping", "zabbix", true),
            list_descriptor("up{job=\"node\"}", "prometheus", true),
        ])];

        let (distinct, skipped) = collect_scoped_list_queries(&hosts);

        assert_eq!(distinct.len(), 1, "only the plain fleet query is polled");
        assert_eq!(
            skipped,
            SkippedDescriptors {
                templated: 1,
                zabbix: 1
            },
            "a descriptor not marked show_in_list is not a skip — it was never a candidate"
        );
    }

    #[test]
    fn correlated_metric_uses_the_first_matching_list_descriptor() {
        let host_id = Uuid::from_u128(1);
        let mut hidden = list_descriptor("up", "prometheus", false);
        hidden.label = Some("not selected".to_string());
        let mut selected = list_descriptor("up", "prometheus", true);
        selected.label = Some("selected".to_string());
        let mut host = host_with(vec![hidden, selected]);
        host.host_id = host_id;

        let query = DistinctListQuery {
            source_type: "prometheus".to_string(),
            query: "up".to_string(),
        };
        let host_index = HashMap::from([("correlated-host".to_string(), host_id)]);
        let host_by_id = HashMap::from([(host_id, &host)]);
        let mut per_host = HashMap::new();

        cache_correlated_metrics(
            &query,
            HashMap::from([("correlated-host".to_string(), serde_json::json!(1))]),
            &host_index,
            &host_by_id,
            &mut per_host,
        );

        let metrics = per_host.get(&host_id).expect("correlated host cache entry");
        assert_eq!(metrics.len(), 1);
        assert_eq!(
            metrics
                .values()
                .next()
                .and_then(|metric| metric.label.as_deref()),
            Some("selected"),
            "a non-list descriptor must not supply display overrides"
        );
    }

    #[test]
    fn correlated_metrics_match_the_descriptor_source_type() {
        let host_id = Uuid::from_u128(1);
        let mut prometheus = list_descriptor("node_load1", "prometheus", true);
        prometheus.label = Some("central".to_string());
        let mut victoriametrics = list_descriptor("node_load1", "victoriametrics", true);
        victoriametrics.label = Some("site-local".to_string());
        let mut host = host_with(vec![prometheus, victoriametrics]);
        host.host_id = host_id;

        let host_index = HashMap::from([("correlated-host".to_string(), host_id)]);
        let host_by_id = HashMap::from([(host_id, &host)]);
        let mut per_host = CachedMetricsByHost::new();

        for (source_type, value) in [("victoriametrics", 2), ("prometheus", 1)] {
            cache_correlated_metrics(
                &DistinctListQuery {
                    source_type: source_type.to_string(),
                    query: "node_load1".to_string(),
                },
                HashMap::from([("correlated-host".to_string(), serde_json::json!(value))]),
                &host_index,
                &host_by_id,
                &mut per_host,
            );
        }

        let metrics = ordered_cached_metrics(
            per_host
                .remove(&host_id)
                .expect("both source-specific metrics are cached"),
        );
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].label.as_deref(), Some("central"));
        assert_eq!(metrics[0].value, serde_json::json!(1));
        assert_eq!(metrics[1].label.as_deref(), Some("site-local"));
        assert_eq!(metrics[1].value, serde_json::json!(2));
    }

    #[test]
    fn cached_metrics_follow_descriptor_order_before_the_cap() {
        let host_id = Uuid::from_u128(1);
        let descriptors = (0..=OBSERVABILITY_POLLER_MAX_QUERIES_PER_HOST)
            .map(|index| {
                let mut descriptor =
                    list_descriptor(&format!("metric_{index:02}"), "prometheus", true);
                descriptor.label = Some(format!("Metric {index:02}"));
                descriptor
            })
            .collect::<Vec<_>>();
        let mut host = host_with(descriptors);
        host.host_id = host_id;
        let host_index = HashMap::from([("correlated-host".to_string(), host_id)]);
        let host_by_id = HashMap::from([(host_id, &host)]);
        let mut per_host = CachedMetricsByHost::new();

        for index in (0..=OBSERVABILITY_POLLER_MAX_QUERIES_PER_HOST).rev() {
            cache_correlated_metrics(
                &DistinctListQuery {
                    source_type: "prometheus".to_string(),
                    query: format!("metric_{index:02}"),
                },
                HashMap::from([("correlated-host".to_string(), serde_json::json!(index))]),
                &host_index,
                &host_by_id,
                &mut per_host,
            );
        }

        let metrics = ordered_cached_metrics(
            per_host
                .remove(&host_id)
                .expect("host metrics were correlated"),
        );
        assert_eq!(metrics.len(), OBSERVABILITY_POLLER_MAX_QUERIES_PER_HOST);
        assert_eq!(
            metrics.first().and_then(|metric| metric.label.as_deref()),
            Some("Metric 00")
        );
        assert_eq!(
            metrics.last().and_then(|metric| metric.label.as_deref()),
            Some("Metric 63")
        );
        assert!(
            metrics
                .iter()
                .all(|metric| metric.label.as_deref() != Some("Metric 64")),
            "the cap must drop the last configured descriptor, not whichever query ran last"
        );
    }

    #[test]
    fn list_queries_are_grouped_by_each_hosts_effective_source() {
        let mut first = host_with(vec![list_descriptor("up", "victoriametrics", true)]);
        first.host_id = Uuid::from_u128(1);
        first.monitoring.query_sources = vec![query_source(
            "site-a",
            "victoriametrics",
            "http://site-a.example/api/v1/query",
        )];
        let mut second = host_with(vec![list_descriptor("up", "victoriametrics", true)]);
        second.host_id = Uuid::from_u128(2);
        second.monitoring.query_sources = vec![query_source(
            "site-b",
            "victoriametrics",
            "http://site-b.example/api/v1/query",
        )];

        let (groups, skipped) = collect_scoped_list_queries(&[first, second]);

        assert_eq!(skipped, SkippedDescriptors::default());
        assert_eq!(
            groups.len(),
            2,
            "each scoped backend must receive its own fleet query"
        );
        assert_eq!(groups[0].host_ids, vec![Uuid::from_u128(1)]);
        assert_eq!(groups[1].host_ids, vec![Uuid::from_u128(2)]);
        assert_eq!(
            groups[0]
                .source
                .as_ref()
                .and_then(|source| source.query_url.as_deref()),
            Some("http://site-a.example/api/v1/query")
        );
        assert_eq!(
            groups[1]
                .source
                .as_ref()
                .and_then(|source| source.query_url.as_deref()),
            Some("http://site-b.example/api/v1/query")
        );
    }

    #[test]
    fn one_failed_batch_is_one_log_line_not_five_hundred() {
        // `set_many` pipelines 500 SETEX per round trip and, when that round
        // trip fails, reports the failure against every key in the chunk —
        // truthfully, since none of them was written. Logged per key that is
        // 500 identical ERROR lines from ONE timeout.
        let results = (0..500)
            .map(|_| Err("timed out".to_string()))
            .collect::<Vec<_>>();

        let (written, messages) = summarize_cache_write_results(results);

        assert_eq!(written, 0);
        assert_eq!(
            messages.len(),
            1,
            "one transport failure must produce one log line, whatever the batch size"
        );
        assert!(
            messages[0].contains("500"),
            "collapsing must not hide the scale: the line has to say how many keys were lost, \
             got {:?}",
            messages[0]
        );
    }

    #[test]
    fn distinct_failures_stay_distinct_and_successes_are_counted() {
        // Collapsing is by message, not blanket: two different faults in one
        // batch are two different problems and must both be visible.
        let results = vec![
            Ok(()),
            Err("timed out".to_string()),
            Ok(()),
            Err("MISCONF".to_string()),
            Err("timed out".to_string()),
        ];

        let (written, messages) = summarize_cache_write_results(results);

        assert_eq!(written, 2, "successful keys are still counted individually");
        assert_eq!(messages.len(), 2, "two distinct faults, two lines");
        assert!(
            messages
                .iter()
                .any(|m| m.starts_with("MISCONF") && !m.contains("affected")),
            "a fault that hit one key must not be dressed up with a count: {messages:?}"
        );
    }

    #[test]
    fn effective_alert_sources_scopes_per_host() {
        // A global alert source and a site-scoped alert source of the same type
        // (different URLs) must coexist: the site host resolves to the site
        // source, and an out-of-site host falls back to the global source.
        // This mirrors the per-host `effective_query_sources` resolution and
        // proves a scoped alert source never contributes to out-of-scope hosts.
        let site_id = Uuid::from_u128(0x5174);
        let site_host_id = Uuid::from_u128(1);
        let other_host_id = Uuid::from_u128(2);

        let mut site_host = sample_host();
        site_host.host_id = site_host_id;
        site_host.site_id = Some(site_id);
        let mut other_host = sample_host();
        other_host.host_id = other_host_id;
        other_host.site_id = None;
        let hosts = vec![site_host, other_host];

        let global_source = MonitoringQuerySourceConfig {
            name: Some("global-am".to_string()),
            source_type: "alertmanager".to_string(),
            query_url: Some("http://10.0.0.1:9093".to_string()),
            credential_secret_id: None,
            timeout_ms: Some(5000),
            options: Value::Object(Default::default()),
        };
        let site_source = MonitoringQuerySourceConfig {
            name: Some("site-am".to_string()),
            source_type: "alertmanager".to_string(),
            query_url: Some("http://10.0.0.2:9093".to_string()),
            credential_secret_id: None,
            timeout_ms: Some(5000),
            options: Value::Object(Default::default()),
        };
        let assignments = vec![
            AlertSourceAssignment {
                id: Uuid::from_u128(10),
                source: global_source,
                scope_type: "global".to_string(),
                scope_id: None,
                weight: 0,
            },
            AlertSourceAssignment {
                id: Uuid::from_u128(20),
                source: site_source,
                scope_type: "site".to_string(),
                scope_id: Some(site_id),
                weight: 0,
            },
        ];

        let resolved = effective_alert_sources(&hosts, &assignments);

        // Two distinct sources are returned (global + site), each scoped to its
        // own host population.
        assert_eq!(resolved.len(), 2);
        let site_scoped = resolved
            .iter()
            .find(|scoped| scoped.source.name.as_deref() == Some("site-am"))
            .expect("site source resolved");
        assert_eq!(site_scoped.host_ids, vec![site_host_id]);

        let global_scoped = resolved
            .iter()
            .find(|scoped| scoped.source.name.as_deref() == Some("global-am"))
            .expect("global source resolved");
        // The site host is covered by the more-specific site source, so the
        // global source only contributes to the out-of-site host.
        assert_eq!(global_scoped.host_ids, vec![other_host_id]);
    }

    #[test]
    fn alert_correlation_cannot_cross_the_source_scope() {
        let scoped_host_id = Uuid::from_u128(1);
        let outside_host_id = Uuid::from_u128(2);
        let mut scoped_host = sample_host();
        scoped_host.host_id = scoped_host_id;
        let mut outside_host = sample_host();
        outside_host.host_id = outside_host_id;
        let hosts = [scoped_host, outside_host];
        let hosts_by_id = hosts
            .iter()
            .map(|host| (host.host_id, host))
            .collect::<HashMap<_, _>>();
        let scoped = ScopedAlertSource {
            source: MonitoringQuerySourceConfig::default(),
            host_ids: vec![scoped_host_id],
        };
        let alert = |name: &str| HostAlert {
            severity: "warning".to_string(),
            name: name.to_string(),
            summary: None,
            since: None,
        };
        let fetched = FetchedSourceAlerts {
            match_strategy: "host_id".to_string(),
            alerts_by_correlation: HashMap::from([
                (scoped_host_id.to_string(), vec![alert("in scope")]),
                (outside_host_id.to_string(), vec![alert("outside scope")]),
            ]),
        };
        let mut per_host = HashMap::new();

        accumulate_scoped_alerts(&scoped, &hosts_by_id, fetched, &mut per_host);

        let scoped_alerts = per_host
            .get(&scoped_host_id)
            .expect("in-scope alert is attributed");
        assert_eq!(scoped_alerts.len(), 1);
        assert_eq!(scoped_alerts[0].name, "in scope");
        assert!(
            !per_host.contains_key(&outside_host_id),
            "a valid host correlation key cannot override the source's scope"
        );
    }

    #[test]
    fn cleared_alerts_are_written_only_after_complete_source_results() {
        let cleared_host_id = Uuid::from_u128(1);
        let unknown_host_id = Uuid::from_u128(2);
        let unconfigured_host_id = Uuid::from_u128(3);
        let hosts = [
            host_with_id(cleared_host_id),
            host_with_id(unknown_host_id),
            host_with_id(unconfigured_host_id),
        ];
        let expected_sources = HashMap::from([(cleared_host_id, 1), (unknown_host_id, 1)]);
        let completed_sources = HashMap::from([(cleared_host_id, 1)]);
        let mut per_host = HashMap::from([(cleared_host_id, Vec::new())]);

        let complete =
            complete_alerts_by_host(&hosts, &expected_sources, &completed_sources, &mut per_host);
        let complete = complete.into_iter().collect::<HashMap<_, _>>();

        assert!(
            complete.get(&cleared_host_id).is_some_and(Vec::is_empty),
            "an authoritative empty response must overwrite a previously active alert"
        );
        assert!(
            !complete.contains_key(&unknown_host_id),
            "a failed source must stay distinguishable from an empty source"
        );
        assert!(
            complete
                .get(&unconfigured_host_id)
                .is_some_and(Vec::is_empty),
            "removing the last source must also clear its former cache entry"
        );
    }

    fn host_with_id(host_id: Uuid) -> DiscoveryHost {
        let mut host = sample_host();
        host.host_id = host_id;
        host
    }
}
