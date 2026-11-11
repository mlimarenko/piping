//! Absence policy, configured source generation and two-observation
//! confirmation for source-owned reconciliation (PIP-606).
//!
//! The authoritative universe is the inventory visible to the configured token
//! at the fixed trusted source. A single completed traversal therefore states an
//! absence, not a deletion: the operator contract requires two consecutive
//! successful traversals of the *same* source generation before an absence may
//! be considered confirmed. This module owns that vocabulary; `sync_inventory_absence`
//! owns the durable state, and its upsert is the only implementation of the
//! counting rule.
//!
//! A failed, partial or access-denied traversal never increments a count: only
//! the writer that completed every collection reports absences at all. Seeing the
//! unit again clears the row, and a generation change restarts the count from one.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::contexts::sync::domain::MappingRef;
use crate::contexts::sync::domain::inventory_export::digest;
use crate::contexts::sync::domain::ownership::NativeUnit;
use crate::contexts::sync::domain::{ProviderType, SourceEntity};
use crate::errors::AppError;

/// Revision of the credential returned by the secret owner with its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCredentialRevision {
    pub secret_id: Uuid,
    pub revision: String,
}

/// Provider state captured alongside the credential used by a source client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderGenerationBinding {
    pub provider_id: Uuid,
    pub base_url: String,
    pub sync_mode: Option<String>,
    pub credential: SourceCredentialRevision,
}

fn invalid(code: &'static str) -> AppError {
    AppError::Validation(format!("inventory_absence:{code}"))
}

/// How a completed traversal treats a source-side absence.
///
/// The mapping version owns the value; it is never a job option, and it never
/// enables a removal on its own. Under [`Self::DeleteSourceOwned`] every other
/// gate — creation ownership, native lifecycle, local drift, dependency budget
/// and Delete authority — still applies. Under [`Self::RetainAndReport`] absence
/// is reported and no absence-driven removal happens.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
    Default,
)]
#[serde(rename_all = "snake_case")]
pub enum AbsencePolicy {
    #[default]
    RetainAndReport,
    DeleteSourceOwned,
}

impl AbsencePolicy {
    /// The value a version receives when nothing destructive was asked for.
    pub const SAFE_DEFAULT: Self = Self::RetainAndReport;

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetainAndReport => "retain_and_report",
            Self::DeleteSourceOwned => "delete_source_owned",
        }
    }

    /// Normalise a stored or submitted value. An unknown value is an error, never
    /// the safe default: silently retaining after destructive policy was accepted
    /// would report an intent the engine did not honour.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw.trim() {
            "retain_and_report" => Ok(Self::RetainAndReport),
            "delete_source_owned" => Ok(Self::DeleteSourceOwned),
            _ => Err(invalid("policy")),
        }
    }

    /// Whether absence-driven removal is requested at all.
    #[must_use]
    pub fn removes_source_owned_units(self) -> bool {
        matches!(self, Self::DeleteSourceOwned)
    }
}

/// The exact configured-source generation a confirmation count belongs to.
///
/// It binds the provider identity, the canonical source origin, the immutable
/// mapping version and its content digest, the absence policy and the credential
/// configuration revision supplied by the owner of the secret bindings. A change
/// to any of them is a different generation, so a confirmation cannot survive a
/// re-pointed source or a re-issued credential. It never contains a credential,
/// an endpoint path with credentials, or a permission grant.
///
/// The credential revision is an explicit integration requirement, not something
/// this module derives: the provider row and its secret commands are written
/// separately, and the stored provider JSON can stay byte-identical while the API
/// token rotates. A revision read from provider JSON or a provider timestamp would
/// therefore miss the rotation, and an empty revision is refused rather than
/// treated as "unchanged". The existing secret owner has to supply the revision of
/// the credential configuration it actually resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGeneration(String);

/// The caller-supplied inputs of one generation.
///
/// Every part must be known: a missing origin, mapping digest or credential
/// revision refuses rather than collapses into a weaker identity.
#[derive(Debug, Clone)]
pub struct SourceGenerationInputs<'a> {
    pub provider_id: Uuid,
    pub provider_type: ProviderType,
    pub origin: &'a str,
    pub mapping: MappingRef,
    pub mapping_digest: &'a str,
    pub policy: AbsencePolicy,
    pub credential_revision: &'a str,
    pub credential_id: Uuid,
    pub sync_mode: &'a str,
    pub manifest_digest: &'a str,
    /// Optional-namespace capability the traversal ran under. `""` when the
    /// source has no optional namespace.
    pub source_capability: &'a str,
}

impl SourceGeneration {
    pub fn seal(inputs: &SourceGenerationInputs<'_>) -> Result<Self, AppError> {
        let provider_type = inputs.provider_type.as_str();
        let origin = canonical_origin(inputs.origin)?;
        if inputs.provider_id.is_nil() {
            return Err(invalid("generation_provider"));
        }
        if inputs.mapping.logical_id.is_nil() || inputs.mapping.version_id <= 0 {
            return Err(invalid("generation_mapping"));
        }
        if !is_digest(inputs.mapping_digest) {
            return Err(invalid("generation_mapping_digest"));
        }
        if inputs.credential_revision.trim().is_empty() {
            return Err(invalid("generation_credential_revision"));
        }
        if inputs.credential_id.is_nil()
            || inputs.sync_mode != "pull_only"
            || !is_digest(inputs.manifest_digest)
        {
            return Err(invalid("generation_source_scope"));
        }
        let mut parts: Vec<Vec<u8>> = vec![
            b"piping.source-generation/v2".to_vec(),
            inputs.provider_id.to_string().into_bytes(),
            provider_type.as_bytes().to_vec(),
            origin.as_bytes().to_vec(),
            inputs.mapping.to_wire().into_bytes(),
            inputs.mapping_digest.as_bytes().to_vec(),
            inputs.policy.as_str().as_bytes().to_vec(),
            inputs.credential_revision.trim().as_bytes().to_vec(),
            inputs.credential_id.to_string().into_bytes(),
            inputs.sync_mode.as_bytes().to_vec(),
            inputs.manifest_digest.as_bytes().to_vec(),
            inputs.source_capability.as_bytes().to_vec(),
        ];
        let mut framed = Vec::new();
        for part in parts {
            framed.extend_from_slice(&(part.len() as u64).to_be_bytes());
            framed.extend_from_slice(&part);
        }
        Ok(Self(digest(&framed)))
    }

    /// Restore a stored generation, refusing a value that is not one of ours.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        if !is_digest(raw) {
            return Err(invalid("generation"));
        }
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Canonical `scheme://host[:port][/path]` of the configured source.
///
/// Credentials, query strings and fragments are refused rather than stripped, so
/// a stored generation never carries a secret or an ambiguous endpoint.
fn canonical_origin(raw: &str) -> Result<String, AppError> {
    let origin = url::Url::parse(raw.trim()).map_err(|_| invalid("generation_origin"))?;
    if !matches!(origin.scheme(), "http" | "https")
        || !origin.has_host()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(invalid("generation_origin"));
    }
    Ok(origin.as_str().trim_end_matches('/').to_string())
}

/// One durable absence observation for a native unit.
///
/// A record is either pending (one completed traversal) or confirmed (two
/// consecutive ones under the same generation). Only a confirmed record may be
/// considered for removal, and only after every other gate holds.
#[derive(Debug, Clone)]
pub struct AbsenceRecord {
    pub provider_id: Uuid,
    pub unit: NativeUnit,
    pub generation: SourceGeneration,
    pub source_entity: SourceEntity,
    pub external_id: String,
    pub observation_count: i16,
    pub first_observed_at: DateTime<Utc>,
    pub first_observed_job_id: Uuid,
    pub last_observed_at: DateTime<Utc>,
    pub last_observed_job_id: Uuid,
    pub confirmed_at: Option<DateTime<Utc>>,
}

impl AbsenceRecord {
    /// The minimum number of consecutive completed observations.
    pub const CONFIRMING_OBSERVATIONS: i16 = 2;

    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.confirmed_at.is_some() && self.observation_count >= Self::CONFIRMING_OBSERVATIONS
    }

    /// Validate a row restored from durable state. Counts, times and confirmation
    /// must agree, so a corrupted or partially written row is never confirmed.
    pub fn validate(&self) -> Result<(), AppError> {
        if !(1..=Self::CONFIRMING_OBSERVATIONS).contains(&self.observation_count) {
            return Err(invalid("observation_count"));
        }
        if self.last_observed_at < self.first_observed_at {
            return Err(invalid("observation_order"));
        }
        if self.confirmed_at.is_some() != (self.observation_count == Self::CONFIRMING_OBSERVATIONS)
        {
            return Err(invalid("observation_confirmation"));
        }
        Ok(())
    }
}

/// Durable, secret-free outcome of one reconciliation pass.
///
/// Counts describe the durable state this job left behind: an absence that was
/// only observed is `pending`, a confirmed absence is `confirmed`, and `applied`
/// counts native removals that the same transaction committed. `retained` covers
/// units the pass deliberately did not act on, and `protected` covers a refusal
/// the pass reported instead of applying. Identities and reasons are closed
/// codes and digests; no provider body, field value or credential appears here.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconciliationReport {
    /// The policy this pass applied, as its wire value.
    pub policy: String,
    /// Active claims this provider held when the pass started.
    pub observed: u64,
    /// Absences recorded for the first time in this job.
    pub pending: u64,
    /// Absences confirmed for this job's configured source generation.
    pub confirmed: u64,
    /// Native units removed by this job, including units already absent.
    pub applied: u64,
    /// Units deliberately left in place.
    pub retained: u64,
    /// Refusals that prevented a proven removal from being applied.
    pub protected: u64,
    /// Pending absences cleared because the unit was observed again.
    pub cleared: u64,
    /// Closed reason codes, with how many units each accounted for.
    pub reason_codes: std::collections::BTreeMap<String, u64>,
    /// SHA-256 of the native identity of every applied unit.
    pub applied_identity_sha256: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::sync::domain::InventoryEntity;

    const MAPPING_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn generation_inputs() -> SourceGenerationInputs<'static> {
        SourceGenerationInputs {
            provider_id: Uuid::from_u128(7),
            provider_type: ProviderType::Netbox,
            origin: "https://netbox.example.test/",
            mapping: MappingRef::new(Uuid::from_u128(3), 2),
            mapping_digest: MAPPING_DIGEST,
            policy: AbsencePolicy::DeleteSourceOwned,
            credential_revision: "secret-binding-4",
            credential_id: Uuid::from_u128(11),
            sync_mode: "pull_only",
            manifest_digest: MAPPING_DIGEST,
            source_capability: "present",
        }
    }

    #[test]
    fn origins_use_url_semantics_without_collapsing_source_paths() {
        assert_eq!(
            canonical_origin("HTTPS://NETBOX.EXAMPLE.TEST:443/tenant-a/")
                .expect("valid source URL"),
            "https://netbox.example.test/tenant-a"
        );
        assert_ne!(
            canonical_origin("https://netbox.example.test/tenant-a").expect("first source URL"),
            canonical_origin("https://netbox.example.test/tenant-b").expect("second source URL")
        );
        for origin in [
            "https://netbox.example.test:invalid",
            "https://[invalid",
            "https://user@netbox.example.test",
            "https://netbox.example.test/?scope=limited",
            "https://netbox.example.test/#different",
        ] {
            assert!(canonical_origin(origin).is_err(), "{origin}");
        }
    }

    #[test]
    fn the_absence_policy_is_typed_and_refuses_an_unknown_value() {
        assert_eq!(
            AbsencePolicy::parse("retain_and_report").expect("policy"),
            AbsencePolicy::SAFE_DEFAULT
        );
        assert!(!AbsencePolicy::SAFE_DEFAULT.removes_source_owned_units());
        assert!(
            AbsencePolicy::parse("delete_source_owned")
                .expect("policy")
                .removes_source_owned_units()
        );
        for unknown in [
            "",
            "delete",
            "DELETE_SOURCE_OWNED",
            "retain",
            "delete_source_owned_x",
        ] {
            assert!(AbsencePolicy::parse(unknown).is_err(), "{unknown}");
        }
    }

    #[test]
    fn a_generation_changes_with_every_part_it_binds() {
        let base = SourceGeneration::seal(&generation_inputs()).expect("generation");
        let mut changed = generation_inputs();
        changed.origin = "https://netbox.example.test";
        assert_eq!(base, SourceGeneration::seal(&changed).expect("generation"));
        changed.origin = "https://other.example.test";
        assert_ne!(base, SourceGeneration::seal(&changed).expect("generation"));
        changed = generation_inputs();
        changed.credential_revision = "secret-binding-5";
        assert_ne!(base, SourceGeneration::seal(&changed).expect("generation"));
        changed = generation_inputs();
        changed.policy = AbsencePolicy::RetainAndReport;
        assert_ne!(base, SourceGeneration::seal(&changed).expect("generation"));
        changed = generation_inputs();
        changed.credential_id = Uuid::from_u128(12);
        assert_ne!(base, SourceGeneration::seal(&changed).expect("generation"));
        changed = generation_inputs();
        changed.manifest_digest =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        assert_ne!(base, SourceGeneration::seal(&changed).expect("generation"));
        changed = generation_inputs();
        changed.source_capability = "absent";
        assert_ne!(
            base,
            SourceGeneration::seal(&changed).expect("generation"),
            "an unavailable optional namespace is a different generation"
        );
        changed = generation_inputs();
        changed.sync_mode = "bidirectional";
        assert!(SourceGeneration::seal(&changed).is_err());
    }

    #[test]
    fn a_generation_refuses_a_credential_bearing_or_unknown_origin() {
        for origin in [
            "https://user:secret@netbox.example.test",
            "https://netbox.example.test/?token=1",
            "ftp://netbox.example.test",
            "netbox.example.test",
        ] {
            let mut inputs = generation_inputs();
            inputs.origin = origin;
            assert!(SourceGeneration::seal(&inputs).is_err(), "{origin}");
        }
        let mut inputs = generation_inputs();
        inputs.credential_revision = "   ";
        assert!(SourceGeneration::seal(&inputs).is_err());
        let mut inputs = generation_inputs();
        inputs.mapping_digest = "not-a-digest";
        assert!(SourceGeneration::seal(&inputs).is_err());
    }

    fn unit() -> NativeUnit {
        NativeUnit::record(InventoryEntity::Device, Uuid::from_u128(11)).expect("unit")
    }

    fn record(count: i16, confirmed_at: Option<DateTime<Utc>>) -> AbsenceRecord {
        AbsenceRecord {
            provider_id: Uuid::from_u128(7),
            unit: unit(),
            generation: SourceGeneration::seal(&generation_inputs()).expect("generation"),
            source_entity: SourceEntity::new("dcim.devices").expect("entity"),
            external_id: "42".to_string(),
            observation_count: count,
            first_observed_at: DateTime::from_timestamp(1_700_000_000, 0).expect("time"),
            first_observed_job_id: Uuid::from_u128(21),
            last_observed_at: DateTime::from_timestamp(1_700_000_100, 0).expect("time"),
            last_observed_job_id: Uuid::from_u128(22),
            confirmed_at,
        }
    }

    #[test]
    fn confirmation_needs_two_observations_and_consistent_state() {
        let pending = record(1, None);
        pending.validate().expect("pending");
        assert!(!pending.is_confirmed());

        let confirmed = record(
            2,
            Some(DateTime::from_timestamp(1_700_000_100, 0).expect("time")),
        );
        confirmed.validate().expect("confirmed");
        assert!(confirmed.is_confirmed());

        // A count without a confirmation time, or a confirmation without the
        // count, is not confirmed state.
        assert!(record(2, None).validate().is_err());
        assert!(
            record(
                1,
                Some(DateTime::from_timestamp(1_700_000_100, 0).expect("time"))
            )
            .validate()
            .is_err()
        );
        assert!(
            record(
                3,
                Some(DateTime::from_timestamp(1_700_000_100, 0).expect("time"))
            )
            .validate()
            .is_err()
        );
    }
}
