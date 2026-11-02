//! Monitoring application services: host read-model assembly, provider
//! source resolution, the observability poller, and the per-host cache
//! contract.

pub mod host_cache;
pub mod poller;
pub mod read_model;
pub mod source_resolution;

pub use host_cache::{
    CachedHostAlerts, CachedHostLabels, CachedHostMetric, CachedHostState, host_alerts_cache_key,
    host_labels_cache_key, read_host_states,
};
pub use poller::{ObservabilityPollSummary, poll_observability_list_metrics};
pub use read_model::{enrich_hosts_with_monitoring, fetch_discovery_hosts};
pub use source_resolution::{
    pin_monitoring_query_url, prometheus_compatible_settings_query_url, redacted_url,
    resolve_metric_query_source, validate_monitoring_query_source_url,
    validate_monitoring_query_url,
};
