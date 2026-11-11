//! Typed source-ownership proof of native inventory units (PIP-606).
//!
//! A proof states one narrow fact: the sync writer observed the unit absent from
//! a completed source traversal and created it inside that same transaction.
//! Value equality never establishes it, and neither does a binding — a binding
//! records that a provider identity maps to a local row, not who created the row.
//!
//! The durable owner is `sync_inventory_ownership`. This module owns the typed
//! vocabulary it stores and the transitions a writer may apply; the native
//! lifecycle generation it compares against lives in `sync_native_unit_lifecycle`
//! and is maintained by the triggers installed by migration 0027. A unit's
//! identity is native throughout: the owning table, the row, and the membership
//! member. Source ids, names and payload values are never identity here.
//!
//! Not every native unit is covered. A table outside the installed lifecycle
//! catalogue has no generation, so a proof for it is refused rather than assumed
//! safe; coverage is a property of the installed schema, not of this module.

use chrono::{DateTime, Utc};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::contexts::sync::domain::inventory_target_schema::{
    ENTITIES, FieldKind, InventoryEntity,
};
use crate::contexts::sync::domain::{InventoryIdentity, SourceIdentity};
use crate::errors::AppError;

/// Digest encoding version of [`UnitDigest`]. A different digest algorithm is a
/// new version, never a silent redefinition of a stored one.
pub const UNIT_DIGEST_VERSION: i16 = 1;

/// Stored marker for a unit without a membership member. The column is a single
/// composite identity, so "no member" is a value rather than a NULL branch.
pub const NO_UNIT_MEMBER: Uuid = Uuid::nil();

fn invalid(code: &'static str) -> AppError {
    AppError::Validation(format!("inventory_ownership:{code}"))
}

/// Kinds of native unit a proof can cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NativeUnitKind {
    /// One native record row.
    Record,
    /// One native `tags text[]` membership on a record row.
    TagMembership,
    /// One native junction pair, named by its owner row and member row.
    JunctionMember,
    /// One cable attachment edge on an interface row.
    CableAttachment,
}

impl NativeUnitKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::TagMembership => "tag_membership",
            Self::JunctionMember => "junction_member",
            Self::CableAttachment => "cable_attachment",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "record" => Some(Self::Record),
            "tag_membership" => Some(Self::TagMembership),
            "junction_member" => Some(Self::JunctionMember),
            "cable_attachment" => Some(Self::CableAttachment),
            _ => None,
        }
    }

    /// Whether the identity includes a membership member.
    #[must_use]
    pub fn carries_member(self) -> bool {
        matches!(
            self,
            Self::TagMembership | Self::JunctionMember | Self::CableAttachment
        )
    }
}

/// The native cable end of one attachment edge.
///
/// A cable attachment is not the holder row: the same interface can carry cable
/// A on end `A` and later cable B on end `B`, and those are two different edges.
/// The end is part of the stored identity, so observing a new edge can never
/// clear or satisfy the confirmation of the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CableEnd {
    A,
    B,
}

impl CableEnd {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    /// Normalise the native `dcim_interface.cable_end` vocabulary. The column is
    /// `character` with a CHECK on exactly these two values.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            _ => Err(invalid("unit_cable_end")),
        }
    }
}

/// One native inventory unit, named by native keys only.
///
/// Construction validates the table against the closed target catalogue, so a
/// caller cannot name a table the sync schema does not own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NativeUnit {
    kind: NativeUnitKind,
    table: &'static str,
    id: Uuid,
    member: Option<Uuid>,
    token: &'static str,
}

impl NativeUnit {
    /// One native record row of `entity`.
    pub fn record(entity: InventoryEntity, id: Uuid) -> Result<Self, AppError> {
        let table = entity.table().ok_or_else(|| invalid("unit_table"))?;
        Ok(Self {
            kind: NativeUnitKind::Record,
            table,
            id,
            member: None,
            token: "",
        })
    }

    /// One cable attachment edge on the interface row `id`: `cable` on `end`.
    ///
    /// The holder row alone is not the edge. Native storage carries `cable_id`,
    /// `cable_end` and `cable_connector` on the interface row, so both the cable
    /// and the end enter the identity.
    pub fn cable_attachment(
        entity: InventoryEntity,
        id: Uuid,
        cable: Uuid,
        end: CableEnd,
    ) -> Result<Self, AppError> {
        if !matches!(
            entity,
            InventoryEntity::Interface | InventoryEntity::VmInterface
        ) {
            return Err(invalid("unit_cable_entity"));
        }
        if cable == NO_UNIT_MEMBER {
            return Err(invalid("unit_cable_member"));
        }
        let table = entity.table().ok_or_else(|| invalid("unit_table"))?;
        Ok(Self {
            kind: NativeUnitKind::CableAttachment,
            table,
            id,
            member: Some(cable),
            token: end.as_str(),
        })
    }

    /// One `tags text[]` membership of `tag` on the record row `id`.
    pub fn tag_membership(entity: InventoryEntity, id: Uuid, tag: Uuid) -> Result<Self, AppError> {
        if !entity.has_tags() {
            return Err(invalid("unit_tag_table"));
        }
        if tag == NO_UNIT_MEMBER {
            return Err(invalid("unit_tag_member"));
        }
        let table = entity.table().ok_or_else(|| invalid("unit_table"))?;
        Ok(Self {
            kind: NativeUnitKind::TagMembership,
            table,
            id,
            member: Some(tag),
            token: "",
        })
    }

    /// One member `member` of the junction field `field` on the owner row `id`.
    ///
    /// The junction table comes from the field catalogue, so the identity cannot
    /// name a relation the mapped schema does not write.
    pub fn junction_member(
        entity: InventoryEntity,
        field: &str,
        id: Uuid,
        member: Uuid,
    ) -> Result<Self, AppError> {
        let field = entity
            .field(field)
            .ok_or_else(|| invalid("unit_junction_field"))?;
        let table = match field.kind() {
            FieldKind::Junction { table, .. } | FieldKind::ContentTypeJunction { table, .. } => {
                table
            }
            _ => return Err(invalid("unit_junction_field")),
        };
        if member == NO_UNIT_MEMBER {
            return Err(invalid("unit_junction_member"));
        }
        Ok(Self {
            kind: NativeUnitKind::JunctionMember,
            table,
            id,
            member: Some(member),
            token: "",
        })
    }

    /// Restore a stored unit identity, re-validating the kind/member shape and
    /// the table catalogue. Stored rows are never trusted as categories.
    pub fn from_stored(
        kind: &str,
        table: &str,
        id: Uuid,
        member_column: Uuid,
        member_token: &str,
    ) -> Result<Self, AppError> {
        let kind = NativeUnitKind::from_wire(kind).ok_or_else(|| invalid("unit_kind"))?;
        let catalogue_table = self::catalogue_table(table).ok_or_else(|| invalid("unit_table"))?;
        let member = if kind.carries_member() {
            if member_column == NO_UNIT_MEMBER {
                return Err(invalid("unit_member"));
            }
            Some(member_column)
        } else {
            if member_column != NO_UNIT_MEMBER {
                return Err(invalid("unit_member"));
            }
            None
        };
        let token: &'static str = match (kind, member_token) {
            (NativeUnitKind::CableAttachment, raw) => CableEnd::parse(raw)?.as_str(),
            (_, "") => "",
            (_, _) => return Err(invalid("unit_member_token")),
        };
        Ok(Self {
            kind,
            table: catalogue_table,
            id,
            member,
            token,
        })
    }

    #[must_use]
    pub fn kind(&self) -> NativeUnitKind {
        self.kind
    }

    #[must_use]
    pub fn table(&self) -> &'static str {
        self.table
    }

    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub fn member(&self) -> Option<Uuid> {
        self.member
    }

    /// The member column value stored for this unit.
    #[must_use]
    pub fn member_column(&self) -> Uuid {
        self.member.unwrap_or(NO_UNIT_MEMBER)
    }

    /// The native member qualifier stored for this unit.
    #[must_use]
    pub fn member_token_column(&self) -> &'static str {
        self.token
    }

    /// The native lifecycle entry this unit's proof depends on.
    ///
    /// A record, a tag membership and a cable attachment are gated by their own
    /// row. A junction member is gated by the junction pair, because replacing a
    /// membership leaves the owner row untouched and equality would not see it.
    pub fn lifecycle_anchor(&self) -> Result<(&'static str, String), AppError> {
        if self.kind == NativeUnitKind::JunctionMember {
            let member = self.member.ok_or_else(|| invalid("unit_member"))?;
            let (local, remote) =
                junction_columns(self.table).ok_or_else(|| invalid("unit_junction"))?;
            let key = if local < remote {
                format!("{}/{}", self.id, member)
            } else {
                format!("{}/{}", member, self.id)
            };
            return Ok((self.table, key));
        }
        Ok((self.table, self.id.to_string()))
    }
}

/// Resolve a stored native table to the catalogue name that owns it, either as a
/// record table or as a junction table written by one of the mapped fields.
///
/// A junction unit's table is the junction itself, never its owner's record
/// table: the stored identity must round-trip to the same native key it was minted
/// from, and the lifecycle anchor of a junction is the junction pair.
fn catalogue_table(candidate: &str) -> Option<&'static str> {
    if let Some(meta) = ENTITIES.iter().find(|meta| meta.table == candidate) {
        return Some(meta.table);
    }
    ENTITIES.iter().find_map(|meta| {
        meta.fields.iter().find_map(|spec| match spec.kind {
            FieldKind::Junction { table, .. } | FieldKind::ContentTypeJunction { table, .. }
                if table == candidate =>
            {
                Some(table)
            }
            _ => None,
        })
    })
}

/// The junction's local and remote column names, in catalogue order.
fn junction_columns(table: &str) -> Option<(&'static str, &'static str)> {
    ENTITIES.iter().find_map(|meta| {
        meta.fields.iter().find_map(|spec| match spec.kind {
            FieldKind::Junction {
                table: candidate,
                local_column,
                remote_column,
                ..
            }
            | FieldKind::ContentTypeJunction {
                table: candidate,
                local_column,
                remote_column,
            } if candidate == table => Some((local_column, remote_column)),
            _ => None,
        })
    })
}

/// Versioned digest of the native state the writer last applied.
///
/// Only the hash is stored: no field values and no secret-bearing content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDigest {
    version: i16,
    hex: String,
}

impl UnitDigest {
    /// Digest version 1: length-prefixed parts through SHA-256.
    pub fn of_v1(parts: &[&[u8]]) -> Self {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part);
        }
        Self {
            version: UNIT_DIGEST_VERSION,
            hex: hex::encode(hasher.finalize()),
        }
    }

    /// Restore a stored digest, refusing an unknown version or a malformed value.
    pub fn from_stored(version: i16, hex: &str) -> Result<Self, AppError> {
        if version != UNIT_DIGEST_VERSION {
            return Err(invalid("unit_digest_version"));
        }
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("unit_digest"));
        }
        Ok(Self {
            version,
            hex: hex.to_string(),
        })
    }

    #[must_use]
    pub fn version(&self) -> i16 {
        self.version
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.hex
    }
}

/// Lifecycle state of one ownership claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipState {
    /// The claim is current and may gate a removal.
    Active,
    /// The unit was touched by something other than its owning source write.
    Relinquished,
    /// The unit no longer exists.
    Deleted,
}

impl OwnershipState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Relinquished => "relinquished",
            Self::Deleted => "deleted",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "relinquished" => Some(Self::Relinquished),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// Why a claim stopped being active. The vocabulary is closed so a report can
/// never carry free-form text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelinquishReason {
    /// A local writer changed editable state or a relation of the unit.
    LocallyModified,
    /// The native lifecycle generation moved past the last applied one.
    LifecycleInvalidated,
    /// Another provider already owns the physical unit.
    OwnerConflict,
    /// The owning source record was removed by its owner.
    SourceDeleted,
}

impl RelinquishReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocallyModified => "locally_modified",
            Self::LifecycleInvalidated => "lifecycle_invalidated",
            Self::OwnerConflict => "owner_conflict",
            Self::SourceDeleted => "source_deleted",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "locally_modified" => Some(Self::LocallyModified),
            "lifecycle_invalidated" => Some(Self::LifecycleInvalidated),
            "owner_conflict" => Some(Self::OwnerConflict),
            "source_deleted" => Some(Self::SourceDeleted),
            _ => None,
        }
    }
}

/// The native state one writer applied, comparable with the stored proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedUnitState {
    pub job_id: Uuid,
    pub mapping: crate::contexts::sync::domain::MappingRef,
    pub digest: UnitDigest,
    pub lifecycle_generation: i64,
    pub applied_at: DateTime<Utc>,
}

/// What a writer must present to mint a claim: the unit it is creating, the
/// source record it observed absent, and the job and mapping version that made
/// the decision.
#[derive(Debug, Clone)]
pub struct UnitMint {
    pub unit: NativeUnit,
    pub identity: SourceIdentity,
    pub job_id: Uuid,
    pub mapping: crate::contexts::sync::domain::MappingRef,
    pub created_at: DateTime<Utc>,
    pub applied: AppliedUnitState,
}

/// One durable ownership claim.
#[derive(Debug, Clone)]
pub struct UnitOwnership {
    pub unit: NativeUnit,
    pub provider_id: Uuid,
    pub source_entity: String,
    pub external_id: String,
    pub state: OwnershipState,
    pub created_job_id: Uuid,
    pub created_mapping: crate::contexts::sync::domain::MappingRef,
    pub created_at: DateTime<Utc>,
    pub applied: AppliedUnitState,
    pub relinquished_at: Option<DateTime<Utc>>,
    pub relinquished_reason: Option<RelinquishReason>,
}

impl UnitOwnership {
    /// Mint a claim for a unit the caller is creating in this transaction.
    ///
    /// The identity must be the same provider and source record the claim is
    /// about; a claim borrowed from another source is not creation provenance.
    pub fn mint(mint: UnitMint) -> Result<Self, AppError> {
        if mint.identity.entity.as_str().is_empty() {
            return Err(invalid("unit_source_entity"));
        }
        if mint.job_id.is_nil() || mint.mapping.version_id <= 0 || mint.mapping.logical_id.is_nil()
        {
            return Err(invalid("unit_job_or_mapping"));
        }
        if mint.applied.lifecycle_generation < 1 {
            return Err(invalid("unit_generation"));
        }
        if mint.applied.job_id != mint.job_id || mint.applied.mapping != mint.mapping {
            return Err(invalid("unit_applied_job"));
        }
        if mint.applied.applied_at < mint.created_at {
            return Err(invalid("unit_applied_time"));
        }
        Ok(Self {
            provider_id: mint.identity.source_provider_id,
            source_entity: mint.identity.entity.to_string(),
            external_id: mint.identity.external_id.clone(),
            state: OwnershipState::Active,
            created_job_id: mint.job_id,
            created_mapping: mint.mapping,
            created_at: mint.created_at,
            applied: mint.applied,
            relinquished_at: None,
            relinquished_reason: None,
            unit: mint.unit,
        })
    }

    /// The provider-qualified identity this claim was minted from.
    pub fn source_identity(&self) -> Result<InventoryIdentity, AppError> {
        let entity = crate::contexts::sync::domain::SourceEntity::new(&self.source_entity)?;
        Ok(InventoryIdentity {
            provider_id: self.provider_id,
            entity: InventoryEntity::from_source_key(entity.as_str())
                .ok_or_else(|| invalid("unit_source_entity"))?,
            external_id: self.external_id.clone(),
        })
    }

    /// Apply newer native state. Only an active claim may be refreshed, and only
    /// by the provider that minted it; a writer refreshes after verifying the
    /// proof intact and after its own writes, never before.
    pub fn refresh(&mut self, applied: AppliedUnitState) -> Result<(), AppError> {
        if self.state != OwnershipState::Active {
            return Err(invalid("unit_not_active"));
        }
        if applied.lifecycle_generation < 1 {
            return Err(invalid("unit_generation"));
        }
        if applied.applied_at < self.applied.applied_at {
            return Err(invalid("unit_applied_time"));
        }
        self.applied = applied;
        Ok(())
    }

    /// Stop the claim from gating a removal. Relinquished claims are retained.
    pub fn relinquish(
        &mut self,
        reason: RelinquishReason,
        at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        if self.state != OwnershipState::Active {
            return Err(invalid("unit_not_active"));
        }
        self.state = OwnershipState::Relinquished;
        self.relinquished_reason = Some(reason);
        self.relinquished_at = Some(at);
        Ok(())
    }

    /// Record that the unit no longer exists. The claim is retained.
    pub fn mark_deleted(&mut self, at: DateTime<Utc>) -> Result<(), AppError> {
        if self.state != OwnershipState::Active {
            return Err(invalid("unit_not_active"));
        }
        self.state = OwnershipState::Deleted;
        self.relinquished_reason = Some(RelinquishReason::SourceDeleted);
        self.relinquished_at = Some(at);
        Ok(())
    }

    /// Whether this claim may still gate a removal.
    #[must_use]
    pub fn gates_removal(&self) -> bool {
        self.state == OwnershipState::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> Uuid {
        Uuid::from_u128(7)
    }

    fn identity(entity: &str, external: &str) -> SourceIdentity {
        SourceIdentity::new(
            provider(),
            crate::contexts::sync::domain::SourceEntity::new(entity).expect("entity"),
            external,
        )
        .expect("identity")
    }

    fn applied(job: Uuid) -> AppliedUnitState {
        AppliedUnitState {
            job_id: job,
            mapping: crate::contexts::sync::domain::MappingRef::new(Uuid::from_u128(3), 2),
            digest: UnitDigest::of_v1(&[b"device"]),
            lifecycle_generation: 4,
            applied_at: Utc::now(),
        }
    }

    fn record_unit() -> NativeUnit {
        NativeUnit::record(InventoryEntity::Device, Uuid::from_u128(11)).expect("unit")
    }

    #[test]
    fn a_record_unit_is_named_by_its_native_row_only() {
        let unit = record_unit();
        assert_eq!(unit.kind(), NativeUnitKind::Record);
        assert_eq!(unit.table(), "dcim_device");
        assert_eq!(unit.member_column(), NO_UNIT_MEMBER);
        assert_eq!(
            unit.lifecycle_anchor().expect("anchor"),
            ("dcim_device", Uuid::from_u128(11).to_string())
        );
    }

    #[test]
    fn unknown_tables_and_members_are_refused() {
        assert!(
            NativeUnit::from_stored("record", "identity_user", Uuid::nil(), NO_UNIT_MEMBER, "")
                .is_err()
        );
        assert!(
            NativeUnit::from_stored("record", "dcim_device", Uuid::nil(), Uuid::from_u128(2), "")
                .is_err()
        );
        assert!(
            NativeUnit::from_stored("membership", "dcim_device", Uuid::nil(), NO_UNIT_MEMBER, "")
                .is_err()
        );
        assert!(
            NativeUnit::tag_membership(InventoryEntity::Device, Uuid::nil(), NO_UNIT_MEMBER)
                .is_err()
        );
        assert!(
            NativeUnit::cable_attachment(
                InventoryEntity::Site,
                Uuid::nil(),
                Uuid::from_u128(3),
                CableEnd::A
            )
            .is_err()
        );
        assert!(
            NativeUnit::from_stored(
                "cable_attachment",
                "dcim_interface",
                Uuid::nil(),
                Uuid::from_u128(3),
                "C"
            )
            .is_err()
        );
        assert!(
            NativeUnit::from_stored(
                "cable_attachment",
                "dcim_interface",
                Uuid::nil(),
                Uuid::from_u128(3),
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn a_tag_membership_member_is_part_of_the_identity() {
        let unit = NativeUnit::tag_membership(
            InventoryEntity::Device,
            Uuid::from_u128(11),
            Uuid::from_u128(5),
        )
        .expect("unit");
        assert_eq!(unit.member(), Some(Uuid::from_u128(5)));
        assert_eq!(unit.member_column(), Uuid::from_u128(5));
        assert_eq!(unit.member_token_column(), "");
        assert!(unit.lifecycle_anchor().is_ok());
    }

    #[test]
    fn a_junction_member_is_gated_by_the_junction_pair() {
        let unit = NativeUnit::junction_member(
            InventoryEntity::Interface,
            "tagged_vlans",
            Uuid::from_u128(11),
            Uuid::from_u128(12),
        )
        .expect("unit");
        assert_eq!(unit.kind(), NativeUnitKind::JunctionMember);
        assert_eq!(unit.table(), "dcim_interface_tagged_vlans");
        let (table, key) = unit.lifecycle_anchor().expect("anchor");
        assert_eq!(table, "dcim_interface_tagged_vlans");
        assert!(key.ends_with(&Uuid::from_u128(12).to_string()));
    }

    #[test]
    fn every_catalogue_junction_round_trips_through_its_stored_identity() {
        let cases = [
            (InventoryEntity::Interface, "tagged_vlans"),
            (InventoryEntity::VmInterface, "tagged_vlans"),
            (InventoryEntity::Contact, "groups"),
            (InventoryEntity::Tag, "object_types"),
            (InventoryEntity::CustomField, "object_types"),
        ];
        for (entity, field) in cases {
            let unit = NativeUnit::junction_member(
                entity,
                field,
                Uuid::from_u128(11),
                Uuid::from_u128(12),
            )
            .expect("junction unit");
            let restored = NativeUnit::from_stored(
                unit.kind().as_str(),
                unit.table(),
                unit.id(),
                unit.member_column(),
                unit.member_token_column(),
            )
            .expect("restored junction unit");
            assert_eq!(restored, unit, "{}::{field}", entity.source_key());
            assert_eq!(
                restored.table(),
                unit.table(),
                "a junction unit must keep the junction table, not its owner record table"
            );
            assert_ne!(restored.table(), entity.table().expect("record table"));
            assert_eq!(
                restored.lifecycle_anchor().expect("anchor"),
                unit.lifecycle_anchor().expect("anchor")
            );
        }
    }

    #[test]
    fn two_cable_edges_on_one_holder_are_different_units() {
        let holder = Uuid::from_u128(11);
        let old_edge = NativeUnit::cable_attachment(
            InventoryEntity::Interface,
            holder,
            Uuid::from_u128(31),
            CableEnd::A,
        )
        .expect("old edge");
        let new_edge = NativeUnit::cable_attachment(
            InventoryEntity::Interface,
            holder,
            Uuid::from_u128(32),
            CableEnd::B,
        )
        .expect("new edge");
        assert_ne!(old_edge, new_edge);
        assert_ne!(old_edge.member(), new_edge.member());
        assert_ne!(
            old_edge.member_token_column(),
            new_edge.member_token_column()
        );

        let same_cable_other_end = NativeUnit::cable_attachment(
            InventoryEntity::Interface,
            holder,
            Uuid::from_u128(31),
            CableEnd::B,
        )
        .expect("other end");
        assert_ne!(old_edge, same_cable_other_end);

        for edge in [&old_edge, &new_edge] {
            let restored = NativeUnit::from_stored(
                edge.kind().as_str(),
                edge.table(),
                edge.id(),
                edge.member_column(),
                edge.member_token_column(),
            )
            .expect("restored edge");
            assert_eq!(&restored, edge);
        }
        // The holder row alone is not the edge: its lifecycle anchor is shared,
        // so the stored identity must keep the cable and the end.
        assert_eq!(
            old_edge.lifecycle_anchor().expect("anchor"),
            new_edge.lifecycle_anchor().expect("anchor")
        );
    }

    #[test]
    fn only_a_provider_and_source_qualified_mint_creates_a_claim() {
        let job = Uuid::from_u128(21);
        let mint = UnitMint {
            unit: record_unit(),
            identity: identity("dcim.devices", "42"),
            job_id: job,
            mapping: crate::contexts::sync::domain::MappingRef::new(Uuid::from_u128(3), 2),
            created_at: Utc::now(),
            applied: applied(job),
        };
        let mut ownership = UnitOwnership::mint(mint.clone()).expect("mint");
        assert_eq!(ownership.source_entity, "dcim.devices");
        assert_eq!(ownership.external_id, "42");
        assert!(ownership.gates_removal());

        let mut mismatched = mint;
        mismatched.applied.job_id = Uuid::from_u128(22);
        assert!(UnitOwnership::mint(mismatched).is_err());
    }

    #[test]
    fn a_relinquished_claim_is_never_refreshed_or_revived() {
        let job = Uuid::from_u128(21);
        let mint = UnitMint {
            unit: record_unit(),
            identity: identity("dcim.devices", "42"),
            job_id: job,
            mapping: crate::contexts::sync::domain::MappingRef::new(Uuid::from_u128(3), 2),
            created_at: Utc::now(),
            applied: applied(job),
        };
        let mut ownership = UnitOwnership::mint(mint).expect("mint");
        ownership
            .relinquish(RelinquishReason::LocallyModified, Utc::now())
            .expect("relinquish");
        assert!(!ownership.gates_removal());
        assert_eq!(
            ownership.relinquished_reason,
            Some(RelinquishReason::LocallyModified)
        );
        assert!(ownership.refresh(applied(job)).is_err());
        assert!(
            ownership
                .relinquish(RelinquishReason::OwnerConflict, Utc::now())
                .is_err()
        );
    }

    #[test]
    fn a_digest_frames_its_parts_and_rejects_a_malformed_value() {
        let first = UnitDigest::of_v1(&[b"ab", b"c"]);
        let second = UnitDigest::of_v1(&[b"a", b"bc"]);
        assert_ne!(first, second);
        assert_eq!(first, UnitDigest::of_v1(&[b"ab", b"c"]));
        assert_eq!(first.version(), UNIT_DIGEST_VERSION);
        assert!(UnitDigest::from_stored(UNIT_DIGEST_VERSION, first.as_str()).is_ok());
        assert!(UnitDigest::from_stored(2, first.as_str()).is_err());
        assert!(UnitDigest::from_stored(UNIT_DIGEST_VERSION, "deadbeef").is_err());
    }
}
