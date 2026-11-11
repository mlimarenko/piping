//! Exporter discovery result contract: the closed status/reason vocabulary,
//! endpoint identity, the typed exporter-signature association and the report
//! envelope. Pure logic, no I/O.
//!
//! The shipped contract is `apps/api/contracts/exporter-discovery.md`. This
//! module owns its vocabulary; collectors, probes and the service-discovery
//! policy consume it instead of inventing parallel status strings.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

/// Schema version of the discovery report artifact.
pub const DISCOVERY_REPORT_SCHEMA_VERSION: u32 = 1;
/// Artifact type the monitoring owner writes after resolving the contract.
pub const DISCOVERY_REPORT_ARTIFACT_TYPE: &str = "exporter-discovery-report";

/// The report is bounded like every other run artifact.
pub const DISCOVERY_MAX_REPORT_ENDPOINTS: usize = 50_000;
pub const DISCOVERY_MAX_ADDRESS_CHARS: usize = 255;
pub const DISCOVERY_MAX_PATH_CHARS: usize = 255;
pub const DISCOVERY_MAX_REASON_CHARS: usize = 64;
pub const DISCOVERY_MAX_LABEL_KEY_CHARS: usize = 128;
pub const DISCOVERY_MAX_LABEL_VALUE_CHARS: usize = 255;
pub const DISCOVERY_MAX_LABELS: usize = 50;
pub const DISCOVERY_MAX_GROUP_REASONS: usize = 32;
pub const DISCOVERY_MAX_OWNER_IDENTIFIER_CHARS: usize = 255;

/// Which evidence source produced an observation. The two sources are enabled
/// independently.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    HostInspection,
    EndpointProbe,
}

impl DiscoverySource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostInspection => "host_inspection",
            Self::EndpointProbe => "endpoint_probe",
        }
    }
}

/// Closed observation status vocabulary from the shipped contract.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStatus {
    ConfirmedDiscovered,
    Unverified,
    Unreachable,
    ConfirmedAbsent,
    Skipped,
    Error,
}

impl DiscoveryStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedDiscovered => "confirmed_discovered",
            Self::Unverified => "unverified",
            Self::Unreachable => "unreachable",
            Self::ConfirmedAbsent => "confirmed_absent",
            Self::Skipped => "skipped",
            Self::Error => "error",
        }
    }

    /// Whether this status can prove positive absence. Only a complete host
    /// inspection may produce it; a probe never does.
    #[must_use]
    pub fn proves_absence(self) -> bool {
        self == Self::ConfirmedAbsent
    }

    /// `error` and `skipped` make a group partial, which blocks absence.
    #[must_use]
    pub fn makes_group_partial(self) -> bool {
        matches!(self, Self::Error | Self::Skipped)
    }
}

impl fmt::Display for DiscoveryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Platform of a host inspection. A probe has no platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryPlatform {
    Linux,
    Windows,
}

impl DiscoveryPlatform {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

/// Structured proof origin. Free-form evidence text is never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryOrigin {
    ConfiguredAssignment,
    HostListener,
    HostProcess,
    HostService,
    HostContainer,
    EndpointHttp,
}

impl DiscoveryOrigin {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfiguredAssignment => "configured_assignment",
            Self::HostListener => "host_listener",
            Self::HostProcess => "host_process",
            Self::HostService => "host_service",
            Self::HostContainer => "host_container",
            Self::EndpointHttp => "endpoint_http",
        }
    }
}

/// Stable reason codes named by the contract. The vocabulary stays open to new
/// snake_case codes, but every observation must carry one.
pub mod reason {
    pub const NO_LISTENER: &str = "no_listener";
    pub const COMPLETE_INSPECTION: &str = "complete_inspection";
    pub const CONNECTION_REFUSED: &str = "connection_refused";
    pub const CONNECT_FAILED: &str = "connect_failed";
    pub const TIMEOUT: &str = "timeout";
    pub const TLS_VERIFICATION_FAILED: &str = "tls_verification_failed";
    pub const REDIRECT_NOT_FOLLOWED: &str = "redirect_not_followed";
    pub const UNRECOGNIZED_OWNER: &str = "unrecognized_owner";
    pub const NOT_METRICS_RESPONSE: &str = "not_metrics_response";
    pub const SOURCE_DISABLED: &str = "source_disabled";
    pub const UNSUPPORTED_PLATFORM: &str = "unsupported_platform";
    pub const OUT_OF_SCOPE: &str = "out_of_scope";
    pub const PERMISSION_DENIED: &str = "permission_denied";
    pub const INSPECTION_FAILED: &str = "inspection_failed";
    pub const PARSE_FAILED: &str = "parse_failed";
    pub const METRICS_RESPONSE: &str = "metrics_response";
    pub const TYPED_LISTENER_OWNER: &str = "typed_listener_owner";
    pub const DECLARED_ASSIGNMENT: &str = "declared_assignment";
    pub const PARTIAL_INSPECTION: &str = "partial_inspection";
    pub const ADDRESS_NOT_OBSERVABLE: &str = "address_not_observable";
    pub const PROBE_DISABLED: &str = "probe_disabled";
}

/// Whether a reason code is a stable snake_case token.
#[must_use]
pub fn is_valid_reason_code(value: &str) -> bool {
    if value.is_empty() || value.len() > DISCOVERY_MAX_REASON_CHARS {
        return false;
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// The exact endpoint identity from the contract. Display names never identify
/// an endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EndpointIdentity {
    pub host_id: Uuid,
    pub address: String,
    pub port: u16,
    pub scheme: String,
    pub path: String,
}

impl EndpointIdentity {
    /// Validate and normalize an identity: `http`/`https` scheme, a bounded
    /// address without userinfo or query, a 1..=65535 port and a leading-slash
    /// metrics path.
    pub fn validated(
        host_id: Uuid,
        address: &str,
        port: u16,
        scheme: &str,
        path: &str,
    ) -> Result<Self, AppError> {
        let identity = Self {
            host_id,
            address: address.trim().to_string(),
            port,
            scheme: scheme.trim().to_ascii_lowercase(),
            path: normalize_discovery_path(path),
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self.host_id.is_nil() {
            return Err(AppError::Validation(
                "discovery endpoint requires a host identity".to_string(),
            ));
        }
        if self.address.is_empty() || self.address.len() > DISCOVERY_MAX_ADDRESS_CHARS {
            return Err(AppError::Validation(
                "discovery endpoint address is empty or too long".to_string(),
            ));
        }
        if self
            .address
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '@' | '?' | '#' | '/' | '\\' | '"' | '\''))
        {
            return Err(AppError::Validation(
                "discovery endpoint address contains a forbidden character".to_string(),
            ));
        }
        if self.port == 0 {
            return Err(AppError::Validation(
                "discovery endpoint port must be between 1 and 65535".to_string(),
            ));
        }
        if self.scheme != "http" && self.scheme != "https" {
            return Err(AppError::Validation(
                "discovery endpoint scheme must be `http` or `https`".to_string(),
            ));
        }
        if !self.path.starts_with('/') || self.path.len() > DISCOVERY_MAX_PATH_CHARS {
            return Err(AppError::Validation(
                "discovery endpoint path must start with `/` and stay bounded".to_string(),
            ));
        }
        if self.path.chars().any(|ch| ch.is_whitespace() || ch == '?') {
            return Err(AppError::Validation(
                "discovery endpoint path must not contain whitespace or a query".to_string(),
            ));
        }
        Ok(())
    }

    /// Deterministic report order: identity tuple with numeric port order and
    /// path after port.
    #[must_use]
    pub fn sort_key(&self) -> (Uuid, String, u16, String, String) {
        (
            self.host_id,
            self.address.clone(),
            self.port,
            self.path.clone(),
            self.scheme.clone(),
        )
    }

    /// Scrape URL used by the probe. The address is validated to be a bare
    /// host or IP, never a URL with credentials.
    pub fn probe_url(&self) -> String {
        let host = if self.address.contains(':') && !self.address.starts_with('[') {
            format!("[{}]", self.address)
        } else {
            self.address.clone()
        };
        format!("{}://{}:{}{}", self.scheme, host, self.port, self.path)
    }
}

/// Normalize a metrics path with a leading slash; an empty path becomes the
/// catalog default `/metrics`.
#[must_use]
pub fn normalize_discovery_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        "/metrics".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Machine identifiers are the only evidence that can establish an exporter
/// type besides a declared catalog assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierKind {
    SystemdUnit,
    ProcessExecutable,
    ContainerImage,
    WindowsService,
    WindowsExecutable,
}

impl IdentifierKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemdUnit => "systemd_unit",
            Self::ProcessExecutable => "process_executable",
            Self::ContainerImage => "container_image",
            Self::WindowsService => "windows_service",
            Self::WindowsExecutable => "windows_executable",
        }
    }
}

/// One declared machine identifier to exporter type association. The table
/// contains protocol and model identifiers only: no product names, localized
/// descriptions or response-body keywords.
#[derive(Debug, Clone, Copy)]
pub struct ExporterSignature {
    pub platform: DiscoveryPlatform,
    pub kind: IdentifierKind,
    pub identifier: &'static str,
    pub exporter_type: &'static str,
}

/// Canonical typed association owned by the exporter catalog vocabulary.
pub const EXPORTER_SIGNATURES: &[ExporterSignature] = &[
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "node_exporter",
        exporter_type: "node",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ProcessExecutable,
        identifier: "node_exporter",
        exporter_type: "node",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "prom/node-exporter",
        exporter_type: "node",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Windows,
        kind: IdentifierKind::WindowsService,
        identifier: "windows_exporter",
        exporter_type: "windows",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Windows,
        kind: IdentifierKind::WindowsExecutable,
        identifier: "windows_exporter",
        exporter_type: "windows",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "gcr.io/cadvisor/cadvisor",
        exporter_type: "docker",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "google/cadvisor",
        exporter_type: "docker",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "nginx",
        exporter_type: "nginx",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "nginx/nginx-prometheus-exporter",
        exporter_type: "nginx",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "postgres_exporter",
        exporter_type: "postgres",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ProcessExecutable,
        identifier: "postgres_exporter",
        exporter_type: "postgres",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "prometheuscommunity/postgres-exporter",
        exporter_type: "postgres",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "redis_exporter",
        exporter_type: "redis",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "oliver006/redis_exporter",
        exporter_type: "redis",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "proxmox_exporter",
        exporter_type: "proxmox",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "prompve/prometheus-pve-exporter",
        exporter_type: "proxmox",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "smartctl_exporter",
        exporter_type: "smartctl",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "prometheuscommunity/smartctl-exporter",
        exporter_type: "smartctl",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "process_exporter",
        exporter_type: "process",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "ncabatoff/process-exporter",
        exporter_type: "process",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "gitlab_exporter",
        exporter_type: "gitlab",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "gitlab/gitlab-exporter",
        exporter_type: "gitlab",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::SystemdUnit,
        identifier: "blackbox_exporter",
        exporter_type: "blackbox",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ProcessExecutable,
        identifier: "blackbox_exporter",
        exporter_type: "blackbox",
    },
    ExporterSignature {
        platform: DiscoveryPlatform::Linux,
        kind: IdentifierKind::ContainerImage,
        identifier: "prom/blackbox-exporter",
        exporter_type: "blackbox",
    },
];

/// Normalize one machine identifier for association matching: basename, no
/// `.service`/`.exe` suffix, no image tag or digest, lowercase.
#[must_use]
pub fn normalize_machine_identifier(kind: IdentifierKind, identifier: &str) -> String {
    let trimmed = identifier.trim();
    let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let value = match kind {
        IdentifierKind::SystemdUnit => basename
            .strip_suffix(".service")
            .unwrap_or(basename)
            .to_string(),
        IdentifierKind::ProcessExecutable | IdentifierKind::WindowsExecutable => basename
            .strip_suffix(".exe")
            .unwrap_or(basename)
            .to_string(),
        IdentifierKind::WindowsService => basename.to_string(),
        IdentifierKind::ContainerImage => {
            // Drop a tag or digest, keep the repository path.
            let without_digest = trimmed.split('@').next().unwrap_or(trimmed);
            let last_segment = without_digest.rsplit('/').next().unwrap_or(without_digest);
            if let Some((_, tag)) = last_segment.rsplit_once(':') {
                let repository = without_digest
                    .strip_suffix(&format!(":{tag}"))
                    .unwrap_or(without_digest);
                repository.to_ascii_lowercase()
            } else {
                without_digest.to_ascii_lowercase()
            }
        }
    };
    value.to_ascii_lowercase()
}

/// Resolve an exporter type from declared machine identifiers only.
#[must_use]
pub fn resolve_exporter_type(
    platform: DiscoveryPlatform,
    kind: IdentifierKind,
    identifier: &str,
) -> Option<&'static str> {
    let normalized = normalize_machine_identifier(kind, identifier);
    if normalized.is_empty() {
        return None;
    }
    EXPORTER_SIGNATURES
        .iter()
        .find(|signature| {
            signature.platform == platform
                && signature.kind == kind
                && normalize_machine_identifier(kind, signature.identifier) == normalized
        })
        .map(|signature| signature.exporter_type)
}

/// Structured, bounded proof. Secret values, process arguments, environment
/// values, response bodies and URL userinfo never enter it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiscoveryProof {
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener: Option<ListenerProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectProof>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ListenerProof {
    pub address: String,
    pub port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OwnerProof {
    pub kind: String,
    pub identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exporter_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConnectProof {
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_verified: Option<bool>,
    pub duration_ms: u64,
}

/// One resolved observation of one endpoint by one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EndpointObservation {
    pub endpoint: EndpointIdentity,
    pub source: DiscoverySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<DiscoveryPlatform>,
    pub observed_at: String,
    pub provenance: ObservationProvenance,
    pub proof: DiscoveryProof,
    pub status: DiscoveryStatus,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exporter_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ObservationProvenance {
    pub run_id: Uuid,
    pub run_group_id: Uuid,
    pub connection_target_id: Uuid,
    pub source_kind: String,
    pub source_id: Uuid,
    pub observed_from: String,
}

impl EndpointObservation {
    /// Validate one observation's closed vocabulary and bounded proof.
    pub fn validate(&self) -> Result<(), AppError> {
        self.endpoint.validate()?;
        if !is_valid_reason_code(&self.reason) {
            return Err(AppError::Validation(
                "discovery observation reason must be a stable snake_case code".to_string(),
            ));
        }
        if self.status == DiscoveryStatus::ConfirmedAbsent
            && self.source != DiscoverySource::HostInspection
        {
            return Err(AppError::Validation(
                "only a complete host inspection may prove absence".to_string(),
            ));
        }
        if self.status == DiscoveryStatus::Unreachable
            && self.source != DiscoverySource::EndpointProbe
        {
            return Err(AppError::Validation(
                "unreachable is a probe-only status".to_string(),
            ));
        }
        if let Some(owner) = &self.proof.owner {
            if owner.identifier.is_empty()
                || owner.identifier.len() > DISCOVERY_MAX_OWNER_IDENTIFIER_CHARS
            {
                return Err(AppError::Validation(
                    "discovery proof owner identifier is empty or too long".to_string(),
                ));
            }
        }
        if self.labels.len() > DISCOVERY_MAX_LABELS {
            return Err(AppError::Validation(
                "discovery observation carries too many labels".to_string(),
            ));
        }
        for (key, value) in &self.labels {
            if key.is_empty()
                || key.len() > DISCOVERY_MAX_LABEL_KEY_CHARS
                || value.len() > DISCOVERY_MAX_LABEL_VALUE_CHARS
            {
                return Err(AppError::Validation(
                    "discovery observation label key or value is out of bounds".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Per-host outcome of one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiscoveryGroup {
    pub host_id: Uuid,
    pub host_name: String,
    pub kind: String,
    pub counts: BTreeMap<String, usize>,
    pub reasons: Vec<String>,
    pub partial: bool,
}

/// The report envelope from the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiscoveryReport {
    pub schema_version: u32,
    pub run_id: Uuid,
    pub run_group_id: Uuid,
    pub started_at: String,
    pub finished_at: String,
    pub sources: BTreeMap<String, bool>,
    pub endpoints: Vec<EndpointObservation>,
    pub groups: Vec<DiscoveryGroup>,
}

impl DiscoveryReport {
    /// Build the deterministic report envelope: groups by `host_name` then
    /// `host_id`, endpoints by identity tuple.
    pub fn build(
        run_id: Uuid,
        run_group_id: Uuid,
        started_at: String,
        finished_at: String,
        sources: BTreeMap<String, bool>,
        host_names: &BTreeMap<Uuid, (String, String)>,
        mut endpoints: Vec<EndpointObservation>,
    ) -> Self {
        endpoints.sort_by(|left, right| left.endpoint.sort_key().cmp(&right.endpoint.sort_key()));
        let groups = build_groups(host_names, &endpoints);
        Self {
            schema_version: DISCOVERY_REPORT_SCHEMA_VERSION,
            run_id,
            run_group_id,
            started_at,
            finished_at,
            sources,
            endpoints,
            groups,
        }
    }

    /// Validate the whole envelope. A truncated or inconsistent report is
    /// rejected instead of being treated as a complete inspection.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != DISCOVERY_REPORT_SCHEMA_VERSION {
            return Err(AppError::Validation(format!(
                "unsupported discovery report schema version {}",
                self.schema_version
            )));
        }
        if self.endpoints.len() > DISCOVERY_MAX_REPORT_ENDPOINTS {
            return Err(AppError::Validation(
                "discovery report exceeds the endpoint bound".to_string(),
            ));
        }
        for observation in &self.endpoints {
            observation.validate()?;
        }
        let mut expected = BTreeMap::<Uuid, BTreeMap<String, usize>>::new();
        let mut reasons = BTreeMap::<Uuid, BTreeSet<String>>::new();
        for observation in &self.endpoints {
            *expected
                .entry(observation.endpoint.host_id)
                .or_default()
                .entry(observation.status.as_str().to_string())
                .or_default() += 1;
            reasons
                .entry(observation.endpoint.host_id)
                .or_default()
                .insert(observation.reason.clone());
        }
        for group in &self.groups {
            let counts = expected.remove(&group.host_id).unwrap_or_default();
            if counts != group.counts {
                return Err(AppError::Validation(
                    "discovery group counts do not add up to the endpoints".to_string(),
                ));
            }
            if group.reasons.len() > DISCOVERY_MAX_GROUP_REASONS
                || group
                    .reasons
                    .iter()
                    .any(|reason| !is_valid_reason_code(reason))
            {
                return Err(AppError::Validation(
                    "discovery group carries an invalid reason set".to_string(),
                ));
            }
        }
        if !expected.is_empty() {
            return Err(AppError::Validation(
                "discovery report has endpoints without a group".to_string(),
            ));
        }
        Ok(())
    }
}

fn build_groups(
    host_names: &BTreeMap<Uuid, (String, String)>,
    endpoints: &[EndpointObservation],
) -> Vec<DiscoveryGroup> {
    let mut by_host = BTreeMap::<Uuid, Vec<&EndpointObservation>>::new();
    for observation in endpoints {
        by_host
            .entry(observation.endpoint.host_id)
            .or_default()
            .push(observation);
    }
    let mut groups = by_host
        .into_iter()
        .map(|(host_id, observations)| {
            let (host_name, kind) = host_names
                .get(&host_id)
                .cloned()
                .unwrap_or_else(|| (host_id.to_string(), "unknown".to_string()));
            let mut counts = BTreeMap::new();
            let mut reasons = BTreeSet::new();
            let mut partial = false;
            for observation in observations {
                *counts
                    .entry(observation.status.as_str().to_string())
                    .or_default() += 1;
                reasons.insert(observation.reason.clone());
                partial |= observation.status.makes_group_partial();
            }
            DiscoveryGroup {
                host_id,
                host_name,
                kind,
                counts,
                reasons: reasons.into_iter().collect(),
                partial,
            }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        left.host_name
            .cmp(&right.host_name)
            .then_with(|| left.host_id.cmp(&right.host_id))
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(status: DiscoveryStatus, reason: &str) -> EndpointObservation {
        EndpointObservation {
            endpoint: EndpointIdentity::validated(
                Uuid::from_u128(1),
                "10.0.0.5",
                9100,
                "http",
                "metrics",
            )
            .unwrap(),
            source: DiscoverySource::HostInspection,
            platform: Some(DiscoveryPlatform::Linux),
            observed_at: "2026-11-10T00:00:00Z".to_string(),
            provenance: ObservationProvenance {
                run_id: Uuid::from_u128(2),
                run_group_id: Uuid::from_u128(3),
                connection_target_id: Uuid::from_u128(1),
                source_kind: "virtual_machine".to_string(),
                source_id: Uuid::from_u128(1),
                observed_from: "runner".to_string(),
            },
            proof: DiscoveryProof {
                origin: DiscoveryOrigin::HostListener.as_str().to_string(),
                ..DiscoveryProof::default()
            },
            status,
            reason: reason.to_string(),
            exporter_type: None,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn endpoint_identity_normalizes_path_and_rejects_userinfo() {
        let identity =
            EndpointIdentity::validated(Uuid::from_u128(1), "10.0.0.5", 9100, "HTTP", "metrics")
                .unwrap();
        assert_eq!(identity.path, "/metrics");
        assert_eq!(identity.scheme, "http");
        assert!(
            EndpointIdentity::validated(Uuid::from_u128(1), "10.0.0.5", 0, "http", "/m").is_err()
        );
        assert!(
            EndpointIdentity::validated(Uuid::from_u128(1), "u@10.0.0.5", 9100, "http", "/m")
                .is_err()
        );
        assert!(
            EndpointIdentity::validated(Uuid::from_u128(1), "10.0.0.5", 9100, "ftp", "/m").is_err()
        );
    }

    #[test]
    fn exporter_type_comes_from_typed_machine_identifiers_only() {
        assert_eq!(
            resolve_exporter_type(
                DiscoveryPlatform::Linux,
                IdentifierKind::SystemdUnit,
                "node_exporter.service"
            ),
            Some("node")
        );
        assert_eq!(
            resolve_exporter_type(
                DiscoveryPlatform::Linux,
                IdentifierKind::ContainerImage,
                "prom/node-exporter:v1.8.2"
            ),
            Some("node")
        );
        assert_eq!(
            resolve_exporter_type(
                DiscoveryPlatform::Windows,
                IdentifierKind::WindowsService,
                "windows_exporter"
            ),
            Some("windows")
        );
        assert_eq!(
            resolve_exporter_type(
                DiscoveryPlatform::Linux,
                IdentifierKind::ProcessExecutable,
                "some-random-service"
            ),
            None
        );
    }

    #[test]
    fn probe_only_statuses_are_rejected_for_host_inspection() {
        let mut item = observation(DiscoveryStatus::Unreachable, reason::TIMEOUT);
        assert!(item.validate().is_err());
        item.status = DiscoveryStatus::ConfirmedAbsent;
        item.reason = reason::NO_LISTENER.to_string();
        assert!(item.validate().is_ok());
        item.source = DiscoverySource::EndpointProbe;
        assert!(item.validate().is_err());
    }

    #[test]
    fn report_counts_must_add_up_and_partial_marks_the_group() {
        let host_id = Uuid::from_u128(1);
        let host_names = BTreeMap::from([(host_id, ("db-01".to_string(), "device".to_string()))]);
        let report = DiscoveryReport::build(
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            "2026-11-10T00:00:00Z".to_string(),
            "2026-11-10T00:01:00Z".to_string(),
            BTreeMap::from([("host_inspection".to_string(), true)]),
            &host_names,
            vec![
                observation(DiscoveryStatus::ConfirmedAbsent, reason::NO_LISTENER),
                observation(DiscoveryStatus::Error, reason::INSPECTION_FAILED),
            ],
        );
        assert!(report.validate().is_ok());
        assert!(report.groups[0].partial);
        assert_eq!(report.groups[0].counts.get("error"), Some(&1));

        let mut broken = report.clone();
        broken.groups[0].counts.remove("error");
        assert!(broken.validate().is_err());
    }

    #[test]
    fn reason_vocabulary_is_snake_case_and_bounded() {
        for reason in [
            reason::NO_LISTENER,
            reason::COMPLETE_INSPECTION,
            reason::CONNECTION_REFUSED,
            reason::TLS_VERIFICATION_FAILED,
            reason::REDIRECT_NOT_FOLLOWED,
            reason::UNRECOGNIZED_OWNER,
            reason::NOT_METRICS_RESPONSE,
            reason::PERMISSION_DENIED,
        ] {
            assert!(is_valid_reason_code(reason), "{reason}");
        }
        assert!(!is_valid_reason_code(""));
        assert!(!is_valid_reason_code("Not A Reason"));
        assert!(!is_valid_reason_code("UPPER"));
    }
}
