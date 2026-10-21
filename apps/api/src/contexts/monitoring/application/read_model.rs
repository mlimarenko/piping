//! Host read-model assembly: fetch discovered hosts and enrich them with
//! their effective (scope-resolved) exporter scrapes, metric descriptors, and
//! query sources. Consumed by the service-discovery render path, per-target
//! config lookups, and the observability poller.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::MonitoringConfig;
use crate::contexts::access_control::domain::VisibilityPredicate;
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::contexts::inventory::infrastructure::persistence::DiscoveryRepository;
use crate::errors::AppError;

use super::super::domain::host::{
    DiscoveryHost, HostMonitoringConfig, MonitoringMetricDescriptor, MonitoringQuerySourceConfig,
    MonitoringScrapeConfig, metric_descriptor_has_query_source,
};
use super::super::infrastructure::persistence::{
    self, ExporterAssignment, HostOverrideResolved, HostScopeIds, MetricQueryRecord,
    MetricSetAssignment, MetricSetItem, QuerySourceAssignment,
};

pub async fn fetch_discovery_hosts(
    discovery: &impl DiscoveryRepository,
    filters: &DiscoveryHostFilters,
    visibility: Option<&VisibilityPredicate>,
) -> Result<Vec<DiscoveryHost>, AppError> {
    let rows = discovery.list_discovery_hosts(filters, visibility).await?;
    Ok(rows
        .into_iter()
        .map(|row| DiscoveryHost {
            host_id: row.host_id,
            host_name: row.host_name,
            host_address: row.host_address,
            kind: row.kind,
            site_id: row.site_id,
            site_name: row.site_name,
            site_slug: row.site_slug,
            tenant_id: row.tenant_id,
            tenant_name: row.tenant_name,
            tenant_slug: row.tenant_slug,
            role_id: row.role_id,
            role_name: row.role_name,
            role_slug: row.role_slug,
            status: row.status,
            platform_name: row.platform_name,
            model: row.model,
            manufacturer: row.manufacturer,
            serial: row.serial,
            asset_tag: row.asset_tag,
            primary_ip4: row.primary_ip4,
            primary_ip6: row.primary_ip6,
            interfaces: row.interfaces,
            interface_addresses: row.interface_addresses,
            tunnel_addresses: row.tunnel_addresses,
            contacts: row.contacts,
            description: row.description,
            comments: row.comments,
            site_facility: row.site_facility,
            site_address: row.site_address,
            site_region: row.site_region,
            site_group: row.site_group,
            tenant_group: row.tenant_group,
            rack: row.rack,
            cluster: row.cluster,
            monitoring: HostMonitoringConfig::default(),
        })
        .collect())
}

/// Descriptors paired with the metric query they came from.
///
/// The id is what makes deduplication and override application identity-based
/// rather than text-based: two queries can render the same PromQL and still be
/// different rows an operator configured separately.
type IdentifiedMetrics = Vec<(Option<Uuid>, MonitoringMetricDescriptor)>;

/// Everything the per-host assembly needs, fetched once for the whole fleet.
///
/// Exists so the assembly below can be a plain function of its inputs. The
/// precedence rules it applies — metric set over exporter default, host
/// override over both — are the part worth testing, and they were unreachable
/// from a test while they lived inside a function that opens a database.
struct HostEnrichmentInputs<'a> {
    effective_exporters: HashMap<Uuid, Vec<ExporterAssignment>>,
    metric_queries: HashMap<Uuid, Vec<MetricQueryRecord>>,
    query_source_assignments: Vec<QuerySourceAssignment>,
    metric_set_assignments: Vec<MetricSetAssignment>,
    metric_set_items: HashMap<Uuid, Vec<MetricSetItem>>,
    default_list_items: Vec<MetricSetItem>,
    host_overrides: &'a HashMap<(Uuid, Uuid), HostOverrideResolved>,
}

pub async fn enrich_hosts_with_monitoring(
    db_pool: &PgPool,
    monitoring_config: &MonitoringConfig,
    hosts: Vec<DiscoveryHost>,
) -> Result<Vec<DiscoveryHost>, AppError> {
    if hosts.is_empty() {
        return Ok(hosts);
    }

    let (host_ids, site_ids, tenant_ids, role_ids) = collect_scope_ids(&hosts);
    let scopes = HostScopeIds {
        host_ids: &host_ids,
        site_ids: &site_ids,
        tenant_ids: &tenant_ids,
        role_ids: &role_ids,
    };
    let exporter_assignments =
        persistence::fetch_exporter_assignments_for_hosts(db_pool, scopes).await?;
    let effective_exporters = effective_exporters_by_host(&hosts, &exporter_assignments);
    let exporter_ids = effective_exporters
        .values()
        .flat_map(|assignments| assignments.iter().map(|assignment| assignment.exporter_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let metric_queries =
        persistence::fetch_metric_queries_for_exporters(db_pool, &exporter_ids).await?;
    let query_source_assignments =
        persistence::fetch_query_source_assignments_for_hosts(db_pool, scopes).await?;
    let metric_set_assignments =
        persistence::fetch_metric_set_assignments_for_hosts(db_pool, scopes).await?;
    let metric_set_ids = metric_set_assignments
        .iter()
        .map(|assignment| assignment.metric_set_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let metric_set_items = persistence::fetch_metric_set_items(db_pool, &metric_set_ids).await?;

    // Default-set fallback: hosts with no explicit metric set get every
    // enabled `default_show_in_list` query in addition to the exporter-derived
    // ones. Fetched once per request (fleet-wide), gated by a single config
    // knob so an operator can opt out, and applied additively per host below.
    let default_list_items = if monitoring_config.default_set_fallback_enabled {
        persistence::fetch_default_list_descriptors(db_pool).await?
    } else {
        Vec::new()
    };

    let host_overrides = persistence::fetch_host_overrides_for_hosts(db_pool, &host_ids).await?;
    let inputs = HostEnrichmentInputs {
        effective_exporters,
        metric_queries,
        query_source_assignments,
        metric_set_assignments,
        metric_set_items,
        default_list_items,
        host_overrides: &host_overrides,
    };

    Ok(hosts
        .into_iter()
        .map(|host| enrich_one_host(host, &inputs))
        .collect())
}

/// Metrics this host's exporters contribute, as `(all, inspector)`.
fn exporter_derived_metrics(
    host_id: Uuid,
    inputs: &HostEnrichmentInputs<'_>,
) -> (IdentifiedMetrics, IdentifiedMetrics) {
    let mut all = Vec::new();
    let mut inspector = Vec::new();
    let Some(exporters) = inputs.effective_exporters.get(&host_id) else {
        return (all, inspector);
    };
    for assignment in exporters {
        let Some(items) = inputs.metric_queries.get(&assignment.exporter_id) else {
            continue;
        };
        for item in items {
            all.push((Some(item.metric_query_id), item.descriptor.clone()));
            if item.show_in_inspector {
                inspector.push((Some(item.metric_query_id), item.descriptor.clone()));
            }
        }
    }
    (all, inspector)
}

/// Apply the host's explicit metric sets, or the default-list fallback.
///
/// These are alternatives, not layers: an explicit set is an operator decision
/// and replaces the inspector list outright, while the fallback only adds to
/// what the exporters already produced.
fn apply_metric_sets_or_exporter_defaults(
    host: &DiscoveryHost,
    inputs: &HostEnrichmentInputs<'_>,
    query_sources: &[MonitoringQuerySourceConfig],
    all_metrics: &mut IdentifiedMetrics,
    inspector_metrics: &mut IdentifiedMetrics,
) {
    let matching = effective_metric_set_assignments(host, &inputs.metric_set_assignments);
    if matching.is_empty() {
        // Additive fallback to every `default_show_in_list = true` query. Dedup
        // by id so an exporter-derived descriptor does not shadow a
        // default-list item with the same query text but a different identity.
        for item in &inputs.default_list_items {
            if !metric_descriptor_has_query_source(&item.descriptor, query_sources) {
                continue;
            }
            if !all_metrics
                .iter()
                .any(|(id, _)| *id == Some(item.metric_query_id))
            {
                all_metrics.push((Some(item.metric_query_id), item.descriptor.clone()));
            }
        }
        return;
    }

    inspector_metrics.clear();
    for assignment in matching {
        let Some(items) = inputs.metric_set_items.get(&assignment.metric_set_id) else {
            continue;
        };
        for item in items {
            if item.show_in_inspector {
                inspector_metrics.push((Some(item.metric_query_id), item.descriptor.clone()));
            }
            // A metric set is an explicit operator choice. Its per-item display
            // flags must override exporter defaults for the same metric query
            // instead of being dropped by identity deduplication.
            upsert_metric_descriptor(all_metrics, item.metric_query_id, item.descriptor.clone());
        }
    }
}

fn enrich_one_host(mut host: DiscoveryHost, inputs: &HostEnrichmentInputs<'_>) -> DiscoveryHost {
    let query_sources = effective_query_sources(&host, &inputs.query_source_assignments);
    let scrapes = inputs
        .effective_exporters
        .get(&host.host_id)
        .map(|exporters| {
            exporters
                .iter()
                .filter_map(exporter_assignment_to_scrape)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let (mut all_metrics, mut inspector_metrics) = exporter_derived_metrics(host.host_id, inputs);
    apply_metric_sets_or_exporter_defaults(
        &host,
        inputs,
        &query_sources,
        &mut all_metrics,
        &mut inspector_metrics,
    );

    // Single application point for per-host overrides. Precedence:
    // host override > metric_set_item override (already applied by
    // `fetch_metric_set_items` via COALESCE) > metric_query default.
    apply_host_overrides(inputs.host_overrides, host.host_id, &mut all_metrics);
    apply_host_overrides(inputs.host_overrides, host.host_id, &mut inspector_metrics);

    let mut all_metrics: Vec<MonitoringMetricDescriptor> = all_metrics
        .into_iter()
        .map(|(_, descriptor)| descriptor)
        .collect();
    let mut inspector_metrics: Vec<MonitoringMetricDescriptor> = inspector_metrics
        .into_iter()
        .map(|(_, descriptor)| descriptor)
        .collect();

    deduplicate_metrics(&mut all_metrics);
    deduplicate_metrics(&mut inspector_metrics);
    let applied_exporters = applied_exporter_ids(host.host_id, inputs);
    let query_exporter = query_exporter_index(inputs);
    normalize_canonical_availability(&mut all_metrics, &query_exporter, &applied_exporters);
    normalize_canonical_availability(&mut inspector_metrics, &query_exporter, &applied_exporters);

    host.monitoring = HostMonitoringConfig {
        scrapes,
        metrics: all_metrics,
        inspector_metrics,
        query_sources,
    };
    host
}

fn upsert_metric_descriptor(
    metrics: &mut Vec<(Option<Uuid>, MonitoringMetricDescriptor)>,
    metric_query_id: Uuid,
    descriptor: MonitoringMetricDescriptor,
) {
    if let Some((_, existing)) = metrics
        .iter_mut()
        .find(|(id, _)| *id == Some(metric_query_id))
    {
        *existing = descriptor;
        return;
    }

    metrics.push((Some(metric_query_id), descriptor));
}

/// Apply per-host overrides (label/unit/options.display) onto the resolved
/// descriptors of a single host.
///
/// Precedence rule (single source of truth):
///   host override > metric_set_item override > metric_query default
///
/// `metric_set_item` overrides are already folded into the descriptor by
/// `fetch_metric_set_items` (via `COALESCE`), so this step only needs to
/// layer the host override on top.
///
/// `options_override` replaces the `display` subtree of the descriptor's
/// `options` JSON wholesale — a missing field there means "inherit from the
/// descriptor", not "deep-merge with the descriptor's display tree". This
/// matches how the front-end editor surfaces the rule set as one atomic
/// block.
fn apply_host_overrides(
    overrides: &HashMap<(Uuid, Uuid), HostOverrideResolved>,
    host_id: Uuid,
    descriptors: &mut [(Option<Uuid>, MonitoringMetricDescriptor)],
) {
    if overrides.is_empty() {
        return;
    }
    for (metric_query_id, descriptor) in descriptors.iter_mut() {
        let Some(metric_query_id) = *metric_query_id else {
            continue;
        };
        let Some(override_row) = overrides.get(&(host_id, metric_query_id)) else {
            continue;
        };
        if let Some(label) = override_row.label_override.clone() {
            descriptor.label = Some(label);
        }
        if let Some(unit) = override_row.unit_override.clone() {
            descriptor.unit = Some(unit);
        }
        if let Some(display_override) = override_row.options_override.as_ref() {
            // The `options_override` JSON IS the new `display` subtree.
            // Replace the `display` key on the descriptor's options object;
            // reset the options to an empty object if the descriptor's
            // options aren't already an object so the override always lands
            // at `options.display`.
            if !matches!(descriptor.options, Value::Object(_)) {
                descriptor.options = Value::Object(serde_json::Map::new());
            }
            if let Value::Object(map) = &mut descriptor.options {
                map.insert("display".to_string(), display_override.clone());
            }
        }
    }
}

fn exporter_assignment_to_scrape(
    assignment: &ExporterAssignment,
) -> Option<MonitoringScrapeConfig> {
    let mut labels = assignment.exporter_labels.clone();
    labels.extend(assignment.assignment_labels.clone());
    let port = assignment.port.or(assignment.default_port);
    let port = port.and_then(|value| u16::try_from(value).ok());
    Some(MonitoringScrapeConfig {
        name: assignment
            .assignment_name
            .clone()
            .or_else(|| Some(assignment.exporter_name.clone())),
        enabled: Some(true),
        target: assignment.target.clone(),
        target_source: Some(assignment.default_target_source.clone()),
        job: assignment
            .job
            .clone()
            .or_else(|| Some(assignment.default_job.clone())),
        scheme: assignment
            .scheme
            .clone()
            .or_else(|| Some(assignment.default_scheme.clone())),
        metrics_path: assignment
            .metrics_path
            .clone()
            .or_else(|| Some(assignment.default_metrics_path.clone())),
        port,
        labels,
    })
}

fn deduplicate_metrics(metrics: &mut Vec<MonitoringMetricDescriptor>) {
    // Dedup by `metric_query_id` when present so two enabled queries with the
    // same `query` text but different identities both survive — operators that
    // attach the same query under two ids see both, and per-host overrides
    // (keyed by id) still apply to the surviving descriptor. Synthetic
    // descriptors (no id) fall back to the historical query-text dedup.
    let mut seen_ids = HashSet::<Uuid>::new();
    let mut seen_queries = HashSet::<String>::new();
    metrics.retain(|metric| match metric.metric_query_id {
        Some(id) => seen_ids.insert(id),
        None => seen_queries.insert(metric.query.clone()),
    });
}

fn normalize_canonical_availability(
    metrics: &mut Vec<MonitoringMetricDescriptor>,
    query_exporter: &HashMap<Uuid, Uuid>,
    applied_exporters: &HashSet<Uuid>,
) {
    // Availability is a property of a scraped exporter, not of the metric
    // catalog: every preset query is seeded globally, so without this filter a
    // fresh install would pick the blackbox `probe_success` descriptor
    // (priority 0) even though no blackbox exporter is applied, and render every
    // host "Unknown". A descriptor whose owning exporter is absent from
    // `applied_exporters` is ineligible; a synthetic descriptor (no
    // `metric_query_id`) or an exporter-less query is treated as eligible
    // because no scrape assignment can gate it.
    let is_applied = |metric: &MonitoringMetricDescriptor| {
        metric
            .metric_query_id
            .and_then(|id| query_exporter.get(&id).copied())
            .is_none_or(|exporter_id| applied_exporters.contains(&exporter_id))
    };

    let selected_identity = metrics
        .iter()
        .filter(|metric| metric.semantic.as_deref() == Some("availability") && is_applied(metric))
        .min_by_key(|metric| availability_metric_priority(metric))
        .map(metric_identity);

    metrics.retain(|metric| {
        metric.semantic.as_deref() != Some("availability")
            || selected_identity
                .as_ref()
                .is_some_and(|identity| *identity == metric_identity(metric))
    });
}

/// Map every metric query id that can reach a host's descriptor list to the
/// exporter that owns it: the applied exporters' queries, the assigned metric
/// sets' items, and the fleet-wide default-list fallback. A query without an
/// exporter (for example the Zabbix bundle) is left unmapped.
fn query_exporter_index(inputs: &HostEnrichmentInputs<'_>) -> HashMap<Uuid, Uuid> {
    let mut index = HashMap::new();
    for (exporter_id, records) in &inputs.metric_queries {
        for record in records {
            index.insert(record.metric_query_id, *exporter_id);
        }
    }
    for items in inputs.metric_set_items.values() {
        for item in items {
            if let Some(exporter_id) = item.exporter_id {
                index.insert(item.metric_query_id, exporter_id);
            }
        }
    }
    for item in &inputs.default_list_items {
        if let Some(exporter_id) = item.exporter_id {
            index.insert(item.metric_query_id, exporter_id);
        }
    }
    index
}

/// The exporters actually applied (assigned and enabled) to a host.
fn applied_exporter_ids(host_id: Uuid, inputs: &HostEnrichmentInputs<'_>) -> HashSet<Uuid> {
    inputs
        .effective_exporters
        .get(&host_id)
        .map(|assignments| {
            assignments
                .iter()
                .map(|assignment| assignment.exporter_id)
                .collect()
        })
        .unwrap_or_default()
}

fn metric_identity(metric: &MonitoringMetricDescriptor) -> (Option<Uuid>, String) {
    (metric.metric_query_id, metric.query.clone())
}

fn availability_metric_priority(metric: &MonitoringMetricDescriptor) -> u8 {
    let query = metric.query.to_ascii_lowercase();
    if query.contains("probe_success")
        || query.contains("ping_loss_ratio")
        || query.contains("job=\"blackbox\"")
        || query.contains("job='blackbox'")
    {
        return 0;
    }
    if query == "up"
        || query.starts_with("up{")
        || query.contains("job=\"node\"")
        || query.contains("job='node'")
    {
        return 20;
    }
    10
}

fn effective_exporters_by_host(
    hosts: &[DiscoveryHost],
    assignments: &[ExporterAssignment],
) -> HashMap<Uuid, Vec<ExporterAssignment>> {
    let mut result = HashMap::new();
    for host in hosts {
        let mut selected = assignments
            .iter()
            .filter(|assignment| {
                assignment_matches_host(host, &assignment.scope_type, assignment.scope_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        // An assignment is a scrape target, not an override keyed by exporter.
        // Multiple assignments of the same exporter can legitimately carry
        // different target/port/labels and must all reach service discovery.
        selected.sort_by_key(|assignment| (assignment.exporter_slug.clone(), assignment.id));
        result.insert(host.host_id, selected);
    }
    result
}

fn effective_metric_set_assignments(
    host: &DiscoveryHost,
    assignments: &[MetricSetAssignment],
) -> Vec<MetricSetAssignment> {
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
    selected
}

fn effective_query_sources(
    host: &DiscoveryHost,
    assignments: &[QuerySourceAssignment],
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

    // Walk highest precedence first. The first assignment retained for a
    // source type is its effective override, while the resulting cross-type
    // order remains meaningful to compatible-source consumers that use
    // `.find()` (for example Prometheus vs VictoriaMetrics).
    let mut seen_source_types = HashSet::new();
    selected
        .into_iter()
        .rev()
        .filter(|assignment| {
            seen_source_types.insert(assignment.source.source_type.to_ascii_lowercase())
        })
        .map(|assignment| assignment.source)
        .collect()
}

pub fn assignment_matches_host(
    host: &DiscoveryHost,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> bool {
    match scope_type {
        "global" => true,
        "site" => scope_id == host.site_id,
        "tenant" => scope_id == host.tenant_id,
        "role" => scope_id == host.role_id,
        "connection_target" => scope_id == Some(host.host_id),
        "device" => host.kind == "device" && scope_id == Some(host.host_id),
        "virtual_machine" | "vm" => {
            host.kind == "virtual_machine" && scope_id == Some(host.host_id)
        }
        _ => false,
    }
}

pub fn scope_precedence(scope_type: &str) -> u32 {
    match scope_type {
        "global" => 0,
        "tenant" => 10,
        "site" => 20,
        "role" => 30,
        "device" | "virtual_machine" | "vm" | "connection_target" => 40,
        _ => 0,
    }
}

/// Distinct host, site, tenant and role ids gathered from discovery hosts.
pub type ScopeIds = (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>, Vec<Uuid>);

pub fn collect_scope_ids(hosts: &[DiscoveryHost]) -> ScopeIds {
    let mut host_ids = HashSet::new();
    let mut site_ids = HashSet::new();
    let mut tenant_ids = HashSet::new();
    let mut role_ids = HashSet::new();
    for host in hosts {
        host_ids.insert(host.host_id);
        if let Some(id) = host.site_id {
            site_ids.insert(id);
        }
        if let Some(id) = host.tenant_id {
            tenant_ids.insert(id);
        }
        if let Some(id) = host.role_id {
            role_ids.insert(id);
        }
    }
    (
        host_ids.into_iter().collect(),
        site_ids.into_iter().collect(),
        tenant_ids.into_iter().collect(),
        role_ids.into_iter().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn availability_descriptor(id: Uuid, query: &str) -> MonitoringMetricDescriptor {
        MonitoringMetricDescriptor {
            name: query.to_string(),
            label: Some("Availability".to_string()),
            query: query.to_string(),
            unit: None,
            value_type: Some("state".to_string()),
            widget_type: Some("badge".to_string()),
            description: None,
            source_type: Some("prometheus".to_string()),
            query_type: Some("promql".to_string()),
            options: Value::Object(Default::default()),
            show_in_list: true,
            list_render: "auto".to_string(),
            metric_query_id: Some(id),
            category: None,
            semantic: Some("availability".to_string()),
        }
    }

    #[test]
    fn normalize_canonical_availability_prefers_blackbox_when_both_exporters_are_applied() {
        let node_exporter = Uuid::from_u128(101);
        let blackbox_exporter = Uuid::from_u128(102);
        let node_id = Uuid::from_u128(1);
        let blackbox_id = Uuid::from_u128(2);
        let mut metrics = vec![
            MonitoringMetricDescriptor {
                name: "load".to_string(),
                label: Some("Load".to_string()),
                query: "node_load1".to_string(),
                unit: None,
                value_type: Some("number".to_string()),
                widget_type: Some("stat".to_string()),
                description: None,
                source_type: Some("prometheus".to_string()),
                query_type: Some("promql".to_string()),
                options: Value::Object(Default::default()),
                show_in_list: true,
                list_render: "value".to_string(),
                metric_query_id: Some(Uuid::from_u128(3)),
                category: None,
                semantic: None,
            },
            availability_descriptor(node_id, "up{job=\"node\"}"),
            availability_descriptor(blackbox_id, "probe_success{job=\"blackbox\"}"),
        ];
        let query_exporter =
            HashMap::from([(node_id, node_exporter), (blackbox_id, blackbox_exporter)]);
        let applied = HashSet::from([node_exporter, blackbox_exporter]);

        normalize_canonical_availability(&mut metrics, &query_exporter, &applied);

        let availability = metrics
            .iter()
            .filter(|metric| metric.semantic.as_deref() == Some("availability"))
            .collect::<Vec<_>>();
        assert_eq!(availability.len(), 1);
        assert_eq!(availability[0].query, "probe_success{job=\"blackbox\"}");
        assert!(metrics.iter().any(|metric| metric.query == "node_load1"));
    }

    #[test]
    fn normalize_canonical_availability_ignores_availability_from_an_unapplied_exporter() {
        // Fresh-install shape: the node exporter is applied, blackbox is seeded
        // in the catalog but never scraped. Node `up` must win, and the blackbox
        // availability descriptor must not survive to render every host
        // "Unknown".
        let node_exporter = Uuid::from_u128(101);
        let blackbox_exporter = Uuid::from_u128(102);
        let node_id = Uuid::from_u128(1);
        let blackbox_id = Uuid::from_u128(2);
        let mut metrics = vec![
            availability_descriptor(node_id, "up{job=\"node\"}"),
            availability_descriptor(blackbox_id, "probe_success{job=\"blackbox\"}"),
        ];
        let query_exporter =
            HashMap::from([(node_id, node_exporter), (blackbox_id, blackbox_exporter)]);
        let applied = HashSet::from([node_exporter]);

        normalize_canonical_availability(&mut metrics, &query_exporter, &applied);

        let availability = metrics
            .iter()
            .filter(|metric| metric.semantic.as_deref() == Some("availability"))
            .collect::<Vec<_>>();
        assert_eq!(availability.len(), 1);
        assert_eq!(availability[0].query, "up{job=\"node\"}");
        assert!(
            !metrics
                .iter()
                .any(|metric| metric.query.contains("probe_success")),
            "an availability descriptor from an unapplied exporter must be dropped"
        );
    }

    #[test]
    fn normalize_canonical_availability_drops_availability_when_no_exporter_is_applied() {
        let node_exporter = Uuid::from_u128(101);
        let node_id = Uuid::from_u128(1);
        let mut metrics = vec![
            availability_descriptor(node_id, "up{job=\"node\"}"),
            descriptor_with("CPU", "%", Value::Object(Default::default())),
        ];
        let query_exporter = HashMap::from([(node_id, node_exporter)]);

        normalize_canonical_availability(&mut metrics, &query_exporter, &HashSet::new());

        assert!(
            !metrics
                .iter()
                .any(|metric| metric.semantic.as_deref() == Some("availability")),
            "no applied exporter means no canonical availability, not a stale one"
        );
        assert_eq!(metrics.len(), 1);
    }

    fn descriptor_with(label: &str, unit: &str, options: Value) -> MonitoringMetricDescriptor {
        MonitoringMetricDescriptor {
            name: "test".to_string(),
            label: Some(label.to_string()),
            query: "up".to_string(),
            unit: Some(unit.to_string()),
            value_type: Some("number".to_string()),
            widget_type: Some("stat".to_string()),
            description: None,
            source_type: Some("prometheus".to_string()),
            query_type: Some("promql".to_string()),
            options,
            show_in_list: false,
            list_render: "auto".to_string(),
            metric_query_id: None,
            category: None,
            semantic: None,
        }
    }

    /// A host with nothing filled in but its id.
    ///
    /// Written out rather than derived from `Default`: a `DiscoveryHost` with a
    /// nil id is not a meaningful value, and giving the domain type a
    /// `Default` would let one be constructed by accident in production code.
    fn bare_host(host_id: Uuid) -> DiscoveryHost {
        DiscoveryHost {
            host_id,
            host_name: "host".to_string(),
            host_address: "10.0.0.1".to_string(),
            kind: "device".to_string(),
            site_id: None,
            site_name: None,
            site_slug: None,
            tenant_id: None,
            tenant_name: None,
            tenant_slug: None,
            role_id: None,
            role_name: None,
            role_slug: None,
            status: None,
            platform_name: None,
            model: None,
            manufacturer: None,
            serial: None,
            asset_tag: None,
            primary_ip4: None,
            primary_ip6: None,
            interfaces: None,
            interface_addresses: Vec::new(),
            tunnel_addresses: Vec::new(),
            contacts: None,
            description: None,
            comments: None,
            site_facility: None,
            site_address: None,
            site_region: None,
            site_group: None,
            tenant_group: None,
            rack: None,
            cluster: None,
            monitoring: HostMonitoringConfig::default(),
        }
    }

    fn exporter_assignment(
        id: Uuid,
        exporter_id: Uuid,
        target: &str,
        port: i32,
    ) -> ExporterAssignment {
        ExporterAssignment {
            id,
            exporter_id,
            exporter_name: "node".to_string(),
            exporter_slug: "node".to_string(),
            assignment_name: Some(target.to_string()),
            scope_type: "global".to_string(),
            scope_id: None,
            weight: 0,
            default_job: "node".to_string(),
            default_scheme: "http".to_string(),
            default_port: Some(9100),
            default_metrics_path: "/metrics".to_string(),
            default_target_source: "primary_ip".to_string(),
            target: Some(target.to_string()),
            job: None,
            scheme: None,
            port: Some(port),
            metrics_path: None,
            exporter_labels: HashMap::new(),
            assignment_labels: HashMap::new(),
        }
    }

    fn query_source_assignment(
        id: Uuid,
        name: &str,
        source_type: &str,
        scope_type: &str,
        scope_id: Option<Uuid>,
        weight: i32,
    ) -> QuerySourceAssignment {
        QuerySourceAssignment {
            id,
            source: MonitoringQuerySourceConfig {
                name: Some(name.to_string()),
                source_type: source_type.to_string(),
                query_url: Some(format!("http://{name}.example/api/v1/query")),
                credential_secret_id: None,
                timeout_ms: Some(5000),
                options: Value::Object(Default::default()),
            },
            scope_type: scope_type.to_string(),
            scope_id,
            weight,
        }
    }

    fn inputs_with_queries(
        host_id: Uuid,
        exporter_id: Uuid,
        queries: Vec<MetricQueryRecord>,
        overrides: &HashMap<(Uuid, Uuid), HostOverrideResolved>,
    ) -> HostEnrichmentInputs<'_> {
        let mut effective_exporters = HashMap::new();
        effective_exporters.insert(
            host_id,
            vec![ExporterAssignment {
                id: Uuid::from_u128(900),
                exporter_id,
                exporter_name: "node".to_string(),
                exporter_slug: "node".to_string(),
                assignment_name: None,
                scope_type: "host".to_string(),
                scope_id: Some(host_id),
                weight: 0,
                default_job: "node".to_string(),
                default_scheme: "http".to_string(),
                default_port: Some(9100),
                default_metrics_path: "/metrics".to_string(),
                default_target_source: "primary_ip".to_string(),
                target: None,
                job: None,
                scheme: None,
                port: None,
                metrics_path: None,
                exporter_labels: HashMap::new(),
                assignment_labels: HashMap::new(),
            }],
        );
        let mut metric_queries = HashMap::new();
        metric_queries.insert(exporter_id, queries);
        HostEnrichmentInputs {
            effective_exporters,
            metric_queries,
            query_source_assignments: Vec::new(),
            metric_set_assignments: Vec::new(),
            metric_set_items: HashMap::new(),
            default_list_items: Vec::new(),
            host_overrides: overrides,
        }
    }

    #[test]
    fn exporter_metrics_reach_the_inspector_only_when_flagged() {
        let host_id = Uuid::from_u128(10);
        let exporter_id = Uuid::from_u128(20);
        let overrides = HashMap::new();
        let inputs = inputs_with_queries(
            host_id,
            exporter_id,
            vec![
                MetricQueryRecord {
                    metric_query_id: Uuid::from_u128(1),
                    descriptor: descriptor_with("Listed", "%", Value::Object(Default::default())),
                    show_in_inspector: false,
                },
                MetricQueryRecord {
                    metric_query_id: Uuid::from_u128(2),
                    descriptor: descriptor_with(
                        "Inspected",
                        "%",
                        Value::Object(Default::default()),
                    ),
                    show_in_inspector: true,
                },
            ],
            &overrides,
        );

        let (all, inspector) = exporter_derived_metrics(host_id, &inputs);

        assert_eq!(all.len(), 2, "every exporter query belongs to the list");
        assert_eq!(
            inspector.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![Some(Uuid::from_u128(2))],
            "only the flagged query belongs to the inspector"
        );
    }

    #[test]
    fn multiple_assignments_of_one_exporter_remain_distinct_scrape_targets() {
        let host_id = Uuid::from_u128(10);
        let exporter_id = Uuid::from_u128(20);
        let host = bare_host(host_id);
        let assignments = vec![
            exporter_assignment(Uuid::from_u128(1), exporter_id, "10.0.0.1", 9100),
            exporter_assignment(Uuid::from_u128(2), exporter_id, "10.0.0.2", 9200),
        ];

        let effective = effective_exporters_by_host(&[host], &assignments);
        let targets = effective[&host_id]
            .iter()
            .map(|assignment| (assignment.target.as_deref(), assignment.port))
            .collect::<Vec<_>>();

        assert_eq!(
            targets,
            vec![
                (Some("10.0.0.1"), Some(9100)),
                (Some("10.0.0.2"), Some(9200))
            ],
            "an assignment is a target instance, not an exporter-wide override"
        );
    }

    #[test]
    fn effective_query_sources_keep_scope_precedence_across_compatible_types() {
        let site_id = Uuid::from_u128(100);
        let mut host = bare_host(Uuid::from_u128(10));
        host.site_id = Some(site_id);
        let assignments = vec![
            query_source_assignment(
                Uuid::from_u128(1),
                "central-prometheus",
                "prometheus",
                "global",
                None,
                0,
            ),
            query_source_assignment(
                Uuid::from_u128(2),
                "site-victoriametrics",
                "victoriametrics",
                "site",
                Some(site_id),
                0,
            ),
        ];

        let sources = effective_query_sources(&host, &assignments);

        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources[0].name.as_deref(),
            Some("site-victoriametrics"),
            "the site override must be the first compatible source selected by consumers"
        );
        assert_eq!(sources[1].name.as_deref(), Some("central-prometheus"));
    }

    #[test]
    fn a_host_with_no_exporters_gets_no_metrics_rather_than_someone_elses() {
        let overrides = HashMap::new();
        let inputs = inputs_with_queries(
            Uuid::from_u128(10),
            Uuid::from_u128(20),
            vec![MetricQueryRecord {
                metric_query_id: Uuid::from_u128(1),
                descriptor: descriptor_with("Listed", "%", Value::Object(Default::default())),
                show_in_inspector: true,
            }],
            &overrides,
        );

        // A different host: the exporter map has no entry for it.
        let (all, inspector) = exporter_derived_metrics(Uuid::from_u128(11), &inputs);

        assert!(
            all.is_empty() && inspector.is_empty(),
            "metrics must not leak from a host that happens to be in the same batch"
        );
    }

    #[test]
    fn the_default_list_fallback_only_adds_and_never_replaces() {
        let host_id = Uuid::from_u128(10);
        let overrides = HashMap::new();
        let mut inputs = inputs_with_queries(host_id, Uuid::from_u128(20), Vec::new(), &overrides);
        inputs.default_list_items = vec![MetricSetItem {
            metric_query_id: Uuid::from_u128(1),
            descriptor: descriptor_with("Fallback", "%", Value::Object(Default::default())),
            show_in_inspector: true,
            exporter_id: Some(Uuid::from_u128(20)),
        }];

        let host = bare_host(host_id);
        let mut all = vec![(
            Some(Uuid::from_u128(99)),
            descriptor_with("FromExporter", "%", Value::Object(Default::default())),
        )];
        let mut inspector = vec![(
            Some(Uuid::from_u128(99)),
            descriptor_with("FromExporter", "%", Value::Object(Default::default())),
        )];

        // No metric set assigned, and the descriptor carries no query source
        // requirement, so the fallback applies.
        apply_metric_sets_or_exporter_defaults(&host, &inputs, &[], &mut all, &mut inspector);

        assert_eq!(
            inspector.len(),
            1,
            "the fallback must not clear the inspector list — only an explicit \
             metric set replaces it"
        );
        assert!(
            all.iter().any(|(id, _)| *id == Some(Uuid::from_u128(99))),
            "the fallback must not drop what the exporters already produced"
        );
    }

    #[test]
    fn apply_host_overrides_replaces_label_unit_and_display() {
        let host_id = Uuid::new_v4();
        let mq_id = Uuid::new_v4();
        let mut overrides = HashMap::new();
        overrides.insert(
            (host_id, mq_id),
            HostOverrideResolved {
                label_override: Some("Host CPU".to_string()),
                unit_override: Some("ms".to_string()),
                options_override: Some(serde_json::json!({
                    "status": {"mode": "numeric", "warning": 50, "critical": 90}
                })),
            },
        );
        let mut descriptors = vec![(
            Some(mq_id),
            descriptor_with(
                "Default label",
                "%",
                serde_json::json!({"display": {"status": {"mode": "state"}}}),
            ),
        )];
        apply_host_overrides(&overrides, host_id, &mut descriptors);
        let (_, descriptor) = &descriptors[0];
        assert_eq!(descriptor.label.as_deref(), Some("Host CPU"));
        assert_eq!(descriptor.unit.as_deref(), Some("ms"));
        let display = descriptor.options.get("display").unwrap();
        let mode = display.get("status").and_then(|s| s.get("mode")).unwrap();
        assert_eq!(mode.as_str(), Some("numeric"));
    }

    #[test]
    fn apply_host_overrides_skips_descriptors_without_metric_query_id() {
        let host_id = Uuid::new_v4();
        let mq_id = Uuid::new_v4();
        let mut overrides = HashMap::new();
        overrides.insert(
            (host_id, mq_id),
            HostOverrideResolved {
                label_override: Some("Should not apply".to_string()),
                unit_override: None,
                options_override: None,
            },
        );
        let mut descriptors = vec![(None, descriptor_with("Untouched", "%", Value::Null))];
        apply_host_overrides(&overrides, host_id, &mut descriptors);
        let (_, descriptor) = &descriptors[0];
        assert_eq!(descriptor.label.as_deref(), Some("Untouched"));
    }

    #[test]
    fn apply_host_overrides_does_not_apply_other_hosts_overrides() {
        let host_a = Uuid::new_v4();
        let host_b = Uuid::new_v4();
        let mq_id = Uuid::new_v4();
        let mut overrides = HashMap::new();
        overrides.insert(
            (host_b, mq_id),
            HostOverrideResolved {
                label_override: Some("Host B label".to_string()),
                unit_override: None,
                options_override: None,
            },
        );
        let mut descriptors = vec![(
            Some(mq_id),
            descriptor_with("Host A label", "%", Value::Null),
        )];
        apply_host_overrides(&overrides, host_a, &mut descriptors);
        let (_, descriptor) = &descriptors[0];
        assert_eq!(descriptor.label.as_deref(), Some("Host A label"));
    }
}
