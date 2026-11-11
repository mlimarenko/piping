//! Source-owned absence reconciliation for one completed local pull (PIP-606).
//!
//! One pull is one observation. For every active claim this provider minted, the
//! complete included-source selection is the evidence:
//!
//! * the unit is still selected by the source record → a pending absence is
//!   cleared;
//! * the unit's source record is absent, or the record is present and no longer
//!   selects the relation member → the durable two-observation counter in
//!   `sync_inventory_absence` is advanced;
//! * the field, the entity or the record scope was not observed at all → nothing
//!   is recorded, because an unobserved unit is not an absent unit.
//!
//! Only a confirmed absence of the *same* configured source generation may be
//! considered for removal, and only under [`AbsencePolicy::DeleteSourceOwned`].
//! Removal is delegated to the canonical inventory owner: this module translates
//! the proved native units into [`ApprovedNativeUnit`]s and typed relation
//! removals, asks [`plan_native_removal`] what deleting them would do, requires
//! the caller's fresh operation-level Delete and per-object Delete/holder Update
//! authority, and executes inside the pull's own transaction. Every gate is
//! checked before the first native mutation; a confirmed unit that cannot be
//! removed safely refuses the whole pull instead of leaving a partially applied
//! set.
//!
//! The engine never takes a source binding as ownership, never issues an
//! unchecked raw `DELETE`, and never reports a removal that the transaction did
//! not commit.

use std::collections::{BTreeSet, HashMap};

use chrono::Utc;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use super::interface_graph;
use super::ownership;
use super::{ColumnValue, InventoryEntity, PlannedRow};
use crate::contexts::access_control::domain::Action;
use crate::contexts::inventory::domain::dcim::interface::InterfaceCableEnd;
use crate::contexts::inventory::infrastructure::persistence::repositories::native_removal::{
    ApprovedNativeUnit, CableEndToken, DerivedStateRepair, NativeBlocker, NativeRelationKind,
    NativeRelationRemoval, execute_native_removal, plan_derived_state_repair, plan_native_removal,
};
use crate::contexts::sync::domain::inventory_export::digest;
use crate::contexts::sync::domain::{
    AbsencePolicy, AbsenceRecord, InventoryCommitAuthorisation, InventoryCommitOperation,
    NativeUnit, NativeUnitKind, ReconciliationReport, SourceEntity, SourceGeneration, UnitDigest,
    UnitOwnership,
};
use crate::contexts::sync::infrastructure::persistence::repositories::{
    OwnershipRepository, SqlxOwnershipRepository,
};
use crate::errors::AppError;

/// How many confirmed absences one job may act on. A larger backlog is refused
/// rather than silently truncated: every confirmed unit must be removed or the
/// complete pull must refuse.
const CONFIRMED_BATCH: i64 = 10_000;

/// The immutable context of one reconciliation pass.
pub(super) struct Pass<'a> {
    pub job_id: Uuid,
    pub generation: &'a SourceGeneration,
    pub policy: AbsencePolicy,
    pub provider_id: Uuid,
    /// Every source entity the completed manifest observed.
    pub completed_entities: &'a [SourceEntity],
    /// The source's own selection, before locally retained members are merged in.
    pub source_selected: &'a [PlannedRow],
    pub authorisation: &'a dyn InventoryCommitAuthorisation,
}

/// A refused pass: nothing this pass changed may become durable.
pub(super) struct Refusal {
    /// The counts and closed reason codes observed up to the refusal.
    pub report: ReconciliationReport,
    pub error: AppError,
}

impl Refusal {
    fn internal(report: &ReconciliationReport, error: AppError) -> Self {
        Self {
            report: report.clone(),
            error,
        }
    }

    /// A proven unit or a proven set cannot be removed safely.
    fn gate(report: &ReconciliationReport, code: &'static str) -> Self {
        let mut report = report.clone();
        *report.reason_codes.entry(code.to_string()).or_insert(0) += 1;
        Self {
            report,
            error: AppError::Conflict(format!("inventory_reconciliation:{code}")),
        }
    }

    fn denied(report: &ReconciliationReport, code: &'static str, error: AppError) -> Self {
        let mut report = report.clone();
        *report.reason_codes.entry(code.to_string()).or_insert(0) += 1;
        Self { report, error }
    }
}

/// What the completed selection says about one claimed unit.
enum Presence {
    /// The source still selects the unit.
    Present,
    /// The source record was observed and does not select the unit.
    Absent,
    /// The field, the entity or the record shape was not observed.
    Unobserved,
}

/// Whether the claimed native unit still exists and still matches its proof.
enum Verified {
    /// Gone natively; there is nothing left to remove.
    AlreadyAbsent,
    /// Present and unchanged since the writer applied it.
    Intact,
    /// Present, but the schema does not cover it: nothing can be proved.
    Uncovered,
    /// Present, but not the state the writer applied.
    Modified,
}

/// Advance the durable counters and, for a destructive policy, execute every
/// confirmed removal inside the caller's transaction.
pub(super) async fn reconcile(
    transaction: &mut Transaction<'_, Postgres>,
    pass: &Pass<'_>,
) -> Result<ReconciliationReport, Refusal> {
    let repository = SqlxOwnershipRepository;
    let mut report = ReconciliationReport {
        policy: pass.policy.as_str().to_string(),
        ..ReconciliationReport::default()
    };

    let claims = repository
        .lock_active_for_provider(transaction, pass.provider_id)
        .await
        .map_err(|error| Refusal::internal(&report, error))?;

    let selected: HashMap<(&str, &str), &PlannedRow> = pass
        .source_selected
        .iter()
        .map(|row| ((row.source_entity.as_str(), row.external_id.as_str()), row))
        .collect();
    let completed: BTreeSet<&str> = pass
        .completed_entities
        .iter()
        .map(SourceEntity::as_str)
        .collect();

    // Only a unit this pass *observed as absent* may be removed. A durable
    // confirmation from an earlier job is not evidence about this observation: an
    // unobserved field, entity or record says nothing about the native unit, so a
    // pass that cannot see it must never act on the old count.
    let mut absent_this_pass: BTreeSet<UnitKey> = BTreeSet::new();

    for claim in &claims {
        report.observed += 1;
        let presence = classify(transaction, claim, &selected, &completed)
            .await
            .map_err(|error| Refusal::internal(&report, error))?;
        match presence {
            Presence::Present => {
                let cleared = repository
                    .clear_absent(transaction, pass.provider_id, &claim.unit)
                    .await
                    .map_err(|error| Refusal::internal(&report, error))?;
                if cleared {
                    report.cleared += 1;
                }
                increment(&mut report, "present");
            }
            Presence::Unobserved => {
                report.retained += 1;
                increment(&mut report, "unobserved");
            }
            Presence::Absent => {
                absent_this_pass.insert(unit_key(&claim.unit));
                let now = Utc::now();
                let record = AbsenceRecord {
                    provider_id: pass.provider_id,
                    unit: claim.unit.clone(),
                    generation: pass.generation.clone(),
                    source_entity: SourceEntity::new(&claim.source_entity)
                        .map_err(|error| Refusal::internal(&report, error))?,
                    external_id: claim.external_id.clone(),
                    observation_count: 1,
                    first_observed_at: now,
                    first_observed_job_id: pass.job_id,
                    last_observed_at: now,
                    last_observed_job_id: pass.job_id,
                    confirmed_at: None,
                };
                let stored = repository
                    .observe_absent(transaction, &record)
                    .await
                    .map_err(|error| Refusal::internal(&report, error))?;
                if stored.is_confirmed() {
                    report.confirmed += 1;
                    increment(&mut report, "absent_confirmed");
                } else {
                    report.pending += 1;
                    increment(&mut report, "absent_pending");
                }
            }
        }
    }

    if !pass.policy.removes_source_owned_units() {
        report.retained += report.confirmed;
        if report.confirmed > 0 {
            increment(&mut report, "retained_by_policy");
        }
        return Ok(report);
    }

    // The destructive operation is checked as an operation, independently of how
    // many candidates exist.
    ensure_delete_operation(pass, &report).await?;

    let confirmed = repository
        .confirmed_absences(
            transaction,
            pass.provider_id,
            pass.generation,
            CONFIRMED_BATCH.saturating_add(1),
        )
        .await
        .map_err(|error| Refusal::internal(&report, error))?;
    // One row beyond the batch proves the backlog is larger than this job may act
    // on; exactly a full batch is processed.
    if i64::try_from(confirmed.len()).unwrap_or(i64::MAX) > CONFIRMED_BATCH {
        report.protected += 1;
        return Err(Refusal::gate(&report, "confirmed_batch_too_large"));
    }
    let by_unit: HashMap<UnitKey, &UnitOwnership> = claims
        .iter()
        .map(|claim| (unit_key(&claim.unit), claim))
        .collect();
    let mut candidates = Vec::new();
    for record in &confirmed {
        let key = unit_key(&record.unit);
        let Some(claim) = by_unit.get(&key) else {
            // The claim was relinquished or retired since the observation: the
            // absence no longer describes a unit this provider owns.
            repository
                .clear_absent(transaction, pass.provider_id, &record.unit)
                .await
                .map_err(|error| Refusal::internal(&report, error))?;
            report.retained += 1;
            increment(&mut report, "claim_gone");
            continue;
        };
        if !absent_this_pass.contains(&key) {
            // Confirmed by an earlier job, but this completed selection did not
            // observe the unit as absent: keep the proof, remove nothing.
            report.retained += 1;
            increment(&mut report, "not_absent_this_pass");
            continue;
        }
        candidates.push(Candidate { claim, record });
    }

    if candidates.is_empty() {
        return Ok(report);
    }

    execute_removals(transaction, pass, &mut report, &candidates).await?;
    Ok(report)
}

/// One confirmed absence this pass may act on.
struct Candidate<'a> {
    claim: &'a UnitOwnership,
    record: &'a AbsenceRecord,
}

async fn ensure_delete_operation(
    pass: &Pass<'_>,
    report: &ReconciliationReport,
) -> Result<(), Refusal> {
    pass.authorisation
        .ensure_operation_access(InventoryCommitOperation::Delete)
        .await
        .map_err(|error| Refusal::denied(report, "unauthorized", error))
}

/// Classify one claim against the completed source selection.
async fn classify(
    _tx: &mut Transaction<'_, Postgres>,
    claim: &UnitOwnership,
    selected: &HashMap<(&str, &str), &PlannedRow>,
    completed: &BTreeSet<&str>,
) -> Result<Presence, AppError> {
    if !completed.contains(claim.source_entity.as_str()) {
        return Ok(Presence::Unobserved);
    }
    let Some(row) = selected.get(&(claim.source_entity.as_str(), claim.external_id.as_str()))
    else {
        return Ok(Presence::Absent);
    };
    let member = claim
        .unit
        .member()
        .ok_or_else(|| AppError::Internal("inventory_reconciliation:member".into()));
    match claim.unit.kind() {
        NativeUnitKind::Record => {
            if row.local_id != claim.unit.id() {
                // The binding that produced this claim no longer names the same
                // local row: the unit cannot be proved from this observation.
                return Ok(Presence::Unobserved);
            }
            Ok(Presence::Present)
        }
        NativeUnitKind::TagMembership => {
            let Some(tags) = row.columns.iter().find_map(|column| match &column.value {
                ColumnValue::Tags(tags) if column.updatable => Some(tags),
                _ => None,
            }) else {
                return Ok(Presence::Unobserved);
            };
            Ok(if tags.contains(&member?.to_string()) {
                Presence::Present
            } else {
                Presence::Absent
            })
        }
        NativeUnitKind::JunctionMember => {
            let Some(junction) = row
                .junctions
                .iter()
                .find(|junction| junction.table == claim.unit.table())
            else {
                return Ok(Presence::Unobserved);
            };
            Ok(if junction.refs.contains(&member?) {
                Presence::Present
            } else {
                Presence::Absent
            })
        }
        NativeUnitKind::CableAttachment => {
            let Some(attachment) = interface_graph::attachment(row) else {
                return Ok(Presence::Unobserved);
            };
            Ok(match (attachment.0, attachment.1) {
                (Some(cable), Some(end))
                    if cable == member?
                        && cable_end_token(end.clone()) == claim.unit.member_token_column() =>
                {
                    Presence::Present
                }
                _ => Presence::Absent,
            })
        }
    }
}

/// The native `dcim_interface.cable_end` token of one source-side attachment.
///
/// The match is exhaustive over [`InterfaceCableEnd`], so a new end variant
/// cannot silently compare equal to a stored token that no longer names it.
fn cable_end_token(end: InterfaceCableEnd) -> &'static str {
    match end {
        InterfaceCableEnd::A => "A",
        InterfaceCableEnd::B => "B",
    }
}

/// Translate confirmed units, check every gate and then mutate the native graph.
async fn execute_removals(
    transaction: &mut Transaction<'_, Postgres>,
    pass: &Pass<'_>,
    report: &mut ReconciliationReport,
    candidates: &[Candidate<'_>],
) -> Result<(), Refusal> {
    let repository = SqlxOwnershipRepository;
    let mut approved: Vec<ApprovedNativeUnit> = Vec::new();
    let mut relations: Vec<NativeRelationRemoval> = Vec::new();
    let mut applied_already: Vec<NativeUnit> = Vec::new();
    let mut subordinate: Vec<NativeUnit> = Vec::new();
    let mut deleted_records: BTreeSet<(&'static str, Uuid)> = BTreeSet::new();

    // Records first: a relation whose holder is removed by this same set needs no
    // separate holder update.
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.claim.unit.kind() == NativeUnitKind::Record)
    {
        let entity = entity_of_table(candidate.claim.unit.table())
            .ok_or_else(|| Refusal::gate(report, "unsupported_unit"))?;
        match verify(transaction, &candidate.claim.unit, entity, candidate.claim)
            .await
            .map_err(|error| Refusal::internal(report, error))?
        {
            Verified::AlreadyAbsent => {
                applied_already.push(candidate.claim.unit.clone());
                increment(report, "already_absent");
            }
            Verified::Intact => {
                approved.push(ApprovedNativeUnit {
                    table: candidate.claim.unit.table(),
                    id: candidate.claim.unit.id(),
                });
                deleted_records.insert((candidate.claim.unit.table(), candidate.claim.unit.id()));
            }
            Verified::Uncovered => {
                report.protected += 1;
                return Err(Refusal::gate(report, "unproven_native_state"));
            }
            Verified::Modified => {
                report.retained += 1;
                return Err(Refusal::gate(report, "modified_since_applied"));
            }
        }
    }

    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.claim.unit.kind() != NativeUnitKind::Record)
    {
        let unit = &candidate.claim.unit;
        let holder = unit.table();
        let entity =
            entity_of_table(holder).ok_or_else(|| Refusal::gate(report, "unsupported_unit"))?;
        match verify(transaction, unit, entity, candidate.claim)
            .await
            .map_err(|error| Refusal::internal(report, error))?
        {
            Verified::AlreadyAbsent => {
                applied_already.push(unit.clone());
                increment(report, "already_absent");
                continue;
            }
            Verified::Intact => {}
            Verified::Uncovered => {
                report.protected += 1;
                return Err(Refusal::gate(report, "unproven_native_state"));
            }
            Verified::Modified => {
                report.retained += 1;
                return Err(Refusal::gate(report, "modified_since_applied"));
            }
        }
        if deleted_records.contains(&(holder, unit.id())) {
            subordinate.push(unit.clone());
            continue;
        }
        let member = unit
            .member()
            .ok_or_else(|| Refusal::gate(report, "unsupported_unit"))?;
        let kind = match unit.kind() {
            NativeUnitKind::TagMembership => NativeRelationKind::Tag { tag_id: member },
            NativeUnitKind::JunctionMember => NativeRelationKind::Junction {
                table: unit.table(),
                member_id: member,
            },
            NativeUnitKind::CableAttachment => NativeRelationKind::Cable {
                cable_id: member,
                end: CableEndToken::parse(unit.member_token_column())
                    .map_err(|_| Refusal::gate(report, "unsupported_unit"))?,
            },
            NativeUnitKind::Record => unreachable!("records were handled first"),
        };
        relations.push(NativeRelationRemoval {
            holder_table: holder,
            holder_id: unit.id(),
            kind,
        });
    }

    let plan = plan_native_removal(transaction, &approved, &relations)
        .await
        .map_err(|error| Refusal::internal(report, error))?;
    if let Err(error) = plan.ensure_executable() {
        report.protected += u64::try_from(plan.blockers().len()).unwrap_or(u64::MAX);
        for blocker in plan.blockers() {
            increment(report, blocker_code(blocker));
        }
        return Err(Refusal::denied(report, "protected_dependency", error));
    }

    for unit in plan.approved() {
        let entity =
            entity_of_table(unit.table).ok_or_else(|| Refusal::gate(report, "unsupported_unit"))?;
        let attributes = row_attributes(transaction, unit.table, unit.id)
            .await
            .map_err(|error| Refusal::internal(report, error))?
            .ok_or_else(|| Refusal::gate(report, "native_row_missing"))?;
        pass.authorisation
            .ensure_mutation_access(
                Action::Delete,
                unit.id,
                with_object_type(entity, attributes),
            )
            .await
            .map_err(|error| Refusal::denied(report, "unauthorized", error))?;
    }
    for relation in plan.relations() {
        let entity = entity_of_table(relation.holder_table)
            .ok_or_else(|| Refusal::gate(report, "unsupported_unit"))?;
        let attributes = row_attributes(transaction, relation.holder_table, relation.holder_id)
            .await
            .map_err(|error| Refusal::internal(report, error))?
            .ok_or_else(|| Refusal::gate(report, "relation_holder_missing"))?;
        pass.authorisation
            .ensure_mutation_access(
                Action::Update,
                relation.holder_id,
                with_object_type(entity, attributes),
            )
            .await
            .map_err(|error| Refusal::denied(report, "unauthorized", error))?;
    }

    // Derived state must be inspected while the owners still exist.
    // Upserted owners were already repaired by the commit that ran before this
    // reconciliation; only the removals add derived work here.
    let repair = plan_derived_state_repair(transaction, plan.approved(), &[])
        .await
        .map_err(|error| Refusal::internal(report, error))?;

    // The canonical executor installs the fences and deletes children first in
    // one savepoint; the typed relation owner then updates each holder.
    execute_native_removal(transaction, &plan)
        .await
        .map_err(|error| Refusal::denied(report, "native_removal_failed", error))?;
    apply_native_relations(transaction, plan.relations())
        .await
        .map_err(|error| Refusal::denied(report, "native_removal_failed", error))?;

    for entity in tree_entities(&repair) {
        super::recompute_tree(transaction, entity)
            .await
            .map_err(|error| Refusal::internal(report, error))?;
    }
    super::counters::recompute(
        transaction,
        &repair.devices,
        &repair.virtual_machines,
        &repair.prefixes,
    )
    .await
    .map_err(|error| Refusal::internal(report, error))?;

    // The refused boundary is re-checked after the mutations, before this
    // transaction may report success.
    ensure_delete_operation(pass, report).await?;

    for candidate in candidates {
        let unit = &candidate.claim.unit;
        let removed = plan
            .approved()
            .iter()
            .any(|approved| approved.table == unit.table() && approved.id == unit.id())
            || plan.relations().iter().any(|relation| {
                relation.holder_table == unit.table() && relation.holder_id == unit.id()
            })
            || applied_already.contains(unit)
            || subordinate.contains(unit);
        if removed {
            report.applied += 1;
            report.applied_identity_sha256.push(identity_sha256(unit));
        }
    }
    report.applied_identity_sha256.sort_unstable();
    report.applied_identity_sha256.dedup();
    increment(report, "removed");

    // Retire the proved claims and clear their observations only after every
    // native effect is durable in this transaction. A subordinate relation claim
    // is retired by the holder's own removal without a separate native write.
    for unit in applied_already.iter().chain(subordinate.iter()) {
        let claim = candidates
            .iter()
            .find(|candidate| &candidate.claim.unit == unit)
            .map(|candidate| candidate.claim);
        let Some(claim) = claim else { continue };
        retire(transaction, report, claim).await?;
    }
    for candidate in candidates {
        let unit = &candidate.claim.unit;
        if applied_already.contains(unit) || subordinate.contains(unit) {
            continue;
        }
        retire(transaction, report, candidate.claim).await?;
    }
    Ok(())
}

async fn retire(
    transaction: &mut Transaction<'_, Postgres>,
    report: &ReconciliationReport,
    claim: &UnitOwnership,
) -> Result<(), Refusal> {
    let repository = SqlxOwnershipRepository;
    let mut claim = claim.clone();
    claim
        .mark_deleted(Utc::now())
        .map_err(|error| Refusal::internal(report, error))?;
    repository
        .retire(transaction, &claim)
        .await
        .map_err(|error| Refusal::internal(report, error))?;
    repository
        .clear_absent(transaction, claim.provider_id, &claim.unit)
        .await
        .map_err(|error| Refusal::internal(report, error))?;
    if claim.unit.kind() == NativeUnitKind::Record {
        // The native record this binding named no longer exists, so the binding
        // must not survive it: a later pull would resolve it to a missing row.
        sqlx::query(
            "DELETE FROM sync_inventory_binding \
             WHERE source_provider_id = $1 AND source_entity = $2 \
               AND external_id = $3 AND local_id = $4",
        )
        .bind(claim.provider_id)
        .bind(&claim.source_entity)
        .bind(&claim.external_id)
        .bind(claim.unit.id())
        .execute(&mut **transaction)
        .await
        .map_err(|error| Refusal::internal(report, AppError::Database(error.to_string())))?;
    }
    Ok(())
}

/// Whether the claimed native unit still exists and still matches its proof.
async fn verify(
    transaction: &mut Transaction<'_, Postgres>,
    unit: &NativeUnit,
    entity: InventoryEntity,
    claim: &UnitOwnership,
) -> Result<Verified, AppError> {
    let repository = SqlxOwnershipRepository;
    let (present, current): (bool, Option<UnitDigest>) = match unit.kind() {
        NativeUnitKind::Record => {
            let present = row_exists(transaction, unit.table(), unit.id()).await?;
            let current = ownership::record_state(transaction, entity, unit)
                .await?
                .map(|state| state.digest);
            (present, current)
        }
        NativeUnitKind::TagMembership => (
            ownership::tag_exists(transaction, unit).await?,
            Some(ownership::membership_digest(unit)),
        ),
        NativeUnitKind::JunctionMember => (
            junction_member_exists(transaction, unit).await?,
            Some(ownership::membership_digest(unit)),
        ),
        NativeUnitKind::CableAttachment => {
            let current = ownership::cable_state(transaction, unit.id()).await?;
            let present = current.as_ref().is_some_and(|(cable, end, _)| {
                Some(*cable) == unit.member() && end.as_str() == unit.member_token_column()
            });
            (present, current.map(|(_, _, digest)| digest))
        }
    };
    if !present {
        return Ok(Verified::AlreadyAbsent);
    }
    let Some(digest) = current else {
        return Ok(Verified::Uncovered);
    };
    let Some(generation) = repository.lifecycle_generation(transaction, unit).await? else {
        return Ok(Verified::Uncovered);
    };
    Ok(
        if digest == claim.applied.digest && generation == claim.applied.lifecycle_generation {
            Verified::Intact
        } else {
            Verified::Modified
        },
    )
}

fn entity_of_table(table: &str) -> Option<InventoryEntity> {
    InventoryEntity::all().find(|entity| entity.table() == Some(table))
}

fn with_object_type(entity: InventoryEntity, mut attributes: Value) -> Value {
    if let Some((app, model)) = entity.content_type()
        && let Some(object) = attributes.as_object_mut()
    {
        object.insert(
            "object_type".into(),
            Value::String(format!("{app}.{model}")),
        );
    }
    attributes
}

fn tree_entities(repair: &DerivedStateRepair) -> Vec<InventoryEntity> {
    let mut entities: Vec<InventoryEntity> = repair
        .trees
        .iter()
        .filter_map(|(table, _)| entity_of_table(table))
        .filter(|entity| entity.is_tree())
        .collect();
    entities.sort_unstable();
    entities.dedup();
    entities
}

fn blocker_code(blocker: &NativeBlocker) -> &'static str {
    match blocker {
        NativeBlocker::RetainedDependent { .. } => "retained_dependent",
        NativeBlocker::TypedDependency { .. } => "typed_dependency",
        NativeBlocker::UnsupportedEffect { .. } => "unsupported_effect",
        NativeBlocker::RelationHolderInRemovalSet { .. } => "relation_holder_in_removal_set",
    }
}

fn increment(report: &mut ReconciliationReport, code: &str) {
    *report.reason_codes.entry(code.to_string()).or_insert(0) += 1;
}

/// The native identity of one unit, as a comparable key.
type UnitKey = (String, String, Uuid, Uuid, String);

fn unit_key(unit: &NativeUnit) -> UnitKey {
    (
        unit.kind().as_str().to_string(),
        unit.table().to_string(),
        unit.id(),
        unit.member_column(),
        unit.member_token_column().to_string(),
    )
}

async fn junction_member_exists(
    transaction: &mut Transaction<'_, Postgres>,
    unit: &NativeUnit,
) -> Result<bool, AppError> {
    let Some((first, second)) = junction_pair_columns(transaction, unit.table()).await? else {
        return Ok(false);
    };
    let member = unit
        .member()
        .ok_or_else(|| AppError::Internal("inventory_reconciliation:member".into()))?;
    let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM ");
    query
        .push(unit.table())
        .push(" WHERE (\"")
        .push(&first)
        .push("\" = ")
        .push_bind(unit.id())
        .push(" AND \"")
        .push(&second)
        .push("\" = ")
        .push_bind(member)
        .push(") OR (\"")
        .push(&first)
        .push("\" = ")
        .push_bind(member)
        .push(" AND \"")
        .push(&second)
        .push("\" = ")
        .push_bind(unit.id())
        .push("))");
    Ok(query
        .build_query_scalar()
        .fetch_one(&mut **transaction)
        .await?)
}

/// The catalogue-owned pair columns of a native junction, validated as plain
/// SQL identifiers before they are interpolated.
async fn junction_pair_columns(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
) -> Result<Option<(String, String)>, AppError> {
    let columns: Option<(String, String)> = sqlx::query_as(
        "SELECT first_column, second_column FROM sync_native_junction_tables() \
         WHERE table_name = $1",
    )
    .bind(table)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((first, second)) = columns else {
        return Ok(None);
    };
    for column in [&first, &second] {
        if column.is_empty()
            || column.len() > 63
            || !column.as_bytes()[0].is_ascii_lowercase()
            || !column
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(AppError::Internal(
                "inventory_reconciliation:junction_column".into(),
            ));
        }
    }
    Ok(Some((first, second)))
}

async fn row_exists(
    transaction: &mut Transaction<'_, Postgres>,
    table: &'static str,
    id: Uuid,
) -> Result<bool, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM ");
    query
        .push(table)
        .push(" WHERE id = ")
        .push_bind(id)
        .push(")");
    Ok(query
        .build_query_scalar()
        .fetch_one(&mut **transaction)
        .await?)
}

/// The locked persisted attributes of one native row, as the policy sees them.
///
/// The row is locked `FOR UPDATE` so the attributes that decide the policy
/// decision cannot change between this read and the mutation it authorises.
async fn row_attributes(
    transaction: &mut Transaction<'_, Postgres>,
    table: &'static str,
    id: Uuid,
) -> Result<Option<Value>, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT to_jsonb(t) FROM ");
    query
        .push(table)
        .push(" t WHERE id = ")
        .push_bind(id)
        .push(" FOR UPDATE");

    Ok(query
        .build_query_scalar()
        .fetch_optional(&mut **transaction)
        .await?)
}

/// Apply the typed relationship removals.
///
/// This is the write side of [`NativeRelationRemoval`]: a tag membership leaves
/// its holder's `tags` array, a junction member deletes its own pair row, and a
/// cable attachment clears only its own edge. The holder row is never deleted and
/// members that are not named by the relation are never touched.
async fn apply_native_relations(
    transaction: &mut Transaction<'_, Postgres>,
    relations: &[NativeRelationRemoval],
) -> Result<(), AppError> {
    for relation in relations {
        let changed = match &relation.kind {
            NativeRelationKind::Tag { tag_id } => {
                let mut query = QueryBuilder::<Postgres>::new("UPDATE ");
                query
                    .push(relation.holder_table)
                    .push(" SET tags = array_remove(tags, ")
                    .push_bind(tag_id.to_string())
                    .push(") WHERE id = ")
                    .push_bind(relation.holder_id)
                    .push(" AND tags @> ARRAY[")
                    .push_bind(tag_id.to_string())
                    .push("]::text[]");
                query
                    .build()
                    .execute(&mut **transaction)
                    .await?
                    .rows_affected()
            }
            NativeRelationKind::Junction { table, member_id } => {
                let Some((first, second)) = junction_pair_columns(transaction, table).await? else {
                    return Err(AppError::Conflict(
                        "inventory_reconciliation:junction_not_catalogued".into(),
                    ));
                };
                let mut query = QueryBuilder::<Postgres>::new("DELETE FROM ");
                query
                    .push(*table)
                    .push(" WHERE (\"")
                    .push(&first)
                    .push("\" = ")
                    .push_bind(relation.holder_id)
                    .push(" AND \"")
                    .push(&second)
                    .push("\" = ")
                    .push_bind(member_id)
                    .push(") OR (\"")
                    .push(&first)
                    .push("\" = ")
                    .push_bind(member_id)
                    .push(" AND \"")
                    .push(&second)
                    .push("\" = ")
                    .push_bind(relation.holder_id)
                    .push(")");
                query
                    .build()
                    .execute(&mut **transaction)
                    .await?
                    .rows_affected()
            }
            NativeRelationKind::Cable { cable_id, end } => {
                let mut query = QueryBuilder::<Postgres>::new("UPDATE ");
                query
                    .push(relation.holder_table)
                    .push(
                        " SET cable_id = NULL, cable_end = NULL, cable_connector = NULL, \
                         cable_positions = NULL WHERE id = ",
                    )
                    .push_bind(relation.holder_id)
                    .push(" AND cable_id = ")
                    .push_bind(cable_id)
                    .push(" AND cable_end = ")
                    .push_bind(end.as_str());
                query
                    .build()
                    .execute(&mut **transaction)
                    .await?
                    .rows_affected()
            }
        };
        if changed != 1 {
            return Err(AppError::Conflict(
                "inventory_reconciliation:relation_changed".into(),
            ));
        }
    }
    Ok(())
}

/// A stable, secret-free identity for the report: native keys only.
fn identity_sha256(unit: &NativeUnit) -> String {
    digest(
        format!(
            "piping.reconciliation.unit/v1{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            unit.kind().as_str(),
            unit.table(),
            unit.id(),
            unit.member_column(),
            unit.member_token_column()
        )
        .as_bytes(),
    )
}
