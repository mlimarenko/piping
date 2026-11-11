//! Exporter discovery run ingestion: resolve one host's collected evidence and
//! its endpoint probes into the contract report.
//!
//! The monitoring owner is the only writer of this report. It never trusts the
//! payload: the run binding is checked by the caller, declared endpoints come
//! from the host's current monitoring configuration, and probe targets stay
//! inside the host's own addresses. Service-discovery changes are not made here.

use std::collections::BTreeMap;

use crate::contexts::monitoring::domain::exporter_discovery::{
    DiscoveryReport, EndpointIdentity, EndpointObservation, ObservationProvenance,
};
use crate::contexts::monitoring::domain::host::DiscoveryHost;
use crate::errors::AppError;

use super::exporter_discovery::{
    DeclaredEndpoint, DiscoveryCollectionArtifact, HostEvidence, resolve_host_inspection,
};
use super::exporter_probe::{ProbeSettings, ProbeTarget, probe_targets};

/// Fully bound input for one host's discovery ingestion.
pub struct DiscoveryIngestInput {
    pub collection: DiscoveryCollectionArtifact,
    pub host: DiscoveryHost,
}

pub struct DiscoveryIngestResult {
    pub report: DiscoveryReport,
    pub probe_target_count: usize,
}

/// Resolve the report for one host. Pure read: no database write and no
/// service-discovery mutation.
pub async fn ingest_discovery_run(
    input: DiscoveryIngestInput,
    probe_settings: ProbeSettings,
) -> Result<DiscoveryIngestResult, AppError> {
    let collection = &input.collection;
    collection.validate()?;
    let host = &input.host;
    if host.host_id != collection.host_id {
        return Err(AppError::Validation(
            "discovery collection host does not match the loaded host".to_string(),
        ));
    }
    if host.kind != collection.source_kind {
        return Err(AppError::Validation(
            "discovery collection source kind does not match the host".to_string(),
        ));
    }
    let observable_addresses = observable_addresses(host);
    let excluded = collection.options.excluded_hosts.iter().any(|value| {
        value == &host.host_name
            || value == &host.host_id.to_string()
            || observable_addresses.iter().any(|address| address == value)
    });
    let whole_host_skipped = excluded || collection.skipped;
    let evidence = if whole_host_skipped || !collection.sources.host_inspection {
        HostEvidence::skipped(collection.platform())
    } else {
        HostEvidence::parse(collection)?
    };
    let provenance = ObservationProvenance {
        run_id: collection.run_id,
        run_group_id: collection.run_group_id,
        connection_target_id: collection.connection_target_id,
        source_kind: collection.source_kind.clone(),
        source_id: collection.host_id,
        observed_from: "runner".to_string(),
    };
    let declared = declared_endpoints(host);
    let observed_at = collection.finished_at.clone();
    let resolution = resolve_host_inspection(
        &evidence,
        &declared,
        &observable_addresses,
        &collection.options.excluded_ports,
        &provenance,
        &observed_at,
        collection.options.detect_exporter_type,
        collection.options.max_endpoints_per_host,
    );

    let mut observations = resolution.observations;
    let mut probe_target_count = 0;
    if collection.sources.endpoint_probe && !whole_host_skipped {
        let targets = probe_targets_for_run(host, &declared, &observations, collection);
        probe_target_count = targets.len();
        if !targets.is_empty() {
            let probe_provenance = ObservationProvenance {
                observed_from: "backend".to_string(),
                ..provenance.clone()
            };
            let probes = probe_targets(
                targets,
                &probe_provenance,
                &observed_at,
                probe_settings,
                collection.options.probe_concurrency,
            )
            .await;
            observations.extend(probes);
        }
    }

    let mut host_names = BTreeMap::new();
    host_names.insert(host.host_id, (host.host_name.clone(), host.kind.clone()));
    let sources = BTreeMap::from([
        (
            "host_inspection".to_string(),
            collection.sources.host_inspection,
        ),
        (
            "endpoint_probe".to_string(),
            collection.sources.endpoint_probe,
        ),
    ]);
    let report = DiscoveryReport::build(
        collection.run_id,
        collection.run_group_id,
        collection.started_at.clone(),
        collection.finished_at.clone(),
        sources,
        &host_names,
        observations,
    );
    report.validate()?;
    Ok(DiscoveryIngestResult {
        report,
        probe_target_count,
    })
}

/// Addresses the host can legitimately be scraped at. Display names never
/// qualify.
#[must_use]
pub fn observable_addresses(host: &DiscoveryHost) -> Vec<String> {
    let mut addresses = Vec::new();
    let mut push = |value: &str| {
        let value = value.trim();
        if !value.is_empty() && !addresses.iter().any(|existing| existing == value) {
            addresses.push(value.to_string());
        }
    };
    push(&host.host_address);
    if let Some(value) = host.primary_ip4.as_deref() {
        push(value);
    }
    if let Some(value) = host.primary_ip6.as_deref() {
        push(value);
    }
    for item in host
        .interface_addresses
        .iter()
        .chain(host.tunnel_addresses.iter())
    {
        push(&item.address);
    }
    addresses
}

/// Declared endpoints from the host's current monitoring assignments.
#[must_use]
pub fn declared_endpoints(host: &DiscoveryHost) -> Vec<DeclaredEndpoint> {
    let observable = observable_addresses(host);
    let fallback_address = observable.first().cloned().unwrap_or_default();
    host.monitoring
        .scrapes
        .iter()
        .filter(|scrape| scrape.enabled != Some(false))
        .filter_map(|scrape| {
            let port = scrape.port?;
            let address = scrape
                .target
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(fallback_address.as_str())
                .to_string();
            let scheme = scrape
                .scheme
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| "http".to_string());
            let path = scrape
                .metrics_path
                .as_deref()
                .map(super::super::domain::exporter_discovery::normalize_discovery_path)
                .unwrap_or_else(|| "/metrics".to_string());
            let endpoint =
                EndpointIdentity::validated(host.host_id, &address, port, &scheme, &path).ok()?;
            Some(DeclaredEndpoint {
                endpoint,
                exporter_type: None,
                labels: scrape
                    .labels
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            })
        })
        .collect()
}

fn probe_targets_for_run(
    host: &DiscoveryHost,
    declared: &[DeclaredEndpoint],
    observations: &[EndpointObservation],
    collection: &DiscoveryCollectionArtifact,
) -> Vec<ProbeTarget> {
    let observable = observable_addresses(host);
    let mut targets = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    let mut push = |endpoint: EndpointIdentity,
                    exporter_type: Option<String>,
                    labels: BTreeMap<String, String>| {
        if collection.options.excluded_ports.contains(&endpoint.port) {
            return;
        }
        let key = (
            endpoint.address.clone(),
            endpoint.port,
            endpoint.path.clone(),
            endpoint.scheme.clone(),
        );
        if !seen.insert(key) {
            return;
        }
        targets.push(ProbeTarget {
            endpoint,
            exporter_type,
            labels,
        });
    };

    for declared_endpoint in declared {
        push(
            declared_endpoint.endpoint.clone(),
            declared_endpoint.exporter_type.clone(),
            declared_endpoint.labels.clone(),
        );
    }
    for observation in observations {
        // Only endpoints that are evidence of presence are probed again; a
        // partial inspection or an absence must not turn into a probe of an
        // address the host never proved it owns.
        if observation.status
            != crate::contexts::monitoring::domain::exporter_discovery::DiscoveryStatus::ConfirmedDiscovered
        {
            continue;
        }
        push(
            observation.endpoint.clone(),
            observation.exporter_type.clone(),
            observation.labels.clone(),
        );
    }
    for extra in &collection.options.extra_endpoints {
        let address = extra.host.trim();
        if !observable.iter().any(|candidate| candidate == address) {
            continue;
        }
        if let Ok(endpoint) = EndpointIdentity::validated(
            host.host_id,
            address,
            extra.port,
            &extra.scheme,
            &extra.path,
        ) {
            push(endpoint, None, collection.options.labels.clone());
        }
    }
    targets.truncate(collection.options.max_endpoints_per_host);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::monitoring::domain::host::sample_host;

    #[test]
    fn observable_addresses_are_bounded_to_host_identities() {
        let host = sample_host();
        let addresses = observable_addresses(&host);
        assert!(addresses.contains(&"10.0.0.10".to_string()));
        assert!(!addresses.iter().any(|value| value == "db-01"));
    }

    #[test]
    fn declared_endpoints_use_configured_scrapes_only() {
        let mut host = sample_host();
        host.host_id = uuid::Uuid::from_u128(1);
        host.monitoring.scrapes = vec![
            crate::contexts::monitoring::domain::host::MonitoringScrapeConfig {
                enabled: Some(true),
                port: Some(9100),
                scheme: Some("http".to_string()),
                metrics_path: Some("metrics".to_string()),
                ..Default::default()
            },
            crate::contexts::monitoring::domain::host::MonitoringScrapeConfig {
                enabled: Some(false),
                port: Some(9200),
                ..Default::default()
            },
        ];
        let declared = declared_endpoints(&host);
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].endpoint.port, 9100);
        assert_eq!(declared[0].endpoint.path, "/metrics");
    }
}
