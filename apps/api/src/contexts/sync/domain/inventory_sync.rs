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

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

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
}

impl SourceSnapshot {
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
}

/// Read-only access to a remote inventory source, fetched page by page.
pub trait InventorySource: Send + Sync {
    /// The canonical source entities this source exposes.
    fn supported_entities(&self) -> Vec<SourceEntity>;

    /// Fetch one page for `entity`. A `next_cursor` of `None` ends the entity.
    fn fetch_page(
        &self,
        entity: &SourceEntity,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<SourcePage, AppError>> + Send;
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

    fn apply_site_event<'a>(
        &'a self,
        identity: &'a InventoryIdentity,
        mutation: SiteWebhookMutation,
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

/// Webhooks may patch only these fields; defaults are insert-only.
pub enum SiteWebhookMutation {
    Upsert {
        create: bool,
        name: Option<String>,
        slug: Option<String>,
    },
    Delete,
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
/// [`Self::ensure_mutation_access`] is the object-level CREATE/UPDATE ceiling for
/// the same object. An operation-level grant — the route's inventory EXECUTE or
/// the transfer's source UPDATE — is not enough on its own: a conditional object
/// DENY must be able to refuse the write.
///
/// There is deliberately no permissive default: an authorisation that cannot
/// name a request principal (a provider lease, for example) refuses.
pub trait InventoryCommitAuthorisation: Send + Sync {
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>>;

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

    /// Authorise a CREATE or UPDATE of the inventory object `id`.
    ///
    /// Only [`Action::Create`] and [`Action::Update`] are representable; any other
    /// action is refused rather than downgraded to a READ or to the
    /// operation-level grant.
    ///
    /// `attributes` are the attributes the writer owns for that decision: the
    /// stored row for an UPDATE of an existing object, the proposed merged
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

/// The only supported job shape: pull from a provider into the local inventory
/// with NetBox-wins conflict resolution.
///
/// Anything else — push, bidirectional, a provider target, or a non-NetBox-wins
/// conflict mode — is rejected explicitly before any write.
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
        if self.direction != MappingDirection::Pull {
            return Err(AppError::Validation(
                "only pull sync into the local inventory is supported".to_string(),
            ));
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
