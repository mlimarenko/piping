//! Shared inventory-synchronisation contract (PIP-86).
//!
//! This module is the provider-neutral boundary between the remote inventory
//! source, the immutable mapping configuration and the local Piping inventory
//! target. Source records remain in memory until the target validates their
//! fields; reports contain identities and counts, not record contents.
//!
//! Ownership:
//!
//! * this module (sync context) — the job/configuration contract and the
//!   read-only source and local-target ports;
//! * `super::inventory_target_schema` — the closed
//!   [`InventoryEntity`]/[`InventoryField`] schema;
//! * `infrastructure::persistence::local_inventory_target` — typed SQL writes.
//!
//! The trait signatures here are the interfaces the target implements; the
//! concrete entity/field enums are owned by `inventory_target_schema` on
//! purpose, so the closed catalogue cannot drift from this port.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::absence::AbsencePolicy;
use super::entity_mapping::EntityMapping;
use super::field_mapping::FieldMapping;
use super::inventory_target_schema::{InventoryEntity, InventoryField};
use super::mapping_configuration::{ConflictMode, MappingConfigurationStatus, MappingDirection};
use super::sync_job::SyncJob;
use crate::contexts::access_control::domain::Action;
use crate::errors::AppError;

// ---------------------------------------------------------------------------
// Immutable mapping identity
// ---------------------------------------------------------------------------

/// Immutable identity of one mapping configuration version:
/// `"<logical_uuid>:<version_i32>"`.
///
/// This string is the only supported `mapping_version` and is never edited in
/// place. A change produces a new `version_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingRef {
    pub logical_id: Uuid,
    pub version_id: i32,
}

impl MappingRef {
    #[must_use]
    pub fn new(logical_id: Uuid, version_id: i32) -> Self {
        Self {
            logical_id,
            version_id,
        }
    }

    /// Canonical immutable wire form, `<logical_uuid>:<version_i32>`.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!("{}:{}", self.logical_id, self.version_id)
    }

    /// Parse the canonical immutable wire form. Rejects any other shape rather
    /// than guessing a version.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        let raw = raw.trim();
        let (logical, version) = raw.split_once(':').ok_or_else(|| {
            AppError::Validation("mapping_version must be '<logical_uuid>:<version>'".to_string())
        })?;
        if logical.contains(':') || version.contains(':') {
            return Err(AppError::Validation(
                "mapping_version must be '<logical_uuid>:<version>'".to_string(),
            ));
        }
        let logical_id = Uuid::parse_str(logical).map_err(|_| {
            AppError::Validation("mapping_version has an invalid logical id".to_string())
        })?;
        let version_id: i32 = version.parse().map_err(|_| {
            AppError::Validation("mapping_version has an invalid version".to_string())
        })?;
        if version_id <= 0 {
            return Err(AppError::Validation(
                "mapping_version must be '<logical_uuid>:<version>'".to_string(),
            ));
        }
        Ok(Self {
            logical_id,
            version_id,
        })
    }
}

impl fmt::Display for MappingRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_wire())
    }
}

// ---------------------------------------------------------------------------
// Mapping target
// ---------------------------------------------------------------------------

/// Where a mapping writes.
///
/// The local target is the typed Piping inventory; a provider target is a
/// remote provider addressed by id. The two are mutually exclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MappingTarget {
    /// The typed local Piping inventory.
    LocalInventory,
    /// A remote provider target.
    Provider { provider_id: Uuid },
}

impl MappingTarget {
    #[must_use]
    pub fn provider(provider_id: Uuid) -> Self {
        Self::Provider { provider_id }
    }

    #[must_use]
    pub fn provider_id(&self) -> Option<Uuid> {
        match self {
            Self::LocalInventory => None,
            Self::Provider { provider_id } => Some(*provider_id),
        }
    }

    /// Map the nullable `target_provider_id` column into a target.
    #[must_use]
    pub fn from_provider_column(value: Option<Uuid>) -> Self {
        match value {
            Some(provider_id) => Self::Provider { provider_id },
            None => Self::LocalInventory,
        }
    }

    /// Map a target back onto the nullable `target_provider_id` column.
    #[must_use]
    pub fn to_provider_column(&self) -> Option<Uuid> {
        self.provider_id()
    }
}

// ---------------------------------------------------------------------------
// Source identity, records and snapshot
// ---------------------------------------------------------------------------

/// Canonical NetBox resource plural, e.g. `dcim.sites` or `ipam.ip-addresses`.
///
/// Always the `<app>.<resource>` shape; free-form paths and provider URLs are
/// rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct SourceEntity(String);

impl SourceEntity {
    pub fn new(raw: &str) -> Result<Self, AppError> {
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > 128 {
            return Err(AppError::Validation(
                "source entity must be a canonical '<app>.<resource>' key".to_string(),
            ));
        }
        let (app, resource) = raw.split_once('.').ok_or_else(|| {
            AppError::Validation(
                "source entity must be a canonical '<app>.<resource>' key".to_string(),
            )
        })?;
        let segment_ok = |segment: &str| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };
        if resource.contains('.') || !segment_ok(app) || !segment_ok(resource) {
            return Err(AppError::Validation(
                "source entity must be a canonical '<app>.<resource>' key".to_string(),
            ));
        }
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceEntity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable identity of one record inside the remote source.
///
/// `source_provider_id` names the deterministic UUIDv5 namespace used by the
/// target binding table; `entity` + `external_id` are the provider-side key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub source_provider_id: Uuid,
    pub entity: SourceEntity,
    pub external_id: String,
}

impl SourceIdentity {
    pub fn new(
        source_provider_id: Uuid,
        entity: SourceEntity,
        external_id: impl Into<String>,
    ) -> Result<Self, AppError> {
        let external_id = external_id.into();
        if external_id.trim().is_empty() || external_id.len() > 256 {
            return Err(AppError::Validation(
                "source record external id must be a non-empty key".to_string(),
            ));
        }
        Ok(Self {
            source_provider_id,
            entity,
            external_id,
        })
    }
}

/// One mapped source record: identity plus its field values.
///
/// `created` and `last_updated` are source observations stored on the provider
/// binding, never replacements for local object dates or `piping_synced_at`.
/// Omission preserves a previous observation; an explicit null makes it unknown.
/// Non-null values require timezone-qualified instants representable without
/// loss at PostgreSQL's microsecond precision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub identity: SourceIdentity,
    pub fields: Map<String, Value>,
}

impl SourceRecord {
    #[must_use]
    pub fn new(identity: SourceIdentity, fields: Map<String, Value>) -> Self {
        Self { identity, fields }
    }

    #[must_use]
    pub fn entity(&self) -> &SourceEntity {
        &self.identity.entity
    }
}

/// One page of a read-only source fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcePage {
    pub records: Vec<SourceRecord>,
    /// `None` when the entity is exhausted; opaque otherwise.
    pub next_cursor: Option<String>,
}

/// The complete read of every approved source entity before any write.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceSnapshot {
    pub records: Vec<SourceRecord>,
    /// Collector-issued traversal evidence. Ad-hoc snapshots leave this empty.
    pub coverage: Option<CompletedSourceCoverage>,
}

impl SourceSnapshot {
    /// Construct an unsealed snapshot for non-destructive imports.
    #[must_use]
    pub fn unsealed(records: Vec<SourceRecord>) -> Self {
        Self {
            records,
            coverage: None,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn entities(&self) -> Vec<&SourceEntity> {
        let mut seen = Vec::new();
        for record in &self.records {
            if !seen.contains(&record.entity()) {
                seen.push(record.entity());
            }
        }
        seen
    }
}

/// Credential-owner evidence attached by a configured source, not a permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredSourceEvidence(super::absence::ProviderGenerationBinding);

impl ConfiguredSourceEvidence {
    pub(crate) fn from_binding(binding: super::absence::ProviderGenerationBinding) -> Self {
        Self(binding)
    }

    pub(crate) fn binding(&self) -> &super::absence::ProviderGenerationBinding {
        &self.0
    }
}

/// Opaque evidence of terminal reads of every collection in a source manifest.
/// It is not credential authority and cannot by itself authorize deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedSourceCoverage {
    provider_id: Uuid,
    entities: Vec<SourceEntity>,
    /// Entities of an optional namespace the source deliberately did not
    /// traverse because the provider does not expose it. They are part of the
    /// sealed manifest identity and can never appear as observed absences.
    withheld: Vec<SourceEntity>,
    /// Optional-namespace capability the traversal ran under, e.g. `present`.
    capability: Option<String>,
    records_digest: String,
    generation_binding: Option<ConfiguredSourceEvidence>,
}

/// Whether a source key belongs to the optional plugin namespace.
fn optional_source_key(entity: &str) -> bool {
    crate::contexts::sync::domain::inventory_target_schema::InventoryEntity::from_source_key(
        entity,
    )
    .is_some_and(
        crate::contexts::sync::domain::inventory_target_schema::InventoryEntity::is_optional_source,
    )
}

impl CompletedSourceCoverage {
    /// Verify the evidence still describes the snapshot and immutable mapping.
    #[must_use]
    pub fn matches(&self, snapshot: &SourceSnapshot, config: &MappingConfigDocument) -> bool {
        let mapped: HashSet<_> = config
            .entity_mappings
            .iter()
            .filter(|m| m.enabled)
            .map(|m| m.source_entity.as_str())
            .collect();
        let observed: HashSet<_> = self.entities.iter().map(SourceEntity::as_str).collect();
        let withheld: HashSet<_> = self.withheld.iter().map(SourceEntity::as_str).collect();
        if !observed.is_disjoint(&withheld) {
            return false;
        }
        let covered: HashSet<&str> = observed.union(&withheld).copied().collect();
        // A mapping may cover the optional plugin namespace even when the source
        // withheld it: that is the normal state of a configured source whose
        // plugin is currently absent. Anything mapped outside the covered set must
        // be optional; core and membership entities are always observed or
        // explicitly withheld.
        let extra_is_optional = mapped
            .difference(&covered)
            .all(|entity| optional_source_key(entity));
        self.provider_id == config.source_provider_id
            && observed.is_subset(&mapped)
            && extra_is_optional
            && snapshot
                .records
                .iter()
                .all(|r| r.identity.source_provider_id == self.provider_id)
            && records_digest(&snapshot.records).is_some_and(|digest| digest == self.records_digest)
    }

    #[must_use]
    pub fn completed_entities(&self) -> &[SourceEntity] {
        &self.entities
    }

    /// Optional-namespace entities the provider withheld from this traversal.
    #[must_use]
    pub fn withheld_entities(&self) -> &[SourceEntity] {
        &self.withheld
    }

    /// Optional-namespace capability this traversal ran under.
    #[must_use]
    pub fn capability(&self) -> Option<&str> {
        self.capability.as_deref()
    }

    /// The source owner's credential binding, if the source was sealed.
    #[must_use]
    pub fn generation_binding(&self) -> Option<&ConfiguredSourceEvidence> {
        self.generation_binding.as_ref()
    }

    pub(crate) fn seal_mapping(
        &self,
        config: &MappingConfigDocument,
    ) -> Result<super::SourceGeneration, AppError> {
        if config.status != MappingConfigurationStatus::Active {
            return Err(AppError::Validation(
                "sync.source_generation_mapping_inactive".into(),
            ));
        }
        let binding = self
            .generation_binding
            .as_ref()
            .ok_or_else(|| AppError::Validation("sync.source_generation_unattested".into()))?
            .binding();
        if binding.provider_id != config.source_provider_id
            || self.provider_id != config.source_provider_id
        {
            return Err(AppError::Validation(
                "sync.source_generation_provider_mismatch".into(),
            ));
        }
        let mut manifest: Vec<&str> = self
            .entities
            .iter()
            .chain(self.withheld.iter())
            .map(SourceEntity::as_str)
            .collect();
        manifest.sort_unstable();
        let manifest = serde_json::to_vec(&manifest)
            .map_err(|_| AppError::Validation("sync.source_generation_manifest".into()))?;
        let manifest_digest = super::inventory_export::digest(&manifest);
        super::SourceGeneration::seal(&super::SourceGenerationInputs {
            provider_id: binding.provider_id,
            provider_type: super::ProviderType::Netbox,
            origin: &binding.base_url,
            mapping: config.reference,
            mapping_digest: &config.content_digest(),
            policy: config.absence_policy,
            credential_revision: &binding.credential.revision,
            credential_id: binding.credential.secret_id,
            sync_mode: binding.sync_mode.as_deref().unwrap_or(""),
            manifest_digest: &manifest_digest,
            source_capability: self.capability.as_deref().unwrap_or(""),
        })
    }
}

fn records_digest(records: &[SourceRecord]) -> Option<String> {
    serde_json::to_vec(records)
        .ok()
        .map(|bytes| super::inventory_export::digest(&bytes))
}

/// Read a full source manifest. Only this collector mints traversal evidence.
pub async fn collect_snapshot<Source: InventorySource>(
    source: &Source,
    entities: &[SourceEntity],
) -> Result<SourceSnapshot, AppError> {
    let supported = source.supported_entities();
    let expected: HashSet<_> = supported.iter().collect();
    let requested: HashSet<_> = entities.iter().collect();
    if expected.is_empty()
        || expected.len() != supported.len()
        || requested.len() != entities.len()
        || requested != expected
    {
        return Err(AppError::Validation(
            "source manifest is incomplete or duplicated".into(),
        ));
    }
    let mut records = Vec::new();
    let mut identities = HashSet::new();
    for entity in entities {
        let mut cursor: Option<String> = None;
        let mut cursors = HashSet::new();
        loop {
            let page = source.fetch_page(entity, cursor.as_deref()).await?;
            for record in &page.records {
                let identity = &record.identity;
                if &identity.entity != entity
                    || identity.external_id.trim().is_empty()
                    || identity.external_id.len() > 256
                    || !identities.insert((
                        identity.source_provider_id,
                        identity.entity.as_str().to_owned(),
                        identity.external_id.clone(),
                    ))
                {
                    return Err(AppError::Validation(
                        "source page contains an invalid or duplicate identity".into(),
                    ));
                }
            }
            records.extend(page.records);
            match page.next_cursor {
                Some(next) => {
                    if next.is_empty() || !cursors.insert(next.clone()) {
                        return Err(AppError::Validation(
                            "source pagination cursor did not progress".into(),
                        ));
                    }
                    cursor = Some(next);
                }
                None => break,
            }
        }
    }
    let coverage = match source.source_provider_id() {
        Some(provider_id)
            if !provider_id.is_nil()
                && records
                    .iter()
                    .all(|r| r.identity.source_provider_id == provider_id) =>
        {
            let withheld = source.withheld_entities();
            if withheld.iter().any(|entity| entities.contains(entity)) {
                return Err(AppError::Validation(
                    "source manifest both traverses and withholds one entity".into(),
                ));
            }
            let records_digest = records_digest(&records)
                .ok_or_else(|| AppError::Validation("source snapshot encoding failed".into()))?;
            Some(CompletedSourceCoverage {
                provider_id,
                entities: entities.to_vec(),
                withheld,
                capability: source.capability_token().map(str::to_owned),
                records_digest,
                generation_binding: source.generation_binding(),
            })
        }
        _ => None,
    };
    Ok(SourceSnapshot { records, coverage })
}

// ---------------------------------------------------------------------------
// Typed local-inventory port
// ---------------------------------------------------------------------------

/// A checked scalar bound to a closed [`InventoryField`].
///
/// Values reach SQL only through this type: a provider value is classified, not
/// coerced, and an unexpected JSON shape becomes [`InventoryValue::Json`] rather
/// than silently stringifying.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InventoryValue {
    Null,
    Boolean { value: bool },
    Integer { value: i64 },
    Number { value: f64 },
    Text { value: String },
    Json { value: Value },
}

impl InventoryValue {
    #[must_use]
    pub fn from_json(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Boolean { value: *value },
            Value::Number(number) => match number.as_i64() {
                Some(value) => Self::Integer { value },
                None => Self::Number {
                    value: number.as_f64().unwrap_or(0.0),
                },
            },
            Value::String(value) => Self::Text {
                value: value.clone(),
            },
            Value::Array(_) | Value::Object(_) => Self::Json {
                value: value.clone(),
            },
        }
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// One prepared row: a closed target entity plus its typed field values.
pub trait TypedInventoryRow {
    fn entity(&self) -> InventoryEntity;
    fn value(&self, field: InventoryField) -> Option<&InventoryValue>;
}

/// A prepared batch of typed rows, produced by [`LocalInventoryTarget::prepare`]
/// and committed atomically by [`LocalInventoryTarget::commit_pull`].
pub trait TypedInventoryBatch: Send + Sync {
    type Row: TypedInventoryRow;

    fn rows(&self) -> &[Self::Row];

    /// Number of source records extracted into this batch.
    fn extracted_count(&self) -> u64 {
        self.rows().len() as u64
    }
}

/// Safe per-entity counters recorded in `sync_job.stats`.
///
/// Only aggregate counts and foreign-key counts are recorded; never raw field
/// values, provider bodies or credentials.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryCommitStats {
    pub extracted: u64,
    /// Committed rows, counted after the transaction commits.
    pub committed: u64,
    /// Referenced local rows resolved, keyed by canonical relation name.
    pub foreign_keys: BTreeMap<String, u64>,
    pub tag_memberships_committed: u64,
    pub entities_extracted: BTreeMap<String, u64>,
    pub entities_committed: BTreeMap<String, u64>,
    /// Optional-namespace capability the pull ran under, when the source has
    /// one. `present` imports the plugin; any other value retains earlier rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_capability: Option<String>,
    /// The absence reconciliation outcome of a snapshot pull, when the source
    /// carried a configured generation. `None` for every other job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<crate::contexts::sync::domain::ReconciliationReport>,
}

pub(crate) mod inventory_source_seal {
    pub trait Sealed {}
}

/// Read-only access to a remote inventory source, fetched page by page.
/// Implementations belong to this crate, not arbitrary event batches.
pub trait InventorySource: inventory_source_seal::Sealed + Send + Sync {
    /// The canonical source entities this source exposes.
    fn supported_entities(&self) -> Vec<SourceEntity>;

    /// Provider identity of this source, when known. This is not credential authority.
    fn source_provider_id(&self) -> Option<Uuid> {
        None
    }

    /// A binding captured by the configured source owner, if available.
    fn generation_binding(&self) -> Option<ConfiguredSourceEvidence> {
        None
    }

    /// Entities of an optional namespace this source deliberately does not
    /// traverse, because the provider does not expose it.
    ///
    /// They stay in the sealed manifest identity, so an unavailable optional
    /// namespace is never imported as a completed empty traversal and retains
    /// the rows earlier pulls created.
    fn withheld_entities(&self) -> Vec<SourceEntity> {
        Vec::new()
    }

    /// Optional-namespace capability this source runs under, for reporting.
    fn capability_token(&self) -> Option<&'static str> {
        None
    }

    /// Fetch one page for `entity`. A `next_cursor` of `None` ends the entity.
    fn fetch_page(
        &self,
        entity: &SourceEntity,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<SourcePage, AppError>> + Send;
}

/// Seal the source-owner evidence and the completed manifest for a mapping.
pub trait ConfiguredSourceGenerationAuthority: Send + Sync {
    fn capture(
        &self,
        config: &MappingConfigDocument,
        snapshot: &SourceSnapshot,
    ) -> Result<super::SourceGeneration, AppError>;
}

impl ConfiguredSourceGenerationAuthority for CompletedSourceCoverage {
    fn capture(
        &self,
        config: &MappingConfigDocument,
        snapshot: &SourceSnapshot,
    ) -> Result<super::SourceGeneration, AppError> {
        if !self.matches(snapshot, config) || config.status != MappingConfigurationStatus::Active {
            return Err(AppError::Validation(
                "sync.source_generation_coverage_mismatch".into(),
            ));
        }
        self.seal_mapping(config)
    }
}

/// One immutable mapping configuration version, resolved with its entity and
/// field mappings.
///
/// This is the single canonical aggregate: the persistence layer stores it and
/// the target port consumes it. There is no separate storage-only document
/// type (`MappingConfigurationDocument` is gone).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingConfigDocument {
    pub reference: MappingRef,
    pub name: String,
    pub description: Option<String>,
    pub source_provider_id: Uuid,
    pub target: MappingTarget,
    pub status: MappingConfigurationStatus,
    pub direction: MappingDirection,
    pub conflict_mode: ConflictMode,
    #[serde(default)]
    pub absence_policy: AbsencePolicy,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
    pub entity_mappings: Vec<EntityMapping>,
    pub field_mappings: Vec<FieldMapping>,
}

impl MappingConfigDocument {
    /// The job request this configuration authorises, subject to the supported
    /// shape gate in [`TargetSyncRequest::ensure_supported`].
    #[must_use]
    pub fn target_sync_request(&self) -> TargetSyncRequest {
        TargetSyncRequest {
            reference: self.reference,
            source_provider_id: self.source_provider_id,
            target: self.target.clone(),
            direction: self.direction,
            conflict_mode: self.conflict_mode,
        }
    }

    /// Canonical digest of the mapping content this version authorises.
    ///
    /// Only semantics enter the digest: the immutable reference and provider, the
    /// target and direction, and every enabled entity and field mapping with its
    /// transforms. Display metadata (name, description), row ids and timestamps
    /// are bookkeeping and are excluded. A configured-source generation binds this
    /// digest, so any mapping edit yields a new generation.
    #[must_use]
    pub fn content_digest(&self) -> String {
        let target = match &self.target {
            MappingTarget::LocalInventory => "local_inventory".to_string(),
            MappingTarget::Provider { provider_id } => format!("provider:{provider_id}"),
        };
        let mut parts: Vec<String> = vec![
            "piping.mapping-content/v1".to_string(),
            self.reference.to_wire(),
            self.source_provider_id.to_string(),
            target,
            self.direction.as_str().to_string(),
            self.conflict_mode.as_str().to_string(),
            self.absence_policy.as_str().to_string(),
        ];
        let mut entities: Vec<&EntityMapping> = self
            .entity_mappings
            .iter()
            .filter(|entity| entity.enabled)
            .collect();
        entities.sort_by(|left, right| {
            (left.source_entity.as_str(), left.target_entity.as_str())
                .cmp(&(right.source_entity.as_str(), right.target_entity.as_str()))
        });
        for entity in entities {
            parts.push(format!(
                "entity\u{1f}{}\u{1f}{}",
                entity.source_entity, entity.target_entity
            ));
            let mut fields: Vec<&FieldMapping> = self
                .field_mappings
                .iter()
                .filter(|field| field.entity_mapping_id == entity.id)
                .collect();
            fields.sort_by(|left, right| {
                (left.source_field.as_str(), left.target_field.as_str())
                    .cmp(&(right.source_field.as_str(), right.target_field.as_str()))
            });
            for field in fields {
                parts.push(format!(
                    "field\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    field.source_field,
                    field.target_field,
                    field.forward_transform_json,
                    field
                        .reverse_transform_json
                        .as_ref()
                        .map_or_else(|| "<none>".to_string(), ToString::to_string),
                    field.round_trip_critical
                ));
            }
        }
        crate::contexts::sync::domain::inventory_export::digest(parts.join("\u{1e}").as_bytes())
    }
}

/// One field of a local inventory target resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LocalInventoryResourceField {
    pub name: String,
    pub field_type: String,
    pub required: bool,
}

/// One local inventory target resource paired with its source entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LocalInventoryResource {
    pub source_entity: SourceEntity,
    pub target_entity: String,
    pub fields: Vec<LocalInventoryResourceField>,
}

/// Local inventory catalogue and identity owner, implemented by the target.
/// Resource routes use its schema view; mutation consumers share its bindings
/// instead of deriving identity from mutable provider fields.
pub trait LocalInventoryCatalogView: Send + Sync {
    fn resources(&self) -> Vec<LocalInventoryResource>;

    fn resource(&self, source_entity: &SourceEntity) -> Option<LocalInventoryResource>;

    fn binding_for_local(
        &self,
        entity: InventoryEntity,
        local_id: Uuid,
    ) -> futures_util::future::BoxFuture<'_, Result<Option<InventoryBinding>, AppError>>;

    /// Apply one authoritative remote record that already arrived through the
    /// provider's live event stream.
    ///
    /// The record is planned and committed through the same batch owner as a
    /// pull, but as a batch of exactly this record: nothing absent from the
    /// batch is withdrawn. A real `sync_job` row records the write.
    fn apply_live_record<'a>(
        &'a self,
        config: &'a MappingConfigDocument,
        record: &'a SourceRecord,
        authorisation: Arc<dyn InventoryCommitAuthorisation>,
    ) -> futures_util::future::BoxFuture<'a, Result<(), AppError>>;

    /// Delete the local row bound to `identity` under the inventory delete
    /// guard, and its binding, in one transaction.
    ///
    /// Returns `false` when no binding exists. Dependent local rows block the
    /// delete; this operation never cascades.
    fn delete_live_record<'a>(
        &'a self,
        config: &'a MappingConfigDocument,
        identity: &'a InventoryIdentity,
        authorisation: Arc<dyn InventoryCommitAuthorisation>,
    ) -> futures_util::future::BoxFuture<'a, Result<bool, AppError>>;
}

/// The immutable provider-qualified identity used by every inventory sync path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryIdentity {
    pub provider_id: Uuid,
    pub entity: InventoryEntity,
    pub external_id: String,
}

/// Canonical NetBox positive-integer identity shared by live and frozen sources.
/// Live callers may normalize digit strings; sealed artifacts must additionally
/// require equality with the stored spelling rather than rewrite captured bytes.
pub(crate) fn normalize_netbox_id(value: &Value) -> Result<String, AppError> {
    let id = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text)
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            text.parse::<u64>().ok()
        }
        _ => None,
    }
    .filter(|id| *id > 0)
    .ok_or_else(|| AppError::Validation("NetBox id must be a positive integer".into()))?;
    Ok(id.to_string())
}

impl InventoryIdentity {
    pub fn local_id(&self) -> Uuid {
        let namespace = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("piping.inventory.sync/provider/{}", self.provider_id).as_bytes(),
        );
        Uuid::new_v5(
            &namespace,
            format!(
                "{}\u{1f}{}\u{1f}{}",
                self.entity.source_key(),
                self.external_id,
                self.entity.target_key()
            )
            .as_bytes(),
        )
    }

    /// NetBox ids are positive integers, never URLs, names or local UUIDs.
    pub fn netbox(
        provider_id: Uuid,
        entity: InventoryEntity,
        value: &Value,
    ) -> Result<Self, AppError> {
        Ok(Self {
            provider_id,
            entity,
            external_id: normalize_netbox_id(value)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct InventoryBinding {
    pub identity: InventoryIdentity,
    pub local_id: Uuid,
}

pub enum OutboundBindingChange {
    None,
    Bind(InventoryBinding),
    Remove(InventoryBinding),
}

/// A prepared commit: the running job plus the batch to write in one
/// transaction with the job's terminal state.
///
/// The job must be marked completed only after the batch transaction commits.
pub struct CommitPull<B> {
    pub job: SyncJob,
    pub batch: B,
    pub authorisation: Arc<dyn InventoryCommitAuthorisation>,
}

/// Authority that must still hold when inventory writes become durable and when
/// a commit reads or mutates one persisted inventory object.
///
/// [`Self::ensure_owned`] is the writer's commit boundary. A provider lease is
/// only one of the things that must hold there; an authorisation that speaks for
/// a request must also re-check that request's current user, policy and token
/// ceilings.
///
/// [`Self::ensure_reference_access`] is the READ ceiling for one persisted local
/// inventory object, evaluated with the attributes read from that stored row.
///
/// [`Self::ensure_mutation_access`] is the object-level CREATE/UPDATE/DELETE ceiling for
/// the same object. An operation-level grant — the route's inventory EXECUTE or
/// the transfer's source UPDATE — is not enough on its own: a conditional object
/// DENY must be able to refuse the write.
///
/// There is deliberately no permissive default: an authorisation that cannot
/// name a request principal (a provider lease, for example) refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryCommitOperation {
    Delete,
}

pub trait InventoryCommitAuthorisation: Send + Sync {
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>>;

    /// Check a destructive operation independently of its number of candidates.
    fn ensure_operation_access(
        &self,
        _operation: InventoryCommitOperation,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async {
            Err(AppError::Forbidden(
                "Destructive operation not authorised".into(),
            ))
        })
    }

    /// Authorise a READ of the persisted inventory object `id` using
    /// `attributes` resolved from that row, never caller-supplied attributes.
    ///
    /// This is a check, not a write boundary: lease ownership stays with
    /// [`Self::ensure_owned`].
    fn ensure_reference_access(
        &self,
        id: Uuid,
        attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>>;

    /// Authorise a CREATE, UPDATE or DELETE of the inventory object `id`.
    ///
    /// Only [`Action::Create`], [`Action::Update`] and [`Action::Delete`] are representable; any other
    /// action is refused rather than downgraded to a READ or to the
    /// operation-level grant.
    ///
    /// `attributes` are the attributes the writer owns for that decision: the
    /// locked stored row for a DELETE or UPDATE of an existing object, the proposed merged
    /// canonical record for a CREATE or for a new UPDATE. They are never taken
    /// from the request body.
    fn ensure_mutation_access(
        &self,
        action: Action,
        id: Uuid,
        attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>>;
}

/// The typed local inventory target.
pub trait LocalInventoryTarget: Send + Sync {
    type Batch: TypedInventoryBatch;

    /// Turn a full snapshot into a typed batch, validating that every target
    /// entity and field exists in the closed schema and that every non-null
    /// target field is covered. Fails before any write.
    fn prepare(
        &self,
        config: &MappingConfigDocument,
        snapshot: &SourceSnapshot,
    ) -> Result<Self::Batch, AppError>;

    /// Commit the batch and the job's terminal state in one transaction.
    fn commit_pull(
        &self,
        commit: CommitPull<Self::Batch>,
    ) -> impl Future<Output = Result<SyncJob, AppError>> + Send;
}

// ---------------------------------------------------------------------------
// Supported-job gate
// ---------------------------------------------------------------------------

/// Who authorises a local write: a full snapshot job or one live record.
///
/// The two scopes differ only in the mapping shape they accept. Both write the
/// same closed schema through the same batch owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScope {
    /// A pull job that reads every enabled entity of the source.
    Snapshot,
    /// One record delivered by the provider's live event stream.
    LiveRecord,
}

/// The only supported job shape: write from a provider into the local
/// inventory with NetBox-wins conflict resolution.
///
/// Anything else — push, a provider target, or a non-NetBox-wins conflict mode
/// — is rejected explicitly before any write. A snapshot job must pull;
/// the live-record scope additionally accepts the bidirectional mapping that
/// carries the inbound event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSyncRequest {
    pub reference: MappingRef,
    pub source_provider_id: Uuid,
    pub target: MappingTarget,
    pub direction: MappingDirection,
    pub conflict_mode: ConflictMode,
}

impl TargetSyncRequest {
    pub fn ensure_supported(&self) -> Result<(), AppError> {
        self.ensure_supported_for(SyncScope::Snapshot)
    }

    pub fn ensure_supported_for(&self, scope: SyncScope) -> Result<(), AppError> {
        let direction_supported = match scope {
            SyncScope::Snapshot => self.direction == MappingDirection::Pull,
            SyncScope::LiveRecord => self.direction == MappingDirection::Bidirectional,
        };
        if !direction_supported {
            return Err(AppError::Validation(match scope {
                SyncScope::Snapshot => {
                    "only pull sync into the local inventory is supported".to_string()
                }
                SyncScope::LiveRecord => {
                    "only bidirectional mappings authorise a live record write".to_string()
                }
            }));
        }
        if self.target != MappingTarget::LocalInventory {
            return Err(AppError::Validation(
                "only the local inventory target is supported".to_string(),
            ));
        }
        if self.conflict_mode != ConflictMode::NetboxWins {
            return Err(AppError::Validation(
                "only netbox_wins conflict mode is supported".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct GenerationSource {
        evidence: ConfiguredSourceEvidence,
        fail: bool,
    }

    impl inventory_source_seal::Sealed for GenerationSource {}

    impl InventorySource for GenerationSource {
        fn supported_entities(&self) -> Vec<SourceEntity> {
            vec![entity("dcim.sites")]
        }

        fn source_provider_id(&self) -> Option<Uuid> {
            Some(self.evidence.0.provider_id)
        }

        fn generation_binding(&self) -> Option<ConfiguredSourceEvidence> {
            Some(self.evidence.clone())
        }

        async fn fetch_page(
            &self,
            _: &SourceEntity,
            _: Option<&str>,
        ) -> Result<SourcePage, AppError> {
            if self.fail {
                return Err(AppError::Validation("collection failed".into()));
            }
            Ok(SourcePage {
                records: vec![],
                next_cursor: None,
            })
        }
    }

    #[tokio::test]
    async fn completed_collection_carries_binding_and_failure_mints_no_snapshot() {
        let evidence = ConfiguredSourceEvidence::from_binding(
            super::super::absence::ProviderGenerationBinding {
                provider_id: Uuid::from_u128(7),
                base_url: "https://netbox.example.com".into(),
                sync_mode: Some("pull_only".into()),
                credential: super::super::absence::SourceCredentialRevision {
                    secret_id: Uuid::from_u128(8),
                    revision: "revision-one".into(),
                },
            },
        );
        let entities = vec![entity("dcim.sites")];
        let source = GenerationSource {
            evidence: evidence.clone(),
            fail: false,
        };
        let snapshot = collect_snapshot(&source, &entities)
            .await
            .expect("complete");
        assert_eq!(
            snapshot.coverage.as_ref().unwrap().generation_binding(),
            Some(&evidence)
        );
        let now = chrono::Utc::now();
        let reference = MappingRef::new(Uuid::from_u128(9), 1);
        let mut config = MappingConfigDocument {
            reference,
            name: "test".into(),
            description: None,
            source_provider_id: Uuid::from_u128(7),
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            absence_policy: AbsencePolicy::DeleteSourceOwned,
            created_by: None,
            created_at: now,
            retired_at: None,
            entity_mappings: vec![EntityMapping {
                id: Uuid::from_u128(10),
                mapping_config_logical_id: reference.logical_id,
                mapping_config_version_id: reference.version_id,
                source_entity: "dcim.sites".into(),
                target_entity: "dcim.site".into(),
                enabled: true,
                created_at: now,
            }],
            field_mappings: vec![],
        };
        let coverage = snapshot.coverage.as_ref().expect("coverage");
        let original = coverage.capture(&config, &snapshot).expect("generation");
        config.absence_policy = AbsencePolicy::RetainAndReport;
        assert_ne!(
            original,
            coverage
                .capture(&config, &snapshot)
                .expect("changed policy")
        );
        config.status = MappingConfigurationStatus::Retired;
        assert!(coverage.capture(&config, &snapshot).is_err());
        config.status = MappingConfigurationStatus::Active;
        config.entity_mappings[0].source_entity = "dcim.regions".into();
        assert!(coverage.capture(&config, &snapshot).is_err());
        let failed = GenerationSource {
            evidence,
            fail: true,
        };
        assert!(collect_snapshot(&failed, &entities).await.is_err());
    }

    #[test]
    fn netbox_identity_is_provider_qualified_and_preserves_pull_uuid() {
        let provider = Uuid::from_u128(1);
        let identity =
            InventoryIdentity::netbox(provider, InventoryEntity::Site, &json!(42)).expect("id");
        assert_eq!(
            identity.local_id(),
            Uuid::parse_str("e84134c4-8834-5a56-99af-f76d94d8225d").expect("UUID")
        );
        assert_eq!(
            identity,
            InventoryIdentity::netbox(provider, InventoryEntity::Site, &json!("0042"))
                .expect("numeric string")
        );
        assert_ne!(
            identity.local_id(),
            InventoryIdentity::netbox(Uuid::from_u128(2), InventoryEntity::Site, &json!(42))
                .expect("other source")
                .local_id()
        );
    }

    #[test]
    fn netbox_identity_rejects_path_uuid_empty_and_nonscalar_ids() {
        for value in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!(true),
            json!(null),
            json!({"id":42}),
            json!([]),
            json!(""),
            json!(" 42 "),
            json!("../42"),
            json!("42/"),
            json!("00000000-0000-0000-0000-000000000001"),
        ] {
            assert!(
                InventoryIdentity::netbox(Uuid::from_u128(1), InventoryEntity::Site, &value)
                    .is_err(),
                "{value}"
            );
        }
    }

    fn entity(raw: &str) -> SourceEntity {
        SourceEntity::new(raw).expect("valid entity")
    }

    #[test]
    fn mapping_ref_round_trips_canonical_wire_form() {
        let logical_id = Uuid::now_v7();
        let reference = MappingRef::new(logical_id, 3);
        let wire = reference.to_wire();
        assert_eq!(wire, format!("{logical_id}:3"));
        assert_eq!(MappingRef::parse(&wire).expect("parse"), reference);
    }

    #[test]
    fn mapping_ref_rejects_non_canonical_versions() {
        assert!(MappingRef::parse("v1").is_err());
        assert!(MappingRef::parse("not-a-uuid:1").is_err());
        assert!(
            MappingRef::parse(&format!("{}:0", Uuid::now_v7())).is_err(),
            "version must be positive"
        );
    }

    #[test]
    fn mapping_target_maps_to_nullable_provider_column() {
        assert_eq!(
            MappingTarget::from_provider_column(None),
            MappingTarget::LocalInventory
        );
        let provider_id = Uuid::now_v7();
        let target = MappingTarget::from_provider_column(Some(provider_id));
        assert_eq!(target, MappingTarget::Provider { provider_id });
        assert_eq!(target.to_provider_column(), Some(provider_id));
        assert_eq!(MappingTarget::LocalInventory.to_provider_column(), None);
    }

    #[test]
    fn mapping_target_serializes_as_tagged_wire() {
        assert_eq!(
            serde_json::to_value(MappingTarget::LocalInventory).expect("serialize"),
            json!({ "kind": "local_inventory" })
        );
        let provider_id = Uuid::now_v7();
        let value =
            serde_json::to_value(MappingTarget::Provider { provider_id }).expect("serialize");
        assert_eq!(
            value,
            json!({ "kind": "provider", "provider_id": provider_id })
        );
    }

    #[test]
    fn source_entity_accepts_canonical_plurals_only() {
        assert_eq!(entity("dcim.sites").as_str(), "dcim.sites");
        assert_eq!(entity("ipam.ip-addresses").as_str(), "ipam.ip-addresses");
        assert!(SourceEntity::new("dcim/sites").is_err());
        assert!(SourceEntity::new("Sites").is_err());
        assert!(SourceEntity::new("dcim..sites").is_err());
        assert!(SourceEntity::new("").is_err());
    }

    #[test]
    fn source_identity_rejects_empty_external_id() {
        let provider_id = Uuid::now_v7();
        assert!(SourceIdentity::new(provider_id, entity("dcim.sites"), "   ").is_err());
        let identity = SourceIdentity::new(provider_id, entity("dcim.sites"), "42").expect("ok");
        assert_eq!(identity.external_id, "42");
    }

    #[test]
    fn snapshot_reports_distinct_entities() {
        let provider_id = Uuid::now_v7();
        let record = |raw: &str, id: &str| SourceRecord {
            identity: SourceIdentity::new(provider_id, entity(raw), id).expect("identity"),
            fields: Map::new(),
        };
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![
                record("dcim.sites", "1"),
                record("dcim.sites", "2"),
                record("dcim.regions", "3"),
            ],
        };
        let entities: Vec<&str> = snapshot
            .entities()
            .into_iter()
            .map(SourceEntity::as_str)
            .collect();
        assert_eq!(entities, vec!["dcim.sites", "dcim.regions"]);
    }

    #[test]
    fn inventory_value_classifies_without_coercion() {
        assert_eq!(
            InventoryValue::from_json(&json!(null)),
            InventoryValue::Null
        );
        assert_eq!(
            InventoryValue::from_json(&json!(7)),
            InventoryValue::Integer { value: 7 }
        );
        assert_eq!(
            InventoryValue::from_json(&json!("text")),
            InventoryValue::Text {
                value: "text".to_string()
            }
        );
        assert!(matches!(
            InventoryValue::from_json(&json!([1, 2])),
            InventoryValue::Json { .. }
        ));
    }

    #[test]
    fn unsupported_job_shapes_are_rejected() {
        let reference = MappingRef::new(Uuid::now_v7(), 1);
        let source_provider_id = Uuid::now_v7();
        let local = TargetSyncRequest {
            reference,
            source_provider_id,
            target: MappingTarget::LocalInventory,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
        };
        assert!(local.ensure_supported().is_ok());

        let mut push = local.clone();
        push.direction = MappingDirection::Push;
        assert!(push.ensure_supported().is_err());

        let mut provider_target = local.clone();
        provider_target.target = MappingTarget::Provider {
            provider_id: Uuid::now_v7(),
        };
        assert!(provider_target.ensure_supported().is_err());

        let mut piping_wins = local;
        piping_wins.conflict_mode = ConflictMode::PipingWins;
        assert!(piping_wins.ensure_supported().is_err());
    }
}
