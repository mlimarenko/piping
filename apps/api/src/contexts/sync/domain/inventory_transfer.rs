//! PWA-81 immutable inventory transfer run contract.
//!
//! A transfer run is identified by the SHA256 of its **exact descriptor bytes**.
//! The descriptor pins every input that can change what is written: source
//! bundle artifact, mapping version, assessment profile, compiled catalog and
//! tool, target schema, executable, target snapshot and plan, operator, source
//! authority and the retained-absence policy. A changed field is a different
//! run; it is never re-planned under the same id.
//!
//! Deserialization is strict (`deny_unknown_fields`) and every value is
//! re-validated after parsing. There is no default, inferred or fallback field:
//! a missing key is a decode error, and an unsupported value (for example a
//! generic live source authority) is rejected rather than downgraded.
//!
//! Ownership:
//!
//! * this module — the descriptor/receipt wire contract and the deterministic
//!   plan/artifact digests shared by the application and the typed writer;
//! * `infrastructure::persistence::local_inventory_target` — the canonical
//!   writer and `sync_inventory_transfer_run` persistence;
//! * `application::inventory_transfer` — intake, target CAS and authorization;
//! * `bin::inventory_transfer` — command routing and operator-facing report.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_export::{SourceAttestationIdentity, digest, valid_digest};
use super::inventory_sync::{MappingRef, SourceEntity};
use crate::errors::AppError;

/// Bumped only with a wire-incompatible descriptor change.
pub const TRANSFER_FORMAT_VERSION: u32 = 1;

fn invalid(code: &'static str) -> AppError {
    AppError::Validation(format!("inventory_transfer:{code}"))
}

/// Operator-approved validity window of the run itself.
///
/// This is deliberately separate from the sealed source attestation: the
/// attestation says who owns the snapshot and which exact scope and expectations
/// the profile pinned; the approval says until when this run may still mutate a
/// target. Neither can be replaced by the other, and the approval carries no
/// source identity of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferApproval {
    pub not_before: DateTime<Utc>,
    /// Exclusive: an approval is not current at `expires_at`.
    pub expires_at: DateTime<Utc>,
}

impl TransferApproval {
    fn ensure_valid(&self) -> Result<(), AppError> {
        if self.not_before >= self.expires_at {
            return Err(invalid("descriptor_approval_window"));
        }
        Ok(())
    }

    /// The approval must be current at the boundary that relies on it.
    pub fn ensure_current(&self, now: DateTime<Utc>) -> Result<(), AppError> {
        if now < self.not_before || now >= self.expires_at {
            return Err(invalid("approval_not_current"));
        }
        Ok(())
    }
}

/// How a source-side absence is treated. The only supported policy retains and
/// reports absence: deletion is never inferred from a missing source record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainAbsencePolicy {
    RetainAndReport,
}

/// The authenticated operator of the run.
///
/// This is an access-control user, not a business owner: owner/owner-group
/// values are inventory data and are never resolved as principals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferOperator {
    pub user_id: Uuid,
}

/// What the run is allowed to write, which fixes the required permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferScope {
    pub creates: bool,
    pub updates: bool,
    pub owner_assignments: bool,
    pub custom_fields: bool,
    pub object_references: bool,
    pub secret_intents: bool,
}

impl TransferScope {
    /// A transfer that writes nothing is not a transfer.
    #[must_use]
    pub fn writes_inventory(&self) -> bool {
        self.creates || self.updates
    }
}

/// Target identity observed under the same read-only assessment as the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferTarget {
    /// Digest of the destination database identity.
    pub identity_sha256: String,
    /// Digest of the full catalog/graph/binding fingerprint.
    pub fingerprint_sha256: String,
    /// Digest of the typed plan derived from that fingerprint.
    pub plan_digest: String,
}

/// Strict immutable run descriptor. Fields are never optional and never
/// defaulted; the digest of the exact encoded bytes is the run identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferDescriptor {
    pub format_version: u32,
    pub run_id: Uuid,
    pub provider_id: Uuid,
    /// External seal of the exact `manifest.json` bytes.
    pub bundle_digest: String,
    pub mapping_ref: MappingRef,
    pub mapping_digest: String,
    pub profile_digest: String,
    pub catalog_digest: String,
    pub tool_digest: String,
    /// Applied target schema (migration set) identity.
    pub schema_digest: String,
    /// Merkle identity of the sealed artifact: manifest bytes plus every
    /// verified payload, derived from the artifact itself.
    pub artifact_sha256: String,
    /// Digest of the running `inventory-transfer` executable.
    pub executable_sha256: String,
    pub target: TransferTarget,
    pub operator: TransferOperator,
    /// Canonical identity of the sealed snapshot attestation, copied from the
    /// verified source. It carries the issuer, issue time, scope digest and
    /// expectations digest the profile pinned independently, so a descriptor
    /// cannot establish authority by naming its own identifiers.
    pub authority: SourceAttestationIdentity,
    /// Operator-approved run window, separate from the attestation.
    pub approval: TransferApproval,
    pub retain_absence: RetainAbsencePolicy,
    pub scope: TransferScope,
}

impl TransferDescriptor {
    /// Structural validation only. Runtime evidence (running executable, live
    /// artifact, target observation) is verified separately at the boundary.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.format_version != TRANSFER_FORMAT_VERSION {
            return Err(invalid("descriptor_version"));
        }
        if self.run_id.is_nil() || self.provider_id.is_nil() {
            return Err(invalid("descriptor_identity"));
        }
        if self.operator.user_id.is_nil() {
            return Err(invalid("descriptor_operator"));
        }
        if self.mapping_ref.logical_id.is_nil() || self.mapping_ref.version_id <= 0 {
            return Err(invalid("descriptor_mapping"));
        }
        for pinned in [
            &self.bundle_digest,
            &self.mapping_digest,
            &self.profile_digest,
            &self.catalog_digest,
            &self.tool_digest,
            &self.schema_digest,
            &self.artifact_sha256,
            &self.executable_sha256,
            &self.target.identity_sha256,
            &self.target.fingerprint_sha256,
            &self.target.plan_digest,
            &self.authority.attestation_sha256,
            &self.authority.scope_digest,
            &self.authority.expectations_sha256,
            &self.authority.provenance_sha256,
        ] {
            if !valid_digest(pinned) {
                return Err(invalid("descriptor_digest"));
            }
        }
        if self.authority.authority_id.is_nil()
            || self.authority.operator_id != self.operator.user_id
            || self.authority.issuer.is_empty()
            || self.authority.issuer.len() > 128
        {
            return Err(invalid("descriptor_authority"));
        }
        self.approval.ensure_valid()?;
        if !self.scope.writes_inventory() {
            return Err(invalid("descriptor_scope"));
        }
        Ok(())
    }

    /// The run approval must be current when the run reaches the commit
    /// boundary, not only when the descriptor was prepared. The source
    /// attestation itself has no expiry: it identifies immutable evidence.
    pub fn ensure_authority_current(&self, now: DateTime<Utc>) -> Result<(), AppError> {
        self.approval.ensure_current(now)
    }

    /// Bind the descriptor to its exact encoded bytes.
    pub fn pin(self) -> Result<PinnedTransferDescriptor, AppError> {
        self.validate()?;
        let bytes = serde_json::to_vec(&self).map_err(|_| invalid("descriptor_encoding"))?;
        let descriptor_digest = digest(&bytes);
        Ok(PinnedTransferDescriptor {
            bytes,
            descriptor_digest,
            descriptor: self,
        })
    }
}

/// A descriptor whose exact bytes were independently pinned by SHA256.
///
/// The writer persists `bytes` for reference and `descriptor_digest` for
/// identity. Re-encoding the parsed value is never used as the identity: JSON
/// round-tripping normalizes whitespace and key order, which would erase the
/// byte identity the operator pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedTransferDescriptor {
    bytes: Vec<u8>,
    descriptor_digest: String,
    descriptor: TransferDescriptor,
}

impl PinnedTransferDescriptor {
    /// Verify the external digest first, then decode strictly and validate.
    pub fn parse(bytes: &[u8], expected_digest: &str) -> Result<Self, AppError> {
        if !valid_digest(expected_digest) {
            return Err(invalid("descriptor_digest_pin"));
        }
        let descriptor_digest = digest(bytes);
        if descriptor_digest != expected_digest {
            return Err(invalid("descriptor_digest_mismatch"));
        }
        let descriptor: TransferDescriptor =
            serde_json::from_slice(bytes).map_err(|_| invalid("descriptor_encoding"))?;
        descriptor.validate()?;
        Ok(Self {
            bytes: bytes.to_vec(),
            descriptor_digest,
            descriptor,
        })
    }

    /// Exact pinned bytes, never a reserialization.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// SHA256 of [`Self::bytes`].
    #[must_use]
    pub fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }

    #[must_use]
    pub fn descriptor(&self) -> &TransferDescriptor {
        &self.descriptor
    }
}

/// Terminal result of a committed run. Only a committed run has a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOutcome {
    Committed,
}

/// One source record that is absent from the sealed source while its bound
/// target row is deliberately retained.
///
/// Only the source-side identity digest is stored; the raw external id never
/// enters a descriptor, receipt, log or report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedAbsence {
    /// Canonical source entity, e.g. `dcim.sites`.
    pub source_entity: String,
    /// SHA256 of the source-side identity text.
    pub external_id_sha256: String,
    /// Bound target row that remains in place.
    pub local_id: Uuid,
}

impl RetainedAbsence {
    /// Hash the raw identity at the boundary so it cannot leak.
    pub fn new(source_entity: &str, external_id: &str, local_id: Uuid) -> Result<Self, AppError> {
        let entity = SourceEntity::new(source_entity).map_err(|_| invalid("retained_absence"))?;
        if external_id.is_empty() || local_id.is_nil() {
            return Err(invalid("retained_absence"));
        }
        Ok(Self {
            source_entity: entity.as_str().to_owned(),
            external_id_sha256: digest(external_id.as_bytes()),
            local_id,
        })
    }

    fn ensure_valid(&self) -> Result<(), AppError> {
        let canonical =
            SourceEntity::new(&self.source_entity).map_err(|_| invalid("retained_absence"))?;
        if canonical.as_str() != self.source_entity.as_str()
            || !valid_digest(&self.external_id_sha256)
            || self.local_id.is_nil()
        {
            return Err(invalid("retained_absence"));
        }
        Ok(())
    }
}

/// Counters of the durable commit. They are not a substitute for inspecting the
/// written rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferReceiptStats {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub tag_memberships: u64,
    pub secret_intents: u64,
}

/// Durable committed receipt, written in the same transaction as the inventory
/// and the terminal job state. A same-run replay returns this value and never
/// rewrites later edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferReceipt {
    pub format_version: u32,
    pub run_id: Uuid,
    pub descriptor_digest: String,
    pub outcome: TransferOutcome,
    /// Terminal sync job written with the inventory. Always present on this
    /// import path; a receipt without a job is not a committed transfer.
    pub sync_job_id: Uuid,
    pub committed_at: DateTime<Utc>,
    /// Fingerprint observed at the commit boundary, not the prepared one.
    pub target_fingerprint_sha256: String,
    pub stats: TransferReceiptStats,
    /// Source records absent from the sealed source whose bound rows were
    /// retained. Absence is reported, never pruned.
    pub retained_absent: Vec<RetainedAbsence>,
}

impl TransferReceipt {
    /// Reject a receipt that does not describe exactly this run and descriptor.
    pub fn verify(&self, run_id: Uuid, descriptor_digest: &str) -> Result<(), AppError> {
        if self.format_version != TRANSFER_FORMAT_VERSION
            || self.run_id != run_id
            || self.descriptor_digest != descriptor_digest
            || !valid_digest(&self.descriptor_digest)
            || !valid_digest(&self.target_fingerprint_sha256)
        {
            return Err(invalid("receipt_identity"));
        }
        if self.outcome != TransferOutcome::Committed || self.sync_job_id.is_nil() {
            return Err(invalid("receipt_outcome"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for absence in &self.retained_absent {
            absence.ensure_valid()?;
            if !seen.insert((
                absence.source_entity.as_str(),
                absence.external_id_sha256.as_str(),
                absence.local_id,
            )) {
                return Err(invalid("retained_absence"));
            }
        }
        Ok(())
    }
}

/// Closed, sanitized routing states of an apply/resume attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferRunState {
    /// Inventory, bindings, job state and receipt committed now.
    Committed,
    /// The durable receipt of this exact run and descriptor was returned.
    Replayed,
    /// The verified inputs or target plan contain blockers; nothing was written.
    Blocked,
    /// The target no longer matches the pinned fingerprint or plan; nothing was
    /// written and nothing was re-planned.
    Stale,
    /// Another writer holds the run or an observation lock; nothing was written.
    Busy,
    /// The operator token or its permissions do not allow the run.
    AuthorizationLost,
    /// Another attempt still owns the run fence.
    InProgress,
    /// The commit outcome cannot be observed; it is resolved by the same run.
    Uncertain,
    /// The transaction failed before commit and nothing was written.
    RolledBack,
}

/// Merkle identity of a sealed export artifact: the exact manifest bytes plus
/// the digest of every verified payload, in path order.
///
/// Returns `None` for a duplicate payload path. A sealed manifest cannot contain
/// one (it is rejected at open), so `None` only means the caller did not read a
/// validated artifact and must block rather than invent an identity.
#[must_use]
pub fn artifact_digest(manifest_bytes: &[u8], payloads: &[(&str, &[u8])]) -> Option<String> {
    let mut ordered = payloads.to_vec();
    ordered.sort_unstable_by(|left, right| left.0.cmp(right.0));
    if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return None;
    }
    let mut canonical = String::from("inventory_transfer:artifact:1\n");
    canonical.push_str(&digest(manifest_bytes));
    canonical.push('\n');
    for (path, bytes) in ordered {
        canonical.push_str(path);
        canonical.push('\u{1f}');
        canonical.push_str(&digest(bytes));
        canonical.push('\n');
    }
    Some(digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authority() -> SourceAttestationIdentity {
        SourceAttestationIdentity {
            operator_id: Uuid::from_u128(4),
            provenance_sha256: "8".repeat(64),
            authority_id: Uuid::from_u128(7),
            attestation_sha256: "b".repeat(64),
            issuer: "pwa-81/synthetic-fixture".into(),
            issued_at: Utc::now() - chrono::TimeDelta::hours(2),
            scope_digest: "6".repeat(64),
            expectations_sha256: "7".repeat(64),
            edge_coverage:
                crate::contexts::sync::domain::inventory_export::EdgeCoverage::TypedInventoryReferences,
        }
    }

    fn approval() -> TransferApproval {
        TransferApproval {
            not_before: Utc::now() - chrono::TimeDelta::hours(1),
            expires_at: Utc::now() + chrono::TimeDelta::hours(1),
        }
    }

    fn descriptor() -> TransferDescriptor {
        TransferDescriptor {
            format_version: TRANSFER_FORMAT_VERSION,
            run_id: Uuid::from_u128(1),
            provider_id: Uuid::from_u128(2),
            bundle_digest: "a".repeat(64),
            mapping_ref: MappingRef::new(Uuid::from_u128(3), 1),
            mapping_digest: "c".repeat(64),
            profile_digest: "d".repeat(64),
            catalog_digest: "e".repeat(64),
            tool_digest: "f".repeat(64),
            schema_digest: "0".repeat(64),
            artifact_sha256: "1".repeat(64),
            executable_sha256: "2".repeat(64),
            target: TransferTarget {
                identity_sha256: "3".repeat(64),
                fingerprint_sha256: "4".repeat(64),
                plan_digest: "5".repeat(64),
            },
            operator: TransferOperator {
                user_id: Uuid::from_u128(4),
            },
            authority: authority(),
            approval: approval(),
            retain_absence: RetainAbsencePolicy::RetainAndReport,
            scope: TransferScope {
                creates: true,
                updates: true,
                owner_assignments: false,
                custom_fields: false,
                object_references: false,
                secret_intents: false,
            },
        }
    }

    #[test]
    fn pinned_descriptor_rejects_changed_bytes_and_keeps_byte_identity() {
        let pinned = descriptor().pin().expect("pin descriptor");
        assert_eq!(pinned.descriptor_digest(), digest(pinned.bytes()));
        let mut changed = pinned.bytes().to_vec();
        changed.push(b' ');
        assert!(
            PinnedTransferDescriptor::parse(&changed, pinned.descriptor_digest()).is_err(),
            "added whitespace must invalidate the pinned identity"
        );
    }

    #[test]
    fn descriptor_rejects_unmatched_operator_and_invalid_provenance() {
        let mut value = descriptor();
        value.authority.operator_id = Uuid::from_u128(9);
        assert!(value.validate().is_err());
        value.authority.operator_id = value.operator.user_id;
        value.authority.provenance_sha256 = "not-a-digest".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn descriptor_rejects_unknown_fields_and_missing_authority_window() {
        let pinned = descriptor().pin().expect("pin descriptor");
        let value: serde_json::Value =
            serde_json::from_slice(pinned.bytes()).expect("descriptor json");
        let mut object = value.as_object().cloned().expect("object");
        object.insert("guessed_default".into(), serde_json::json!(true));
        let encoded = serde_json::to_vec(&object).expect("encode");
        assert!(
            PinnedTransferDescriptor::parse(&encoded, &digest(&encoded)).is_err(),
            "unknown fields must not be ignored"
        );

        let mut expired = descriptor();
        expired.approval.expires_at = expired.approval.not_before;
        assert!(expired.validate().is_err());
        assert!(expired.pin().is_err());
    }

    #[test]
    fn descriptor_rejects_a_nested_extra_key_in_the_mapping_reference() {
        let pinned = descriptor().pin().expect("pin descriptor");
        let mut value: serde_json::Value =
            serde_json::from_slice(pinned.bytes()).expect("descriptor json");
        value["mapping_ref"]["guessed_version"] = serde_json::json!(2);
        let encoded = serde_json::to_vec(&value).expect("encode");
        assert!(
            PinnedTransferDescriptor::parse(&encoded, &digest(&encoded)).is_err(),
            "nested strictness must not be weaker than the top-level contract"
        );
    }

    #[test]
    fn expired_approval_is_rejected_at_the_boundary() {
        let pinned = descriptor().pin().expect("pin descriptor");
        let now = pinned.descriptor().approval.expires_at;
        assert!(
            pinned.descriptor().ensure_authority_current(now).is_err(),
            "approval expiry is exclusive"
        );
    }

    #[test]
    fn descriptor_does_not_ignore_unknown_mapping_identity_fields() {
        let mut value = serde_json::to_value(descriptor()).expect("descriptor json");
        value["mapping_ref"]["guessed_provider"] = serde_json::json!("unapproved");
        let bytes = serde_json::to_vec(&value).expect("descriptor bytes");
        assert!(PinnedTransferDescriptor::parse(&bytes, &digest(&bytes)).is_err());
    }

    #[test]
    fn receipt_must_describe_the_same_run_and_descriptor() {
        let receipt = TransferReceipt {
            format_version: TRANSFER_FORMAT_VERSION,
            run_id: Uuid::from_u128(1),
            descriptor_digest: "a".repeat(64),
            outcome: TransferOutcome::Committed,
            sync_job_id: Uuid::from_u128(9),
            committed_at: Utc::now(),
            target_fingerprint_sha256: "b".repeat(64),
            stats: TransferReceiptStats::default(),
            retained_absent: vec![
                RetainedAbsence::new("dcim.sites", "42", Uuid::from_u128(5)).expect("absence"),
            ],
        };
        assert!(receipt.verify(Uuid::from_u128(1), &"a".repeat(64)).is_ok());
        assert!(receipt.verify(Uuid::from_u128(2), &"a".repeat(64)).is_err());
        assert!(receipt.verify(Uuid::from_u128(1), &"c".repeat(64)).is_err());
        assert!(
            receipt
                .retained_absent
                .first()
                .is_some_and(|absence| absence.external_id_sha256 != "42"),
            "the raw source identity must never be stored"
        );
    }

    #[test]
    fn artifact_digest_blocks_duplicate_paths_and_ignores_input_order() {
        let payloads = [
            ("a.json", [1_u8, 2].as_slice()),
            ("b.json", [3_u8].as_slice()),
        ];
        let mut reversed = payloads;
        reversed.reverse();
        assert_eq!(
            artifact_digest(b"manifest", &payloads),
            artifact_digest(b"manifest", &reversed)
        );
        let duplicated = [("a.json", [1_u8].as_slice()), ("a.json", [1_u8].as_slice())];
        assert!(artifact_digest(b"manifest", &duplicated).is_none());
        assert_ne!(
            artifact_digest(b"manifest", &payloads),
            artifact_digest(b"other", &payloads)
        );
    }
}
