//! Sealed NetBox capture format. Sealing authenticates bytes, not migration readiness.
//!
//! The external trust anchor is SHA256 of the exact `manifest.json` bytes. Every
//! other file is referenced exactly once by that manifest. No target catalog is
//! consulted: even empty, inaccessible and unrepresentable resources survive in
//! the census. Discovery completeness is evidence supplied by the capture owner,
//! not something a file reader can infer from a list of resources.
//!
//! A seal also proves only integrity: whoever writes the bytes also writes the
//! claim about them. `SourceAuthority` therefore names who attests the contents,
//! and only a snapshot attestation carries exact expectations
//! that this format re-derives from the captured records. A live capture status
//! is evidence about a read, never authority to migrate.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::inventory_sync::{SourceEntity, SourceRecord};
use crate::errors::AppError;

pub const EXPORT_FORMAT_VERSION: u32 = 2;
pub const NETBOX_VERSION: &str = "4.7.1";
pub const MANIFEST_FILE: &str = "manifest.json";
/// Independent expectations document for a snapshot attestation.
pub const EXPECTATIONS_FORMAT_VERSION: u32 = 1;
/// Field-path prefix of a target custom-field reference edge.
pub const CUSTOM_FIELD_EDGE_PREFIX: &str = "custom_fields.sha256:";

/// Hash of exact bytes, encoded as lowercase hexadecimal SHA256.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn invalid(code: &'static str) -> AppError {
    AppError::Validation(format!("inventory_export:{code}"))
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Flat ASCII filenames avoid directory aliases within the sealed v2 bundle.
pub(crate) fn safe_payload_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && path != MANIFEST_FILE
        && !path.starts_with('.')
        && path.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.'
        })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadDescriptor {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatus {
    Captured,
    Partial,
    /// Withheld by the explicit capture profile, not an observed HTTP failure.
    NotAttempted,
    Inaccessible,
    Unrepresentable,
}

/// A discovered endpoint, whether or not representable by `SourceEntity`.
/// Endpoint and record values are private artifact data, never diagnostic text.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceCensus {
    pub resource_key: String,
    pub endpoint: String,
    pub entity: Option<SourceEntity>,
    pub status: CaptureStatus,
    pub declared_rows: Option<u64>,
    pub observed_rows: u64,
    /// JSON array of the existing `SourceRecord` aggregate, including `[]`.
    pub payload: Option<PayloadDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessArea {
    Plugins,
    ScriptsAndJobs,
    Webhooks,
    EventRules,
    Permissions,
}

/// Canonical order is the declaration order, so a scope comparison and its
/// digest never depend on how an author listed the areas.
pub const PROCESS_AREAS: [ProcessArea; 5] = [
    ProcessArea::Plugins,
    ProcessArea::ScriptsAndJobs,
    ProcessArea::Webhooks,
    ProcessArea::EventRules,
    ProcessArea::Permissions,
];

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Observed,
    Inaccessible,
    Unobservable,
    Unsupported,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessCoverage {
    pub area: ProcessArea,
    pub status: ObservationStatus,
    /// Captured evidence bytes, not a promise that this area can be migrated.
    pub evidence: Option<PayloadDescriptor>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyStatus {
    Consistent,
    Inconsistent,
    Unproven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureOutcome {
    Complete,
    Incomplete,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureEvidence {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub outcome: CaptureOutcome,
    pub consistency: ConsistencyStatus,
    pub consistency_evidence: PayloadDescriptor,
    pub discovery_complete: bool,
    pub discovery_evidence: PayloadDescriptor,
}

/// Who attests the captured contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// Read from a live NetBox. A capture status describes that read; it is not
    /// an independent attestation of visibility, consistency or completeness.
    LiveCapture,
    /// An owned endorsed snapshot with exact expectations.
    OperatorEndorsedSnapshot,
}

/// One declared resource of the exact authored scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeResource {
    pub resource_key: String,
    pub entity: SourceEntity,
}

/// The exact source scope a capture claims to cover. It must equal the census,
/// so a capture cannot narrow its own claim after the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceScope {
    pub resources: Vec<ScopeResource>,
    pub processes: Vec<ProcessArea>,
}

impl SourceScope {
    /// Strictly increasing canonical keys and exactly the five process areas.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.processes != PROCESS_AREAS {
            return Err(invalid("authority_scope"));
        }
        let mut previous: Option<&str> = None;
        for resource in &self.resources {
            // The scope arrives from untrusted bytes; revalidate its identities.
            if SourceEntity::new(resource.entity.as_str())
                .map_err(|_| invalid("authority_scope"))?
                != resource.entity
                || resource.resource_key.trim().is_empty()
                || previous.is_some_and(|key| key >= resource.resource_key.as_str())
            {
                return Err(invalid("authority_scope"));
            }
            previous = Some(&resource.resource_key);
        }
        Ok(())
    }

    /// Canonical digest of the exact declared scope. Callers pin this instead of
    /// trusting a scope that arrives inside the same bytes it describes.
    pub fn digest(&self) -> String {
        // Length-prefixed fields keep the encoding unambiguous for any bytes an
        // untrusted scope may carry.
        let mut bytes = Vec::new();
        for resource in &self.resources {
            for part in [
                resource.resource_key.as_bytes(),
                resource.entity.as_str().as_bytes(),
            ] {
                bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
                bytes.extend_from_slice(part);
            }
        }
        bytes.push(0);
        for area in &self.processes {
            // Declaration index, independent of serde naming.
            bytes.push(*area as u8);
        }
        digest(&bytes)
    }
}

/// Which reference shapes an attestation reconciles exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeCoverage {
    /// Every reference the canonical catalog and the captured custom-field
    /// definitions imply for a record, with the exact entity each field targets:
    /// ordinary foreign keys (object, scalar or id list), generic references
    /// resolved through their `app.model` type token, junction and tag lists,
    /// and object/multiobject custom-field values in scalar, object or list
    /// form. Content-type scope metadata is validated as tokens and is never
    /// turned into an inventory row. A reference field the catalog does not
    /// describe, or a populated custom field without a captured definition, is
    /// refused rather than assumed to be data.
    TypedInventoryReferences,
}

/// Exact expectations, committed by digest so private captured
/// values never appear in this document. Each edge names the exact source field
/// path, the target identity and the entity that field implies; the immutable
/// provider re-derives that typed census and rejects any mismatch.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotExpectations {
    pub format_version: u32,
    pub records: Vec<ExpectedRecord>,
    pub edges: Vec<ExpectedEdge>,
}

/// One exactly expected record. `value_sha256` is [`record_value_digest`].
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRecord {
    pub entity: SourceEntity,
    pub external_id: String,
    pub value_sha256: String,
}

/// One exactly expected reference edge inside the captured records.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedEdge {
    pub from_entity: SourceEntity,
    pub from_external_id: String,
    /// `name`, `name[index]` or `custom_fields.<name>`, the documented
    /// expectations path grammar.
    pub field: String,
    pub to_entity: SourceEntity,
    pub to_external_id: String,
}

/// Digest of the exact canonical record values, never the record itself.
pub fn record_value_digest(record: &SourceRecord) -> Result<String, AppError> {
    serde_json::to_vec(&record.fields)
        .map(|bytes| digest(&bytes))
        .map_err(|_| invalid("expectation_record_encoding"))
}

/// Read the provider object id an expectation names at `path`.
///
/// The rule is the structural expectations grammar: a `name` field holds either
/// a scalar id or an object with an `id`, `name[index]` selects one element of
/// an array of the same, and a `custom_fields.<key>` path reads that custom
/// field value. Authoring tools and structural tests use it; acceptance is
/// decided by the typed census in the immutable provider, which also proves the
/// entity each field implies.
pub fn expectation_edge_target(record: &SourceRecord, path: &str) -> Option<String> {
    let (base, index) = match path.split_once('[') {
        Some((base, rest)) => (base, Some(rest.strip_suffix(']')?.parse::<usize>().ok()?)),
        None => (path, None),
    };
    let value = match base.strip_prefix("custom_fields.") {
        Some(key) => record.fields.get("custom_fields")?.as_object()?.get(key)?,
        None => record.fields.get(base)?,
    };
    let value = match index {
        None => value,
        Some(index) => value.as_array()?.get(index)?,
    };
    match value {
        serde_json::Value::Object(map) => map.get("id").and_then(scalar_id),
        other => scalar_id(other),
    }
}

fn scalar_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// A canonical positive provider id as [`String`]. Provider ids are canonical
/// integers, so an all-digit value is the only scalar a reference list may hold.
fn canonical_id(value: &serde_json::Value) -> Option<String> {
    let text = scalar_id(value)?;
    (!text.is_empty()
        && text.len() <= 20
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && text.trim_start_matches('0') != "")
        .then_some(text)
}

fn identified_object(value: &serde_json::Value) -> bool {
    matches!(value, serde_json::Value::Object(map) if map.get("id").and_then(scalar_id).is_some())
}

/// Every reference path of one record in the structural expectations grammar.
///
/// This is an authoring and structural aid: it locates reference-shaped values
/// without a target catalog. Trusted acceptance is decided by the typed census
/// in the immutable provider, which knows the entity each field implies and
/// refuses shapes it cannot type. Shapes that may hide provider ids but cannot
/// be expressed as one path are rejected instead of skipped.
pub fn reference_paths(record: &SourceRecord) -> Result<Vec<String>, AppError> {
    let mut paths = Vec::new();
    for (name, value) in &record.fields {
        collect_reference_paths(name, value, &mut paths)?;
    }
    match record.fields.get("custom_fields") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(map)) => {
            for (name, value) in map {
                if name.is_empty() || name.contains(['.', '[', ']']) {
                    return Err(invalid("edge_coverage"));
                }
                collect_reference_paths(&format!("custom_fields.{name}"), value, &mut paths)?;
            }
        }
        Some(_) => return Err(invalid("edge_coverage")),
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn collect_reference_paths(
    path: &str,
    value: &serde_json::Value,
    paths: &mut Vec<String>,
) -> Result<(), AppError> {
    match value {
        serde_json::Value::Object(_) => {
            if identified_object(value) {
                paths.push(path.to_owned());
            }
            Ok(())
        }
        serde_json::Value::Array(items) if items.is_empty() => Ok(()),
        serde_json::Value::Array(items) => {
            let identified = items.iter().filter(|item| identified_object(item)).count();
            let ids = items
                .iter()
                .filter(|item| canonical_id(item).is_some())
                .count();
            if identified == items.len() || (identified == 0 && ids == items.len()) {
                for index in 0..items.len() {
                    paths.push(format!("{path}[{index}]"));
                }
                return Ok(());
            }
            if identified == 0 && ids == 0 {
                // Plain data values cannot be canonical provider ids.
                return Ok(());
            }
            // A list that mixes identified and unidentified entries may still be
            // a provider-id list, so it is refused rather than skipped.
            Err(invalid("edge_coverage"))
        }
        _ => Ok(()),
    }
}

/// Ownership and scope of an endorsed snapshot. The caller must pin the same
/// issuer, scope digest and expectations digest independently of these bytes.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotAttestation {
    pub operator_id: Uuid,
    pub provenance: PayloadDescriptor,
    /// Bounded ownership marker, e.g. `pwa-81/operator`.
    pub issuer: String,
    pub issued_at: DateTime<Utc>,
    pub scope: SourceScope,
    /// Reference shapes this attestation actually reconciles.
    pub edge_coverage: EdgeCoverage,
    /// The verified expectations payload this attestation claims.
    pub expectations: PayloadDescriptor,
}

/// Immutable identity of one verified snapshot attestation.
///
/// A run descriptor pins these values: `authority_id` and `attestation_sha256`
/// are derived here, so a descriptor cannot establish authority by naming its
/// own UUID or digest. The trust decision stays with the profile's independent
/// anchors, which must match first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAttestationIdentity {
    pub operator_id: Uuid,
    pub provenance_sha256: String,
    /// Deterministic UUIDv5 over the canonical attestation digest.
    pub authority_id: Uuid,
    /// SHA256 of the canonical attestation fields. The exact sealed bytes are
    /// covered separately by the manifest digest.
    pub attestation_sha256: String,
    pub issuer: String,
    pub issued_at: DateTime<Utc>,
    pub scope_digest: String,
    pub expectations_sha256: String,
    pub edge_coverage: EdgeCoverage,
}

/// Attestation claim of one bundle. Live captures carry no attestation at all,
/// which is why their capture status can never authorize an import.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAuthority {
    pub kind: AuthorityKind,
    /// Present exactly for [`AuthorityKind::OperatorEndorsedSnapshot`].
    pub snapshot: Option<SnapshotAttestation>,
}

impl SourceAuthority {
    pub fn live_capture() -> Self {
        Self {
            kind: AuthorityKind::LiveCapture,
            snapshot: None,
        }
    }

    pub fn operator_endorsed_snapshot(attestation: SnapshotAttestation) -> Self {
        Self {
            kind: AuthorityKind::OperatorEndorsedSnapshot,
            snapshot: Some(attestation),
        }
    }

    /// Canonical identity of the sealed attestation, or `None` for a live
    /// capture. Derived from verified fields, never from a caller-supplied id.
    pub fn attestation_identity(&self) -> Result<Option<SourceAttestationIdentity>, AppError> {
        let Some(attestation) = &self.snapshot else {
            return Ok(None);
        };
        let bytes = serde_json::to_vec(attestation)
            .map_err(|_| AppError::Internal("inventory_export:attestation_encoding".into()))?;
        let attestation_sha256 = digest(&bytes);
        Ok(Some(SourceAttestationIdentity {
            operator_id: attestation.operator_id,
            provenance_sha256: attestation.provenance.sha256.clone(),
            authority_id: Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                format!("piping.inventory.source-attestation:{attestation_sha256}").as_bytes(),
            ),
            attestation_sha256,
            issuer: attestation.issuer.clone(),
            issued_at: attestation.issued_at,
            scope_digest: attestation.scope.digest(),
            expectations_sha256: attestation.expectations.sha256.clone(),
            edge_coverage: attestation.edge_coverage,
        }))
    }

    /// The kind and the attestation must agree; there is no half-attested form.
    pub fn validate(&self) -> Result<(), AppError> {
        match (self.kind, self.snapshot.as_ref()) {
            (AuthorityKind::LiveCapture, None) => Ok(()),
            (AuthorityKind::OperatorEndorsedSnapshot, Some(attestation)) => {
                if attestation.operator_id.is_nil() || !valid_digest(&attestation.provenance.sha256)
                {
                    return Err(invalid("authority_provenance"));
                }
                if attestation.issuer.is_empty()
                    || attestation.issuer.len() > 128
                    || !attestation.issuer.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || byte == b'-'
                            || byte == b'.'
                            || byte == b'_'
                            || byte == b':'
                            || byte == b'/'
                    })
                {
                    return Err(invalid("authority_issuer"));
                }
                attestation.scope.validate()
            }
            _ => Err(invalid("authority")),
        }
    }
}

/// Presence of this strict manifest is the seal. An unfinished writer has no
/// manifest; a sealed incomplete capture remains explicitly incomplete.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportManifest {
    pub format_version: u32,
    pub source_provider_id: Uuid,
    pub observed_netbox_version: String,
    pub exporter_source_sha: String,
    pub authority: SourceAuthority,
    pub capture: CaptureEvidence,
    pub schema: PayloadDescriptor,
    pub resources: Vec<ResourceCensus>,
    pub processes: Vec<ProcessCoverage>,
}

impl ExportManifest {
    /// Check structural evidence without imposing target migration readiness.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.format_version != EXPORT_FORMAT_VERSION
            || self.observed_netbox_version != NETBOX_VERSION
            || self.source_provider_id.is_nil()
            || self.exporter_source_sha.len() != 40
            || !self
                .exporter_source_sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("identity"));
        }
        self.authority.validate()?;
        if self.capture.ended_at < self.capture.started_at {
            return Err(invalid("capture_interval"));
        }
        let mut paths = BTreeSet::new();
        for descriptor in self.payloads() {
            if !safe_payload_path(&descriptor.path)
                || !valid_digest(&descriptor.sha256)
                || !paths.insert(&descriptor.path)
            {
                return Err(invalid("payload_descriptor"));
            }
        }
        let mut keys = BTreeSet::new();
        let mut endpoints = BTreeSet::new();
        let mut entities = BTreeSet::new();
        for resource in &self.resources {
            if resource.resource_key.trim().is_empty()
                || resource.endpoint.trim().is_empty()
                || !keys.insert(&resource.resource_key)
                || !endpoints.insert(&resource.endpoint)
            {
                return Err(invalid("resource_census"));
            }
            if let Some(entity) = &resource.entity {
                let validated =
                    SourceEntity::new(entity.as_str()).map_err(|_| invalid("resource_entity"))?;
                if validated != *entity || !entities.insert(entity.as_str()) {
                    return Err(invalid("resource_entity"));
                }
            }
            let captured = matches!(
                resource.status,
                CaptureStatus::Captured | CaptureStatus::Partial
            );
            if captured != resource.payload.is_some()
                || (captured && resource.entity.is_none())
                || (!captured && resource.observed_rows != 0)
                || resource
                    .declared_rows
                    .is_some_and(|n| n < resource.observed_rows)
                || (resource.status == CaptureStatus::Captured
                    && resource.declared_rows != Some(resource.observed_rows))
            {
                return Err(invalid("resource_count"));
            }
        }
        let areas: BTreeSet<_> = self.processes.iter().map(|p| p.area).collect();
        if self.processes.len() != 5
            || areas
                != BTreeSet::from([
                    ProcessArea::Plugins,
                    ProcessArea::ScriptsAndJobs,
                    ProcessArea::Webhooks,
                    ProcessArea::EventRules,
                    ProcessArea::Permissions,
                ])
            || self
                .processes
                .iter()
                .any(|p| p.status == ObservationStatus::Observed && p.evidence.is_none())
        {
            return Err(invalid("process_coverage"));
        }
        if self.capture.outcome == CaptureOutcome::Complete
            && (!self.capture.discovery_complete
                || self.capture.consistency != ConsistencyStatus::Consistent
                || self
                    .resources
                    .iter()
                    .any(|r| r.status != CaptureStatus::Captured)
                || self
                    .processes
                    .iter()
                    .any(|p| p.status != ObservationStatus::Observed))
        {
            return Err(invalid("false_complete"));
        }
        if self.authority.kind == AuthorityKind::OperatorEndorsedSnapshot {
            // An endorsed snapshot is complete by construction; an unfinished or
            // inconsistent one is a plain incomplete capture and stays blocked.
            if self.capture.outcome != CaptureOutcome::Complete {
                return Err(invalid("authority_incomplete"));
            }
            let attestation = self
                .authority
                .snapshot
                .as_ref()
                .ok_or_else(|| invalid("authority"))?;
            if attestation.scope != self.declared_scope()? {
                return Err(invalid("authority_scope"));
            }
        }
        Ok(())
    }

    /// Exact scope the census claims, in canonical order. A census entry without
    /// a canonical entity cannot be attested.
    pub fn declared_scope(&self) -> Result<SourceScope, AppError> {
        let mut resources = Vec::with_capacity(self.resources.len());
        for resource in &self.resources {
            resources.push(ScopeResource {
                resource_key: resource.resource_key.clone(),
                entity: resource
                    .entity
                    .clone()
                    .ok_or_else(|| invalid("authority_scope"))?,
            });
        }
        resources.sort_by(|a, b| a.resource_key.cmp(&b.resource_key));
        let scope = SourceScope {
            resources,
            processes: PROCESS_AREAS.to_vec(),
        };
        // Sorting is not enough: duplicate keys would make one claim ambiguous.
        scope.validate()?;
        Ok(scope)
    }

    pub fn payloads(&self) -> Vec<&PayloadDescriptor> {
        let mut payloads = vec![
            &self.schema,
            &self.capture.consistency_evidence,
            &self.capture.discovery_evidence,
        ];
        payloads.extend(self.resources.iter().filter_map(|r| r.payload.as_ref()));
        payloads.extend(self.processes.iter().filter_map(|p| p.evidence.as_ref()));
        payloads.extend(
            self.authority
                .snapshot
                .iter()
                .map(|attestation| &attestation.expectations),
        );
        payloads.extend(
            self.authority
                .snapshot
                .iter()
                .map(|attestation| &attestation.provenance),
        );
        payloads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(name: &str) -> PayloadDescriptor {
        PayloadDescriptor {
            path: name.into(),
            byte_length: 2,
            sha256: digest(b"{}"),
        }
    }

    fn complete_manifest() -> ExportManifest {
        let now = Utc::now();
        ExportManifest {
            format_version: EXPORT_FORMAT_VERSION,
            source_provider_id: Uuid::from_u128(1),
            observed_netbox_version: NETBOX_VERSION.into(),
            exporter_source_sha: "6b53415ba3ebed45b55d08d2c5e15d766f2dd2c1".into(),
            authority: SourceAuthority::live_capture(),
            capture: CaptureEvidence {
                started_at: now,
                ended_at: now,
                outcome: CaptureOutcome::Complete,
                consistency: ConsistencyStatus::Consistent,
                consistency_evidence: descriptor("consistency.json"),
                discovery_complete: true,
                discovery_evidence: descriptor("discovery.json"),
            },
            schema: descriptor("schema.json"),
            resources: vec![ResourceCensus {
                resource_key: "dcim.sites".into(),
                endpoint: "/api/dcim/sites/".into(),
                entity: Some(SourceEntity::new("dcim.sites").expect("entity")),
                status: CaptureStatus::Captured,
                declared_rows: Some(0),
                observed_rows: 0,
                payload: Some(PayloadDescriptor {
                    path: "sites.json".into(),
                    byte_length: 2,
                    sha256: digest(b"[]"),
                }),
            }],
            processes: [
                ProcessArea::Plugins,
                ProcessArea::ScriptsAndJobs,
                ProcessArea::Webhooks,
                ProcessArea::EventRules,
                ProcessArea::Permissions,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, area)| ProcessCoverage {
                area,
                status: ObservationStatus::Observed,
                evidence: Some(descriptor(&format!("process-{i}.json"))),
            })
            .collect(),
        }
    }

    #[test]
    fn completeness_cannot_conceal_unproven_inconsistent_or_unobservable_capture() {
        complete_manifest()
            .validate()
            .expect("structurally complete");
        for operation in 0..5 {
            let mut manifest = complete_manifest();
            match operation {
                0 => manifest.capture.consistency = ConsistencyStatus::Unproven,
                1 => manifest.capture.consistency = ConsistencyStatus::Inconsistent,
                2 => manifest.capture.discovery_complete = false,
                3 => manifest.resources[0].status = CaptureStatus::Partial,
                _ => manifest.processes[0].status = ObservationStatus::Unobservable,
            }
            assert!(manifest.validate().is_err());
            // These are admissible source facts, not readiness decisions.
            manifest.capture.outcome = CaptureOutcome::Incomplete;
            manifest.validate().expect("preserved incomplete evidence");
        }
    }

    #[test]
    fn unavailable_noncanonical_resources_and_process_evidence_survive_roundtrip() {
        let mut manifest = complete_manifest();
        manifest.capture.outcome = CaptureOutcome::Incomplete;
        manifest.resources.push(ResourceCensus {
            resource_key: "plugins.nested.endpoint".into(),
            endpoint: "/api/plugins/example/nested/endpoint/".into(),
            entity: None,
            status: CaptureStatus::Unrepresentable,
            declared_rows: Some(19),
            observed_rows: 0,
            payload: None,
        });
        manifest.resources.push(ResourceCensus {
            resource_key: "users.permissions".into(),
            endpoint: "/api/users/permissions/".into(),
            entity: Some(SourceEntity::new("users.permissions").expect("entity")),
            status: CaptureStatus::Inaccessible,
            declared_rows: None,
            observed_rows: 0,
            payload: None,
        });
        manifest.processes[0].status = ObservationStatus::Unsupported;
        manifest.processes[4].status = ObservationStatus::Inaccessible;
        let bytes = serde_json::to_vec(&manifest).expect("serialize");
        let decoded: ExportManifest = serde_json::from_slice(&bytes).expect("deserialize");
        decoded
            .validate()
            .expect("all discovered evidence retained");
        assert_eq!(decoded.resources.len(), 3);
        assert_eq!(decoded.resources[1].declared_rows, Some(19));
        assert!(decoded.resources[1].entity.is_none());
        assert!(decoded.resources[2].status == CaptureStatus::Inaccessible);
        assert!(decoded.processes[0].status == ObservationStatus::Unsupported);
        assert!(decoded.processes[4].status == ObservationStatus::Inaccessible);
    }

    /// The attestation claims the census, so it is derived from it here.
    fn attest(manifest: &mut ExportManifest) -> SnapshotAttestation {
        let attestation = SnapshotAttestation {
            operator_id: Uuid::from_u128(4),
            provenance: descriptor("provenance.json"),
            issuer: "pwa-81/synthetic-fixture".into(),
            issued_at: Utc::now(),
            scope: manifest.declared_scope().expect("scope"),
            edge_coverage: EdgeCoverage::TypedInventoryReferences,
            expectations: descriptor("expectations.json"),
        };
        manifest.authority = SourceAuthority::operator_endorsed_snapshot(attestation.clone());
        attestation
    }

    #[test]
    fn synthetic_attestation_requires_complete_exact_scope_and_expectations() {
        let mut manifest = complete_manifest();
        let attestation = attest(&mut manifest);
        manifest.validate().expect("attested synthetic capture");
        // A scope is canonical: ascending keys validate, and its digest is stable.
        let mut scope = attestation.scope.clone();
        scope.resources.insert(
            0,
            ScopeResource {
                resource_key: "dcim.racks".into(),
                entity: SourceEntity::new("dcim.racks").expect("entity"),
            },
        );
        scope.validate().expect("canonical order");
        assert_eq!(scope.digest(), scope.clone().digest());
        scope.resources.reverse();
        assert!(scope.validate().is_err());

        for operation in 0..6 {
            let mut manifest = complete_manifest();
            let attestation = attest(&mut manifest);
            let mut authority = manifest.authority.clone();
            let expected = match operation {
                0 => {
                    manifest.resources[0].resource_key = "dcim.racks".into();
                    "authority_scope"
                }
                1 => {
                    manifest.resources[0].status = CaptureStatus::Partial;
                    "false_complete"
                }
                2 => {
                    manifest.capture.outcome = CaptureOutcome::Incomplete;
                    "authority_incomplete"
                }
                3 => {
                    authority.kind = AuthorityKind::LiveCapture;
                    "inventory_export:authority"
                }
                4 => {
                    authority.snapshot = None;
                    "inventory_export:authority"
                }
                _ => {
                    let mut scope = attestation.scope;
                    scope.resources.reverse();
                    scope.resources.push(ScopeResource {
                        resource_key: "dcim.racks".into(),
                        entity: SourceEntity::new("dcim.racks").expect("entity"),
                    });
                    authority = SourceAuthority::operator_endorsed_snapshot(SnapshotAttestation {
                        scope,
                        ..attestation
                    });
                    "authority_scope"
                }
            };
            manifest.authority = authority;
            let error = manifest.validate().err().expect("must reject").to_string();
            assert!(error.contains(expected), "unsafe rejection: {error}");
        }

        let mut manifest = complete_manifest();
        let mut authority = attest(&mut manifest);
        authority.issuer = "unsafe issuer".into();
        manifest.authority = SourceAuthority::operator_endorsed_snapshot(authority);
        assert!(manifest.validate().is_err());
    }

    fn record(fields: serde_json::Value) -> SourceRecord {
        use crate::contexts::sync::domain::inventory_sync::SourceIdentity;
        let serde_json::Value::Object(fields) = fields else {
            panic!("record fields must be an object");
        };
        SourceRecord::new(
            SourceIdentity::new(
                Uuid::from_u128(1),
                SourceEntity::new("dcim.sites").expect("entity"),
                "1",
            )
            .expect("identity"),
            fields,
        )
    }

    #[test]
    fn reference_paths_cover_object_and_identifier_lists_and_refuse_ambiguity() {
        let source = record(serde_json::json!({
            "name": "private",
            "count": 5,
            "region": {"id": 2},
            "related_ids": [3, "1"],
            "names": ["a", "b"],
            "empty": [],
            "custom_fields": {
                "owner_ref": {"id": 1},
                "multi": [{"id": 4}, {"id": 5}],
                "labels": ["a", "b"],
                "flag": true,
            },
        }));
        assert_eq!(
            reference_paths(&source).expect("covered grammar"),
            vec![
                "custom_fields.multi[0]".to_owned(),
                "custom_fields.multi[1]".to_owned(),
                "custom_fields.owner_ref".to_owned(),
                "region".to_owned(),
                "related_ids[0]".to_owned(),
                "related_ids[1]".to_owned(),
            ]
        );
        assert_eq!(
            expectation_edge_target(&source, "related_ids[1]").as_deref(),
            Some("1")
        );
        assert_eq!(
            expectation_edge_target(&source, "custom_fields.owner_ref").as_deref(),
            Some("1")
        );

        // A list that mixes identified and unidentified entries may hide ids.
        let mixed = record(serde_json::json!({"related": [{"id": 1}, "x"]}));
        let error = reference_paths(&mixed).err().expect("ambiguous list");
        assert!(error.to_string().contains("inventory_export:edge_coverage"));
        let partial_ids = record(serde_json::json!({"related": [1, "x"]}));
        assert!(reference_paths(&partial_ids).is_err());
        // A custom-field name the path grammar cannot address is refused.
        let dotted = record(serde_json::json!({"custom_fields": {"a.b": {"id": 1}}}));
        assert!(reference_paths(&dotted).is_err());
    }

    #[test]
    fn attestation_identity_is_derived_from_the_verified_claim() {
        assert!(
            SourceAuthority::live_capture()
                .attestation_identity()
                .expect("live has no identity")
                .is_none()
        );
        let mut manifest = complete_manifest();
        let attestation = attest(&mut manifest);
        let identity = manifest
            .authority
            .attestation_identity()
            .expect("derived")
            .expect("synthetic identity");
        assert_eq!(identity.issuer, attestation.issuer);
        assert_eq!(identity.scope_digest, attestation.scope.digest());
        assert_eq!(
            identity.expectations_sha256,
            attestation.expectations.sha256
        );
        assert_eq!(
            identity.edge_coverage,
            EdgeCoverage::TypedInventoryReferences
        );
        assert_eq!(
            manifest
                .authority
                .attestation_identity()
                .expect("repeat")
                .expect("repeat identity"),
            identity
        );

        for change in 0..3 {
            let mut changed = manifest.authority.clone();
            let attestation = changed.snapshot.as_mut().expect("attestation");
            match change {
                0 => attestation.issuer = "pwa-81/other-fixture".into(),
                1 => attestation.expectations.sha256 = "0".repeat(64),
                _ => {
                    attestation.scope.resources.pop();
                }
            }
            let changed = changed
                .attestation_identity()
                .expect("derived")
                .expect("identity");
            assert_ne!(changed.attestation_sha256, identity.attestation_sha256);
            assert_ne!(changed.authority_id, identity.authority_id);
        }
    }
}
