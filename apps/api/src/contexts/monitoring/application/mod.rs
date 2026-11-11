//! Monitoring application services: host read-model assembly, provider
//! source resolution, the observability poller, and the per-host cache
//! contract.

pub mod coverage;
pub mod exporter_discovery;
pub mod exporter_discovery_ingest;
pub mod exporter_discovery_reconcile;
pub mod exporter_probe;
pub mod host_cache;
pub mod poller;
pub mod read_model;
pub mod source_resolution;

pub use exporter_discovery::{
    CollectionCoverage, CollectionOptions, CollectionSources, DeclaredEndpoint,
    DiscoveryCollectionArtifact, ExtraEndpoint, HostEvidence, HostResolution, ObservedContainer,
    ObservedListener, ObservedOwner, probe_observation, resolve_host_inspection,
};
pub use exporter_probe::{ProbeSettings, ProbeTarget, probe_endpoint, probe_targets};

pub use coverage::{
    CoverageAssignmentInfo, CoverageAssignmentSummary, CoverageDuplicate,
    CoverageDuplicateOccurrence, CoverageHostEvidence, CoverageHostResult, CoverageReason,
    CoverageResult, CoverageState, CoverageTarget, CoverageTotals, build_coverage,
};
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
