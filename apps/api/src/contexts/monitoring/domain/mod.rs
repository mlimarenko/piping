//! Monitoring domain: host read-model types, provider vocabulary, query
//! templating, fleet-vector correlation, and alert normalization. Pure logic —
//! no I/O.

pub mod alerts;
pub mod correlation;
pub mod host;
pub mod source;
pub mod templating;

pub use alerts::{
    AlertProvider, HostAlert, alert_provider_for_source_type, alert_provider_query_url,
    correlate_alerts_by_host, parse_alerts_for_provider,
};
pub use correlation::{
    build_host_correlation_index, correlation_label_for_source, correlation_match_for_source,
    demux_vector_by_host_label, is_list_capable_query, scope_prometheus_metric_to_host,
};
pub use host::{
    DiscoveryHost, HostMonitoringConfig, MonitoringMetricDescriptor, MonitoringQuerySourceConfig,
    MonitoringScrapeConfig, descriptor_query_type, descriptor_source_type,
    is_prometheus_compatible_source_type, is_zabbix_source_type,
    metric_descriptor_has_query_source, source_type_matches_descriptor,
};
pub use source::{LatestMetricSourceInfo, PinnedMonitoringResolution, ResolvedMetricQuerySource};
pub use templating::{render_host_metric_query, render_zabbix_host_selector};
