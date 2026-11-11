//! Host-inspection evidence parsing and status resolution for exporter
//! discovery.
//!
//! The playbook is a mechanical collector: it returns bounded command output and
//! coverage flags. This module owns the interpretation, so status resolution is
//! product code with product tests instead of an untestable template.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

use super::super::domain::exporter_discovery::{
    ConnectProof, DiscoveryOrigin, DiscoveryPlatform, DiscoveryProof, DiscoverySource,
    DiscoveryStatus, EndpointIdentity, EndpointObservation, IdentifierKind, ListenerProof,
    ObservationProvenance, OwnerProof, reason, resolve_exporter_type,
};

/// Bounds shared with the playbook schema.
pub const DISCOVERY_MAX_RAW_LISTENER_LINES: usize = 5_000;
pub const DISCOVERY_MAX_RAW_SERVICE_LINES: usize = 5_000;
pub const DISCOVERY_MAX_RAW_PROCESS_LINES: usize = 5_000;
pub const DISCOVERY_MAX_RAW_CONTAINER_LINES: usize = 500;
pub const DISCOVERY_MAX_EXTRA_ENDPOINTS: usize = 500;
pub const DISCOVERY_MAX_EXCLUSIONS: usize = 500;
pub const DISCOVERY_MAX_ENDPOINTS_PER_HOST_LIMIT: usize = 512;
pub const DISCOVERY_MIN_PROBE_TIMEOUT_MS: u64 = 100;
pub const DISCOVERY_MAX_PROBE_TIMEOUT_MS: u64 = 30_000;
pub const DISCOVERY_MAX_PROBE_CONCURRENCY: usize = 16;

/// The collection artifact written by the discovery playbook.
#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct DiscoveryCollectionArtifact {
    pub schema_version: u32,
    pub run_id: Uuid,
    pub run_group_id: Uuid,
    pub connection_target_id: Uuid,
    pub host_id: Uuid,
    pub host_name: String,
    pub source_kind: String,
    pub platform: String,
    pub started_at: String,
    pub finished_at: String,
    #[serde(default)]
    pub skipped: bool,
    pub sources: CollectionSources,
    #[serde(default)]
    pub options: CollectionOptions,
    pub coverage: CollectionCoverage,
    #[serde(default)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct CollectionSources {
    #[serde(default = "default_true")]
    pub host_inspection: bool,
    #[serde(default = "default_true")]
    pub endpoint_probe: bool,
}

impl Default for CollectionSources {
    fn default() -> Self {
        Self {
            host_inspection: true,
            endpoint_probe: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct CollectionOptions {
    #[serde(default = "default_true")]
    pub detect_exporter_type: bool,
    #[serde(default)]
    pub excluded_hosts: Vec<String>,
    #[serde(default)]
    pub excluded_ports: Vec<u16>,
    #[serde(default)]
    pub extra_endpoints: Vec<ExtraEndpoint>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default = "default_probe_timeout_ms")]
    pub probe_timeout_ms: u64,
    #[serde(default = "default_probe_concurrency")]
    pub probe_concurrency: usize,
    #[serde(default = "default_max_endpoints_per_host")]
    pub max_endpoints_per_host: usize,
    /// Absence policy mode: `manual` (default) or explicit `automatic`.
    #[serde(default = "default_absence_policy")]
    pub absence_policy: String,
    /// Consecutive complete runs required before an automatic exclusion.
    #[serde(default = "default_absence_confirm_runs")]
    pub absence_confirm_runs: u32,
    /// Restore a source-owned assignment when the endpoint is confirmed again.
    #[serde(default = "default_true")]
    pub restore_on_reappearance: bool,
}

impl Default for CollectionOptions {
    fn default() -> Self {
        Self {
            detect_exporter_type: true,
            excluded_hosts: Vec::new(),
            excluded_ports: Vec::new(),
            extra_endpoints: Vec::new(),
            labels: BTreeMap::new(),
            probe_timeout_ms: default_probe_timeout_ms(),
            probe_concurrency: default_probe_concurrency(),
            max_endpoints_per_host: default_max_endpoints_per_host(),
            absence_policy: default_absence_policy(),
            absence_confirm_runs: default_absence_confirm_runs(),
            restore_on_reappearance: true,
        }
    }
}

fn default_absence_policy() -> String {
    "manual".to_string()
}

fn default_absence_confirm_runs() -> u32 {
    3
}

fn default_probe_timeout_ms() -> u64 {
    3_000
}

fn default_probe_concurrency() -> usize {
    4
}

fn default_max_endpoints_per_host() -> usize {
    64
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct ExtraEndpoint {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

fn default_scheme() -> String {
    "http".to_string()
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct CollectionCoverage {
    pub listeners: String,
    pub services: String,
    pub processes: String,
    pub containers: String,
    pub listener_owners: String,
}

impl CollectionCoverage {
    fn listeners_complete(&self) -> bool {
        self.listeners == "complete"
    }
}

impl DiscoveryCollectionArtifact {
    /// Validate the collection envelope and its typed options.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 1 {
            return Err(AppError::Validation(format!(
                "unsupported discovery collection schema version {}",
                self.schema_version
            )));
        }
        if self.run_id.is_nil()
            || self.run_group_id.is_nil()
            || self.connection_target_id.is_nil()
            || self.host_id.is_nil()
        {
            return Err(AppError::Validation(
                "discovery collection is missing a run, target or host identity".to_string(),
            ));
        }
        if self.host_name.trim().is_empty() || self.host_name.len() > 255 {
            return Err(AppError::Validation(
                "discovery collection host name is empty or too long".to_string(),
            ));
        }
        if !matches!(self.source_kind.as_str(), "device" | "virtual_machine") {
            return Err(AppError::Validation(
                "discovery collection source kind must be device or virtual_machine".to_string(),
            ));
        }
        if !matches!(self.platform.as_str(), "linux" | "windows") {
            return Err(AppError::Validation(
                "discovery collection platform must be linux or windows".to_string(),
            ));
        }
        if !is_rfc3339_timestamp(&self.started_at) || !is_rfc3339_timestamp(&self.finished_at) {
            return Err(AppError::Validation(
                "discovery collection timestamps must be RFC 3339 UTC instants".to_string(),
            ));
        }
        if self.options.excluded_hosts.len() > DISCOVERY_MAX_EXCLUSIONS
            || self.options.excluded_ports.len() > DISCOVERY_MAX_EXCLUSIONS
            || self.options.extra_endpoints.len() > DISCOVERY_MAX_EXTRA_ENDPOINTS
        {
            return Err(AppError::Validation(
                "discovery collection options exceed their bounds".to_string(),
            ));
        }
        if !(DISCOVERY_MIN_PROBE_TIMEOUT_MS..=DISCOVERY_MAX_PROBE_TIMEOUT_MS)
            .contains(&self.options.probe_timeout_ms)
        {
            return Err(AppError::Validation(
                "discovery probe timeout is out of bounds".to_string(),
            ));
        }
        if self.options.probe_concurrency == 0
            || self.options.probe_concurrency > DISCOVERY_MAX_PROBE_CONCURRENCY
        {
            return Err(AppError::Validation(
                "discovery probe concurrency is out of bounds".to_string(),
            ));
        }
        if self.options.max_endpoints_per_host == 0
            || self.options.max_endpoints_per_host > DISCOVERY_MAX_ENDPOINTS_PER_HOST_LIMIT
        {
            return Err(AppError::Validation(
                "discovery endpoint cap per host is out of bounds".to_string(),
            ));
        }
        Ok(())
    }

    /// Platform parsed from the validated string.
    pub fn platform(&self) -> DiscoveryPlatform {
        match self.platform.as_str() {
            "windows" => DiscoveryPlatform::Windows,
            _ => DiscoveryPlatform::Linux,
        }
    }
}

/// A listener observed on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedListener {
    pub address: String,
    pub port: u16,
    pub protocol: String,
    pub owner: Option<ObservedOwner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedOwner {
    pub kind: IdentifierKind,
    pub identifier: String,
}

/// A container observed on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedContainer {
    pub image: String,
    pub name: String,
    pub published_ports: Vec<u16>,
}

/// Parsed, bounded host evidence.
#[derive(Debug, Clone)]
pub struct HostEvidence {
    pub platform: DiscoveryPlatform,
    pub skipped: bool,
    pub listeners: Vec<ObservedListener>,
    pub services: Vec<String>,
    pub processes: Vec<String>,
    pub containers: Vec<ObservedContainer>,
    pub listeners_complete: bool,
    /// A truncated or unparseable listener list can never prove absence.
    pub listeners_degraded: bool,
    pub reasons: BTreeSet<String>,
}

impl HostEvidence {
    /// Evidence for a host the run was told to skip. A skipped host never
    /// proves absence.
    #[must_use]
    pub fn skipped(platform: DiscoveryPlatform) -> Self {
        Self {
            platform,
            skipped: true,
            listeners: Vec::new(),
            services: Vec::new(),
            processes: Vec::new(),
            containers: Vec::new(),
            listeners_complete: false,
            listeners_degraded: true,
            reasons: BTreeSet::from([reason::SOURCE_DISABLED.to_string()]),
        }
    }

    /// Parse the collector artifact into structured evidence.
    pub fn parse(artifact: &DiscoveryCollectionArtifact) -> Result<Self, AppError> {
        artifact.validate()?;
        let platform = artifact.platform();
        let mut reasons = BTreeSet::new();
        if artifact.skipped {
            return Ok(Self::skipped(platform));
        }
        let evidence = match platform {
            DiscoveryPlatform::Linux => parse_linux(artifact, &mut reasons)?,
            DiscoveryPlatform::Windows => parse_windows(artifact, &mut reasons)?,
        };
        Ok(evidence)
    }
}

fn parse_linux(
    artifact: &DiscoveryCollectionArtifact,
    reasons: &mut BTreeSet<String>,
) -> Result<HostEvidence, AppError> {
    let raw = artifact.raw.as_object().ok_or_else(|| {
        AppError::Validation("discovery collection raw evidence must be an object".to_string())
    })?;
    let listeners_raw = raw_string(raw.get("listeners"));
    let services_raw = raw_string(raw.get("services"));
    let processes_raw = raw_string(raw.get("processes"));
    let containers_raw = raw_string(raw.get("containers"));

    let listener_lines = bounded_lines(&listeners_raw, DISCOVERY_MAX_RAW_LISTENER_LINES);
    let listeners_truncated = listener_lines.len() >= DISCOVERY_MAX_RAW_LISTENER_LINES;
    if listeners_truncated {
        reasons.insert(reason::PARTIAL_INSPECTION.to_string());
    }
    let mut listeners = Vec::new();
    let mut listener_parse_failed = false;
    for line in &listener_lines {
        match parse_ss_listener(line) {
            Some(listener) => listeners.push(listener),
            None => {
                if !line.trim().is_empty() {
                    listener_parse_failed = true;
                    reasons.insert(reason::PARSE_FAILED.to_string());
                }
            }
        }
    }
    let services = bounded_lines(&services_raw, DISCOVERY_MAX_RAW_SERVICE_LINES)
        .into_iter()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .filter(|name| name.ends_with(".service"))
        .collect::<Vec<_>>();
    let processes = bounded_lines(&processes_raw, DISCOVERY_MAX_RAW_PROCESS_LINES)
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let containers = parse_docker_lines(&containers_raw, reasons);

    Ok(HostEvidence {
        platform: DiscoveryPlatform::Linux,
        skipped: false,
        listeners,
        services,
        processes,
        containers,
        listeners_complete: artifact.coverage.listeners_complete(),
        listeners_degraded: listeners_truncated || listener_parse_failed,
        reasons: reasons.clone(),
    })
}

fn parse_windows(
    artifact: &DiscoveryCollectionArtifact,
    reasons: &mut BTreeSet<String>,
) -> Result<HostEvidence, AppError> {
    let raw = artifact.raw.as_object().ok_or_else(|| {
        AppError::Validation("discovery collection raw evidence must be an object".to_string())
    })?;
    let processes = raw
        .get("processes")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut process_names_by_id = BTreeMap::<i64, String>::new();
    if let Some(items) = raw.get("processes").and_then(serde_json::Value::as_array) {
        for item in items {
            if let (Some(id), Some(name)) = (
                item.get("id").and_then(serde_json::Value::as_i64),
                item.get("name").and_then(serde_json::Value::as_str),
            ) {
                process_names_by_id.insert(id, name.to_string());
            }
        }
    }
    let mut listeners = Vec::new();
    let mut listeners_degraded = false;
    match raw.get("listeners") {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                let Some(address) = item.get("address").and_then(serde_json::Value::as_str) else {
                    listeners_degraded = true;
                    reasons.insert(reason::PARSE_FAILED.to_string());
                    continue;
                };
                let Some(port) = item
                    .get("port")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|port| u16::try_from(port).ok())
                else {
                    listeners_degraded = true;
                    reasons.insert(reason::PARSE_FAILED.to_string());
                    continue;
                };
                let owner = item
                    .get("process_id")
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|id| process_names_by_id.get(&id))
                    .map(|name| ObservedOwner {
                        kind: IdentifierKind::WindowsExecutable,
                        identifier: name.clone(),
                    });
                listeners.push(ObservedListener {
                    address: address.to_string(),
                    port,
                    protocol: "tcp".to_string(),
                    owner,
                });
            }
        }
        Some(_) => {
            listeners_degraded = true;
            reasons.insert(reason::PARSE_FAILED.to_string());
        }
        None => listeners_degraded = true,
    }
    let services = raw
        .get("services")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let containers = raw
        .get("containers")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(ObservedContainer {
                        image: item
                            .get("Image")
                            .and_then(serde_json::Value::as_str)?
                            .to_string(),
                        name: item
                            .get("Names")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        published_ports: Vec::new(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(HostEvidence {
        platform: DiscoveryPlatform::Windows,
        skipped: false,
        listeners,
        services,
        processes,
        containers,
        listeners_complete: artifact.coverage.listeners_complete(),
        listeners_degraded,
        reasons: reasons.clone(),
    })
}

fn is_rfc3339_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn raw_string(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn bounded_lines(value: &str, cap: usize) -> Vec<String> {
    value
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .take(cap)
        .map(str::to_string)
        .collect()
}

/// Parse one `ss -H -lntup` line. The owner block is optional because an
/// unprivileged inspection cannot see other users' processes.
#[must_use]
pub fn parse_ss_listener(line: &str) -> Option<ObservedListener> {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    let protocol = parts[0];
    if !matches!(protocol, "tcp" | "tcp6" | "udp" | "udp6") {
        return None;
    }
    let local = parts[4];
    let (address, port) = split_socket_address(local)?;
    let owner = parse_ss_owner(parts.get(6..).unwrap_or_default().join(" ").as_str());
    Some(ObservedListener {
        address,
        port,
        protocol: if protocol.starts_with("tcp") {
            "tcp".to_string()
        } else {
            "udp".to_string()
        },
        owner,
    })
}

fn split_socket_address(value: &str) -> Option<(String, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (address, tail) = rest.split_once("]:")?;
        return Some((address.to_string(), tail.parse().ok()?));
    }
    let (address, port) = value.rsplit_once(':')?;
    Some((address.to_string(), port.parse().ok()?))
}

fn parse_ss_owner(process: &str) -> Option<ObservedOwner> {
    let marker = "users:((";
    let start = process.find(marker)? + marker.len();
    let rest = process[start..].strip_prefix('"')?;
    let end = rest.find('"')?;
    let identifier = rest[..end].trim();
    if identifier.is_empty() {
        return None;
    }
    Some(ObservedOwner {
        kind: IdentifierKind::ProcessExecutable,
        identifier: identifier.to_string(),
    })
}

fn parse_docker_lines(raw: &str, reasons: &mut BTreeSet<String>) -> Vec<ObservedContainer> {
    bounded_lines(raw, DISCOVERY_MAX_RAW_CONTAINER_LINES)
        .into_iter()
        .filter_map(|line| {
            let mut parts = line.split('|');
            let image = parts.next().unwrap_or_default().trim();
            let name = parts.next().unwrap_or_default().trim();
            let ports = parts.next().unwrap_or_default();
            if image.is_empty() {
                reasons.insert(reason::PARSE_FAILED.to_string());
                return None;
            }
            Some(ObservedContainer {
                image: image.to_string(),
                name: name.to_string(),
                published_ports: published_host_ports(ports),
            })
        })
        .collect()
}

/// Extract published host ports from a `docker ps` ports string such as
/// `0.0.0.0:9100->9100/tcp, :::9100->9100/tcp`.
#[must_use]
pub fn published_host_ports(ports: &str) -> Vec<u16> {
    ports
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            let (host, _) = entry.split_once("->")?;
            let (_, port) = host.rsplit_once(':')?;
            port.trim().parse::<u16>().ok()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Owner evidence for a listener: a directly visible process/unit owner wins,
/// otherwise a container publishing that host port is the machine owner.
fn effective_owner<'a>(
    evidence: &'a HostEvidence,
    listener: &'a ObservedListener,
) -> Option<ObservedOwner> {
    if let Some(owner) = listener.owner.as_ref() {
        return Some(owner.clone());
    }
    evidence
        .containers
        .iter()
        .find(|container| container.published_ports.contains(&listener.port))
        .map(|container| ObservedOwner {
            kind: IdentifierKind::ContainerImage,
            identifier: container.image.clone(),
        })
}

/// One endpoint declared by the monitoring owner (assignment or extra endpoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredEndpoint {
    pub endpoint: EndpointIdentity,
    pub exporter_type: Option<String>,
    pub labels: BTreeMap<String, String>,
}

/// Outcome of resolving one host's evidence.
#[derive(Debug, Clone)]
pub struct HostResolution {
    pub observations: Vec<EndpointObservation>,
    pub truncated: bool,
}

/// Resolve declared endpoints and discovered listeners into contract
/// observations for one host.
///
/// Absence is only produced from a complete listener inspection with an
/// address that is observable from the host. A partial inspection, an excluded
/// host or a truncated set can never prove absence.
pub fn resolve_host_inspection(
    evidence: &HostEvidence,
    declared: &[DeclaredEndpoint],
    observable_addresses: &[String],
    excluded_ports: &[u16],
    provenance: &ObservationProvenance,
    observed_at: &str,
    detect_exporter_type: bool,
    max_endpoints_per_host: usize,
) -> HostResolution {
    let mut observations = Vec::new();
    let mut truncated = false;
    let mut seen = BTreeSet::<(String, u16, String, String)>::new();

    let listeners_complete = evidence.listeners_complete && !evidence.listeners_degraded;
    if evidence.skipped {
        for declared_endpoint in declared.iter().take(max_endpoints_per_host) {
            observations.push(host_observation(
                declared_endpoint,
                evidence,
                DiscoveryStatus::Skipped,
                reason::SOURCE_DISABLED,
                provenance,
                observed_at,
                None,
            ));
        }
        return HostResolution {
            observations,
            truncated: declared.len() > max_endpoints_per_host,
        };
    }

    for declared_endpoint in declared.iter().take(max_endpoints_per_host) {
        if seen.contains(&identity_key(&declared_endpoint.endpoint)) {
            continue;
        }
        seen.insert(identity_key(&declared_endpoint.endpoint));
        if excluded_ports.contains(&declared_endpoint.endpoint.port) {
            observations.push(host_observation(
                declared_endpoint,
                evidence,
                DiscoveryStatus::Skipped,
                reason::OUT_OF_SCOPE,
                provenance,
                observed_at,
                None,
            ));
            continue;
        }
        let listener = find_listener(evidence, &declared_endpoint.endpoint);
        match listener {
            Some(listener) => {
                let (status, reason_code, origin, owner) =
                    listener_status(evidence, listener, detect_exporter_type);
                observations.push(host_observation(
                    declared_endpoint,
                    evidence,
                    status,
                    reason_code,
                    provenance,
                    observed_at,
                    owner.map(|owner| (origin, owner)),
                ));
            }
            None if !listeners_complete => {
                observations.push(host_observation(
                    declared_endpoint,
                    evidence,
                    DiscoveryStatus::Error,
                    reason::PARTIAL_INSPECTION,
                    provenance,
                    observed_at,
                    None,
                ));
            }
            None if !address_is_observable(
                &declared_endpoint.endpoint.address,
                observable_addresses,
            ) =>
            {
                observations.push(host_observation(
                    declared_endpoint,
                    evidence,
                    DiscoveryStatus::Error,
                    reason::ADDRESS_NOT_OBSERVABLE,
                    provenance,
                    observed_at,
                    None,
                ));
            }
            None => {
                observations.push(host_observation(
                    declared_endpoint,
                    evidence,
                    DiscoveryStatus::ConfirmedAbsent,
                    reason::NO_LISTENER,
                    provenance,
                    observed_at,
                    None,
                ));
            }
        }
    }

    if declared.len() > max_endpoints_per_host {
        truncated = true;
    }

    // Discovered listeners that are not declared assignments are still evidence.
    let primary = observable_addresses
        .iter()
        .find(|address| !address.trim().is_empty())
        .cloned();
    for listener in &evidence.listeners {
        if listener.protocol != "tcp" || excluded_ports.contains(&listener.port) {
            continue;
        }
        if observations.len() >= max_endpoints_per_host {
            truncated = true;
            break;
        }
        if !is_wildcard_address(&listener.address) && is_loopback_or_link_local(&listener.address) {
            continue;
        }
        let address = if is_wildcard_address(&listener.address) {
            match &primary {
                Some(primary) => primary.clone(),
                None => continue,
            }
        } else if address_is_observable(&listener.address, observable_addresses) {
            listener.address.clone()
        } else {
            continue;
        };
        let Ok(endpoint) = EndpointIdentity::validated(
            provenance.source_id,
            &address,
            listener.port,
            "http",
            "/metrics",
        ) else {
            continue;
        };
        if seen.contains(&identity_key(&endpoint)) {
            continue;
        }
        seen.insert(identity_key(&endpoint));
        let declared_endpoint = DeclaredEndpoint {
            endpoint,
            exporter_type: None,
            labels: BTreeMap::new(),
        };
        let (status, reason_code, origin, owner) =
            listener_status(evidence, listener, detect_exporter_type);
        observations.push(host_observation(
            &declared_endpoint,
            evidence,
            status,
            reason_code,
            provenance,
            observed_at,
            owner.map(|owner| (origin, owner)),
        ));
    }

    HostResolution {
        observations,
        truncated,
    }
}

fn listener_status(
    evidence: &HostEvidence,
    listener: &ObservedListener,
    detect_exporter_type: bool,
) -> (
    DiscoveryStatus,
    &'static str,
    DiscoveryOrigin,
    Option<ObservedOwner>,
) {
    let Some(owner) = effective_owner(evidence, listener) else {
        return (
            DiscoveryStatus::Unverified,
            reason::UNRECOGNIZED_OWNER,
            DiscoveryOrigin::HostListener,
            None,
        );
    };
    if detect_exporter_type
        && resolve_exporter_type(evidence.platform, owner.kind, &owner.identifier).is_some()
    {
        return (
            DiscoveryStatus::ConfirmedDiscovered,
            reason::TYPED_LISTENER_OWNER,
            owner_origin(owner.kind),
            Some(owner),
        );
    }
    (
        DiscoveryStatus::Unverified,
        reason::UNRECOGNIZED_OWNER,
        owner_origin(owner.kind),
        Some(owner),
    )
}

fn owner_origin(kind: IdentifierKind) -> DiscoveryOrigin {
    match kind {
        IdentifierKind::SystemdUnit => DiscoveryOrigin::HostService,
        IdentifierKind::WindowsService => DiscoveryOrigin::HostService,
        IdentifierKind::ContainerImage => DiscoveryOrigin::HostContainer,
        IdentifierKind::ProcessExecutable | IdentifierKind::WindowsExecutable => {
            DiscoveryOrigin::HostProcess
        }
    }
}

fn host_observation(
    declared: &DeclaredEndpoint,
    evidence: &HostEvidence,
    status: DiscoveryStatus,
    reason_code: &str,
    provenance: &ObservationProvenance,
    observed_at: &str,
    owner: Option<(DiscoveryOrigin, ObservedOwner)>,
) -> EndpointObservation {
    let listener = evidence
        .listeners
        .iter()
        .find(|listener| find_listener_matches(listener, &declared.endpoint));
    let (origin, exporter_type) = match &owner {
        Some((origin, observed_owner)) => (
            *origin,
            resolve_exporter_type(
                evidence.platform,
                observed_owner.kind,
                &observed_owner.identifier,
            )
            .map(str::to_string),
        ),
        None => (
            DiscoveryOrigin::HostListener,
            declared.exporter_type.clone(),
        ),
    };
    EndpointObservation {
        endpoint: declared.endpoint.clone(),
        source: DiscoverySource::HostInspection,
        platform: Some(evidence.platform),
        observed_at: observed_at.to_string(),
        provenance: provenance.clone(),
        proof: DiscoveryProof {
            origin: origin.as_str().to_string(),
            listener: listener.map(|listener| ListenerProof {
                address: listener.address.clone(),
                port: listener.port,
                protocol: listener.protocol.clone(),
            }),
            owner: owner.map(|(_, observed_owner)| OwnerProof {
                kind: observed_owner.kind.as_str().to_string(),
                identifier: observed_owner.identifier.clone(),
                exporter_type: resolve_exporter_type(
                    evidence.platform,
                    observed_owner.kind,
                    &observed_owner.identifier,
                )
                .map(str::to_string),
            }),
            coverage: Some(
                if evidence.listeners_complete && !evidence.listeners_degraded {
                    "complete".to_string()
                } else {
                    "partial".to_string()
                },
            ),
            connect: None,
        },
        status,
        reason: reason_code.to_string(),
        exporter_type: exporter_type.or_else(|| declared.exporter_type.clone()),
        labels: declared.labels.clone(),
    }
}

/// Probe observation used by the endpoint-probe source.
#[must_use]
pub fn probe_observation(
    endpoint: &EndpointIdentity,
    provenance: &ObservationProvenance,
    observed_at: &str,
    status: DiscoveryStatus,
    reason_code: &str,
    connect: ConnectProof,
    exporter_type: Option<String>,
    labels: BTreeMap<String, String>,
) -> EndpointObservation {
    EndpointObservation {
        endpoint: endpoint.clone(),
        source: DiscoverySource::EndpointProbe,
        platform: None,
        observed_at: observed_at.to_string(),
        provenance: provenance.clone(),
        proof: DiscoveryProof {
            origin: DiscoveryOrigin::EndpointHttp.as_str().to_string(),
            listener: None,
            owner: None,
            coverage: None,
            connect: Some(connect),
        },
        status,
        reason: reason_code.to_string(),
        exporter_type,
        labels,
    }
}

fn find_listener<'a>(
    evidence: &'a HostEvidence,
    endpoint: &EndpointIdentity,
) -> Option<&'a ObservedListener> {
    evidence
        .listeners
        .iter()
        .find(|listener| find_listener_matches(listener, endpoint))
}

fn find_listener_matches(listener: &ObservedListener, endpoint: &EndpointIdentity) -> bool {
    listener.protocol == "tcp"
        && listener.port == endpoint.port
        && (is_wildcard_address(&listener.address) || listener.address == endpoint.address)
}

#[must_use]
pub fn is_wildcard_address(address: &str) -> bool {
    matches!(address, "0.0.0.0" | "::" | "*" | "[::]" | "")
}

#[must_use]
pub fn is_loopback_or_link_local(address: &str) -> bool {
    if address == "::1" || address.starts_with("127.") {
        return true;
    }
    if let Ok(ip) = address.parse::<std::net::IpAddr>() {
        return ip.is_loopback()
            || ip.is_unspecified()
            || match ip {
                std::net::IpAddr::V4(v4) => v4.is_link_local(),
                std::net::IpAddr::V6(v6) => v6.is_unicast_link_local(),
            };
    }
    false
}

fn address_is_observable(address: &str, observable_addresses: &[String]) -> bool {
    observable_addresses
        .iter()
        .any(|candidate| candidate == address)
}

fn identity_key(endpoint: &EndpointIdentity) -> (String, u16, String, String) {
    (
        endpoint.address.clone(),
        endpoint.port,
        endpoint.path.clone(),
        endpoint.scheme.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> ObservationProvenance {
        ObservationProvenance {
            run_id: Uuid::from_u128(10),
            run_group_id: Uuid::from_u128(11),
            connection_target_id: Uuid::from_u128(12),
            source_kind: "virtual_machine".to_string(),
            source_id: Uuid::from_u128(12),
            observed_from: "runner".to_string(),
        }
    }

    fn declared(port: u16, exporter_type: Option<&str>) -> DeclaredEndpoint {
        DeclaredEndpoint {
            endpoint: EndpointIdentity::validated(
                Uuid::from_u128(12),
                "10.0.0.5",
                port,
                "http",
                "/metrics",
            )
            .unwrap(),
            exporter_type: exporter_type.map(str::to_string),
            labels: BTreeMap::new(),
        }
    }

    fn linux_evidence(listeners: &str, listeners_complete: bool) -> HostEvidence {
        let artifact: DiscoveryCollectionArtifact = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": Uuid::from_u128(10),
            "run_group_id": Uuid::from_u128(11),
            "connection_target_id": Uuid::from_u128(12),
            "host_id": Uuid::from_u128(12),
            "host_name": "db-01",
            "source_kind": "virtual_machine",
            "platform": "linux",
            "started_at": "2026-11-10T00:00:00Z",
            "finished_at": "2026-11-10T00:00:05Z",
            "sources": {"host_inspection": true, "endpoint_probe": true},
            "coverage": {
                "listeners": if listeners_complete { "complete" } else { "partial" },
                "services": "complete",
                "processes": "complete",
                "containers": "not_installed",
                "listener_owners": "complete"
            },
            "raw": {"listeners": listeners, "services": "", "processes": "", "containers": ""}
        }))
        .unwrap();
        HostEvidence::parse(&artifact).unwrap()
    }

    #[test]
    fn ss_listener_parsing_keeps_owner_and_protocol() {
        let listener = parse_ss_listener(
            r#"tcp   LISTEN 0      4096   0.0.0.0:9100   0.0.0.0:*    users:(("node_exporter",pid=812,fd=3))"#,
        )
        .unwrap();
        assert_eq!(listener.port, 9100);
        assert_eq!(listener.protocol, "tcp");
        assert_eq!(listener.owner.as_ref().unwrap().identifier, "node_exporter");
        let v6 = parse_ss_listener(
            r#"tcp   LISTEN 0      4096   [::]:9100   [::]:*    users:(("node_exporter",pid=812,fd=4))"#,
        )
        .unwrap();
        assert_eq!(v6.address, "::");
        assert_eq!(v6.port, 9100);
    }

    #[test]
    fn complete_inspection_proves_absence_only_with_an_observable_address() {
        let evidence = linux_evidence("", true);
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(
            resolution.observations[0].status,
            DiscoveryStatus::ConfirmedAbsent
        );
        assert_eq!(resolution.observations[0].reason, reason::NO_LISTENER);

        let unobservable = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.99".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(unobservable.observations[0].status, DiscoveryStatus::Error);
        assert_eq!(
            unobservable.observations[0].reason,
            reason::ADDRESS_NOT_OBSERVABLE
        );
    }

    #[test]
    fn partial_inspection_never_proves_absence() {
        let evidence = linux_evidence("", false);
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(resolution.observations[0].status, DiscoveryStatus::Error);
        assert_eq!(
            resolution.observations[0].reason,
            reason::PARTIAL_INSPECTION
        );
    }

    #[test]
    fn new_wildcard_listeners_use_observable_address_but_loopback_is_excluded() {
        for listener_address in ["0.0.0.0", "[::]"] {
            let listener = format!(
                r#"tcp LISTEN 0 4096 {listener_address}:9100 0.0.0.0:* users:(("node_exporter",pid=1,fd=3))"#
            );
            let evidence = linux_evidence(&listener, true);
            let resolution = resolve_host_inspection(
                &evidence,
                &[],
                &["192.0.2.10".to_string()],
                &[],
                &provenance(),
                "2026-11-10T00:00:05Z",
                true,
                64,
            );
            assert_eq!(resolution.observations.len(), 1);
            let observation = &resolution.observations[0];
            assert_eq!(observation.endpoint.address, "192.0.2.10");
            assert_eq!(observation.status, DiscoveryStatus::ConfirmedDiscovered);
            assert_eq!(observation.exporter_type.as_deref(), Some("node"));
        }
        let evidence = linux_evidence(
            r#"tcp LISTEN 0 4096 127.0.0.1:9100 0.0.0.0:* users:(("node_exporter",pid=1,fd=3))"#,
            true,
        );
        let resolution = resolve_host_inspection(
            &evidence,
            &[],
            &["192.0.2.10".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert!(resolution.observations.is_empty());
    }

    #[test]
    fn typed_listener_owner_confirms_discovery_and_untyped_stays_unverified() {
        let typed = linux_evidence(
            r#"tcp LISTEN 0 4096 0.0.0.0:9100 0.0.0.0:* users:(("node_exporter",pid=1,fd=3))"#,
            true,
        );
        let resolution = resolve_host_inspection(
            &typed,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(
            resolution.observations[0].status,
            DiscoveryStatus::ConfirmedDiscovered
        );
        assert_eq!(
            resolution.observations[0].exporter_type.as_deref(),
            Some("node")
        );

        let untyped = linux_evidence(
            r#"tcp LISTEN 0 4096 0.0.0.0:9100 0.0.0.0:* users:(("mystery",pid=1,fd=3))"#,
            true,
        );
        let resolution = resolve_host_inspection(
            &untyped,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(
            resolution.observations[0].status,
            DiscoveryStatus::Unverified
        );
        assert_eq!(resolution.observations[0].exporter_type, None);
    }

    #[test]
    fn excluded_ports_skip_instead_of_proving_absence() {
        let evidence = linux_evidence("", true);
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[9100],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(resolution.observations[0].status, DiscoveryStatus::Skipped);
        assert_eq!(resolution.observations[0].reason, reason::OUT_OF_SCOPE);
    }

    #[test]
    fn skipped_host_reports_source_disabled_not_absence() {
        let artifact: DiscoveryCollectionArtifact = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": Uuid::from_u128(10),
            "run_group_id": Uuid::from_u128(11),
            "connection_target_id": Uuid::from_u128(12),
            "host_id": Uuid::from_u128(12),
            "host_name": "db-01",
            "source_kind": "virtual_machine",
            "platform": "linux",
            "started_at": "2026-11-10T00:00:00Z",
            "finished_at": "2026-11-10T00:00:05Z",
            "skipped": true,
            "sources": {"host_inspection": true, "endpoint_probe": true},
            "coverage": {
                "listeners": "partial",
                "services": "partial",
                "processes": "partial",
                "containers": "skipped",
                "listener_owners": "skipped"
            },
            "raw": {}
        }))
        .unwrap();
        let evidence = HostEvidence::parse(&artifact).unwrap();
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(resolution.observations[0].status, DiscoveryStatus::Skipped);
        assert_eq!(resolution.observations[0].reason, reason::SOURCE_DISABLED);
    }

    #[test]
    fn windows_listener_resolves_owner_from_process_table() {
        let artifact: DiscoveryCollectionArtifact = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": Uuid::from_u128(10),
            "run_group_id": Uuid::from_u128(11),
            "connection_target_id": Uuid::from_u128(12),
            "host_id": Uuid::from_u128(12),
            "host_name": "win-01",
            "source_kind": "device",
            "platform": "windows",
            "started_at": "2026-11-10T00:00:00Z",
            "finished_at": "2026-11-10T00:00:05Z",
            "sources": {"host_inspection": true, "endpoint_probe": true},
            "coverage": {
                "listeners": "complete",
                "services": "complete",
                "processes": "complete",
                "containers": "not_installed",
                "listener_owners": "complete"
            },
            "raw": {
                "listeners": [{"address": "0.0.0.0", "port": 9182, "process_id": 44}],
                "processes": [{"id": 44, "name": "windows_exporter"}],
                "services": [{"name": "windows_exporter", "state": "Running"}],
                "containers": []
            }
        }))
        .unwrap();
        let evidence = HostEvidence::parse(&artifact).unwrap();
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9182, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(
            resolution.observations[0].status,
            DiscoveryStatus::ConfirmedDiscovered
        );
        assert_eq!(
            resolution.observations[0].exporter_type.as_deref(),
            Some("windows")
        );
    }

    #[test]
    fn windows_unparseable_listener_evidence_cannot_prove_absence() {
        let artifact: DiscoveryCollectionArtifact = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": uuid::Uuid::from_u128(10),
            "run_group_id": uuid::Uuid::from_u128(11),
            "connection_target_id": uuid::Uuid::from_u128(12),
            "host_id": uuid::Uuid::from_u128(12),
            "host_name": "win-01",
            "source_kind": "device",
            "platform": "windows",
            "started_at": "2026-11-10T00:00:00Z",
            "finished_at": "2026-11-10T00:00:05Z",
            "sources": {"host_inspection": true, "endpoint_probe": true},
            "coverage": {
                "listeners": "complete",
                "services": "complete",
                "processes": "complete",
                "containers": "not_installed",
                "listener_owners": "complete"
            },
            "raw": {
                "listeners": [{"address": "0.0.0.0", "port": "9100", "process_id": 4}],
                "processes": [{"id": 4, "name": "windows_exporter"}],
                "services": [],
                "containers": []
            }
        }))
        .unwrap();
        let evidence = HostEvidence::parse(&artifact).unwrap();
        assert!(evidence.listeners_degraded);
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(resolution.observations[0].status, DiscoveryStatus::Error);
        assert_eq!(
            resolution.observations[0].reason,
            reason::PARTIAL_INSPECTION
        );
    }

    #[test]
    fn docker_ps_lines_keep_image_name_and_published_ports_only() {
        let mut reasons = BTreeSet::new();
        let containers = parse_docker_lines(
            "prom/node-exporter|node-exporter|0.0.0.0:9100->9100/tcp, :::9100->9100/tcp\n\
             redis:7|redis||",
            &mut reasons,
        );
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].image, "prom/node-exporter");
        assert_eq!(containers[0].published_ports, vec![9100]);
        assert_eq!(containers[1].image, "redis:7");
        assert!(containers[1].published_ports.is_empty());
        assert!(reasons.is_empty());
    }

    #[test]
    fn container_published_port_attributes_a_typed_owner() {
        let evidence = linux_evidence_with_containers(
            r#"tcp LISTEN 0 4096 0.0.0.0:9100 0.0.0.0:*"#,
            "prom/node-exporter|node-exporter|0.0.0.0:9100->9100/tcp",
        );
        let resolution = resolve_host_inspection(
            &evidence,
            &[declared(9100, None)],
            &["10.0.0.5".to_string()],
            &[],
            &provenance(),
            "2026-11-10T00:00:05Z",
            true,
            64,
        );
        assert_eq!(
            resolution.observations[0].status,
            DiscoveryStatus::ConfirmedDiscovered
        );
        assert_eq!(
            resolution.observations[0].exporter_type.as_deref(),
            Some("node")
        );
        assert_eq!(
            resolution.observations[0].proof.origin,
            DiscoveryOrigin::HostContainer.as_str()
        );
    }

    fn linux_evidence_with_containers(listeners: &str, containers: &str) -> HostEvidence {
        let artifact: DiscoveryCollectionArtifact = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": Uuid::from_u128(10),
            "run_group_id": Uuid::from_u128(11),
            "connection_target_id": Uuid::from_u128(12),
            "host_id": Uuid::from_u128(12),
            "host_name": "db-01",
            "source_kind": "virtual_machine",
            "platform": "linux",
            "started_at": "2026-11-10T00:00:00Z",
            "finished_at": "2026-11-10T00:00:05Z",
            "sources": {"host_inspection": true, "endpoint_probe": true},
            "coverage": {
                "listeners": "complete",
                "services": "complete",
                "processes": "complete",
                "containers": "complete",
                "listener_owners": "partial"
            },
            "raw": {
                "listeners": listeners,
                "services": "",
                "processes": "",
                "containers": containers
            }
        }))
        .unwrap();
        HostEvidence::parse(&artifact).unwrap()
    }
}
