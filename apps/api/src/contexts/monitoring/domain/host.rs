//! The monitoring host read model: a discovered host enriched with its
//! resolved scrape/metric/query-source configuration, plus the descriptor
//! accessors shared by the poller, the provider clients, and the REST layer.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::inventory::domain::DiscoveryInterfaceAddress;

#[derive(Debug, Clone, Default)]
pub struct HostMonitoringConfig {
    pub scrapes: Vec<MonitoringScrapeConfig>,
    pub metrics: Vec<MonitoringMetricDescriptor>,
    pub inspector_metrics: Vec<MonitoringMetricDescriptor>,
    pub query_sources: Vec<MonitoringQuerySourceConfig>,
}

#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
pub struct MonitoringScrapeConfig {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub target: Option<String>,
    pub target_source: Option<String>,
    pub job: Option<String>,
    pub scheme: Option<String>,
    pub metrics_path: Option<String>,
    pub port: Option<u16>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
pub struct MonitoringMetricDescriptor {
    pub name: String,
    pub label: Option<String>,
    pub query: String,
    pub unit: Option<String>,
    pub value_type: Option<String>,
    pub widget_type: Option<String>,
    pub description: Option<String>,
    pub source_type: Option<String>,
    pub query_type: Option<String>,
    pub options: Value,
    /// Whether this metric should surface as an inline list/table label.
    ///
    /// Resolved as the set-item `show_in_list` override OR the metric-level
    /// `default_show_in_list`, mirroring how `show_in_inspector` resolves.
    pub show_in_list: bool,
    /// Inline list render hint (`auto`, `dot`, `value`, `boolean`, `gauge`).
    pub list_render: String,
    /// Identity of the stored metric query this descriptor came from; absent
    /// for descriptors with no stored query (for example the one-shot
    /// live-test path). Per-host overrides address this id, so two queries that
    /// share the same `query` text stay distinct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_query_id: Option<Uuid>,
    /// Optional visual grouping for inspector metric rendering. `null` means
    /// "uncategorized". Set from the metric query's `category`; not a free-form
    /// label override.
    pub category: Option<String>,
    /// Optional machine-readable meaning for cross-surface rendering. `availability`
    /// marks the canonical host up/down metric selected by backend scope rules.
    pub semantic: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MonitoringQuerySourceConfig {
    pub name: Option<String>,
    pub source_type: String,
    pub query_url: Option<String>,
    /// Canonical named-secret identity for a monitoring source. This is
    /// populated from `secret_usage_binding`; a source never persists a second
    /// credential reference of its own, for either query or alert sources.
    pub credential_secret_id: Option<Uuid>,
    pub timeout_ms: Option<u64>,
    pub options: Value,
}

#[derive(Debug, Clone)]
pub struct DiscoveryHost {
    pub host_id: Uuid,
    pub host_name: String,
    pub host_address: String,
    pub kind: String,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub site_slug: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub tenant_slug: Option<String>,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub role_slug: Option<String>,
    pub status: Option<String>,
    pub platform_name: Option<String>,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub primary_ip4: Option<String>,
    pub primary_ip6: Option<String>,
    pub interfaces: Option<String>,
    pub interface_addresses: Vec<DiscoveryInterfaceAddress>,
    pub tunnel_addresses: Vec<DiscoveryInterfaceAddress>,
    pub contacts: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub site_facility: Option<String>,
    pub site_address: Option<String>,
    pub site_region: Option<String>,
    pub site_group: Option<String>,
    pub tenant_group: Option<String>,
    pub rack: Option<String>,
    pub cluster: Option<String>,
    pub monitoring: HostMonitoringConfig,
}

pub fn descriptor_source_type(descriptor: &MonitoringMetricDescriptor) -> &str {
    descriptor
        .source_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("prometheus")
}

pub fn descriptor_query_type(descriptor: &MonitoringMetricDescriptor) -> &str {
    descriptor
        .query_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if is_zabbix_source_type(descriptor_source_type(descriptor)) {
                "zabbix_item_key"
            } else {
                "promql"
            }
        })
}

pub fn is_zabbix_source_type(source_type: &str) -> bool {
    source_type.eq_ignore_ascii_case("zabbix")
}

pub fn is_prometheus_compatible_source_type(source_type: &str) -> bool {
    matches!(
        source_type.to_ascii_lowercase().as_str(),
        "prometheus" | "victoriametrics"
    )
}

pub fn source_type_matches_descriptor(source_type: &str, descriptor_source_type: &str) -> bool {
    source_type.eq_ignore_ascii_case(descriptor_source_type)
        || (is_prometheus_compatible_source_type(source_type)
            && is_prometheus_compatible_source_type(descriptor_source_type))
}

pub fn metric_descriptor_has_query_source(
    descriptor: &MonitoringMetricDescriptor,
    sources: &[MonitoringQuerySourceConfig],
) -> bool {
    let descriptor_source_type = descriptor_source_type(descriptor);
    sources
        .iter()
        .any(|source| source_type_matches_descriptor(&source.source_type, descriptor_source_type))
}

/// Deterministic host fixture shared by the monitoring context unit tests.
#[cfg(test)]
pub(crate) fn sample_host() -> DiscoveryHost {
    DiscoveryHost {
        host_id: Uuid::nil(),
        host_name: "db-01".to_string(),
        host_address: "10.0.0.10".to_string(),
        kind: "virtual_machine".to_string(),
        site_id: None,
        site_name: None,
        site_slug: None,
        tenant_id: None,
        tenant_name: None,
        tenant_slug: None,
        role_id: None,
        role_name: None,
        role_slug: None,
        status: Some("active".to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_type_matching_allows_prometheus_compatible_sources() {
        assert!(source_type_matches_descriptor(
            "victoriametrics",
            "prometheus"
        ));
        assert!(source_type_matches_descriptor(
            "prometheus",
            "victoriametrics"
        ));
        assert!(source_type_matches_descriptor("zabbix", "zabbix"));
        assert!(!source_type_matches_descriptor("zabbix", "prometheus"));
    }

    #[test]
    fn default_metric_fallback_requires_matching_query_source() {
        let prometheus_descriptor = MonitoringMetricDescriptor {
            name: "load1".to_string(),
            query: "node_load1".to_string(),
            source_type: Some("prometheus".to_string()),
            query_type: Some("promql".to_string()),
            options: Value::Object(Default::default()),
            list_render: "auto".to_string(),
            ..MonitoringMetricDescriptor::default()
        };
        let zabbix_descriptor = MonitoringMetricDescriptor {
            name: "zabbix-agent-ping".to_string(),
            label: Some("Agent ping".to_string()),
            query: "agent.ping".to_string(),
            unit: None,
            value_type: Some("state".to_string()),
            widget_type: Some("badge".to_string()),
            description: None,
            source_type: Some("zabbix".to_string()),
            query_type: Some("zabbix_item_key".to_string()),
            options: Value::Object(Default::default()),
            show_in_list: true,
            list_render: "auto".to_string(),
            metric_query_id: None,
            category: None,
            semantic: None,
        };
        let victoria_source = MonitoringQuerySourceConfig {
            source_type: "victoriametrics".to_string(),
            query_url: Some("https://monitoring.example/api/v1/query".to_string()),
            ..MonitoringQuerySourceConfig::default()
        };

        assert!(metric_descriptor_has_query_source(
            &prometheus_descriptor,
            std::slice::from_ref(&victoria_source),
        ));
        assert!(!metric_descriptor_has_query_source(
            &zabbix_descriptor,
            std::slice::from_ref(&victoria_source),
        ));
        assert!(metric_descriptor_has_query_source(
            &zabbix_descriptor,
            &[MonitoringQuerySourceConfig {
                source_type: "zabbix".to_string(),
                query_url: Some("https://zabbix.example/api_jsonrpc.php".to_string()),
                ..MonitoringQuerySourceConfig::default()
            }],
        ));
    }
}
