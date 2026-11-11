//! SQL local inventory target (PIP-86).
//!
//! [`LocalInventoryCatalog`] implements the
//! [`LocalInventoryTarget`](crate::contexts::sync::domain::inventory_sync::LocalInventoryTarget)
//! port: [`prepare`](LocalInventoryTarget::prepare) turns a full source snapshot
//! into a typed batch validated against the closed
//! [`inventory_target_schema`](crate::contexts::sync::domain::inventory_target_schema)
//! catalogue, and [`commit_pull`](LocalInventoryTarget::commit_pull) writes the
//! whole graph plus its binding rows and the job's terminal state in one
//! transaction.
//!
//! Design rules enforced here:
//!
//! * Every physical table/column comes from the closed catalogue; a caller can
//!   never name one.
//! * A source record is bound by its provider-scoped identity (UUIDv5 over the
//!   provider namespace and the `(source_entity, external_id, target_entity)`
//!   tuple); an existing binding always wins and a row is never adopted by a
//!   mutable natural key.
//! * A populated source field needs an approved mapping or a typed derived/
//!   handled-elsewhere disposition. Unsupported fields block instead of dropping.
//! * The graph is resolved and validated before any write; a late failure
//!   rolls the whole transaction back, so no partial binding or half-written
//!   row can survive.
//! * No raw provider body, credential or unmapped value is ever logged or
//!   stored in `sync_job.stats`.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::DateTime;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::custom_fields::domain::{
    CustomFieldDefinition, CustomFieldType, FilterLogic, UiEditable, UiVisible,
    validate_definition, validate_value,
};
use crate::contexts::inventory::domain::ipam::service::validate_port_mappings;
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::{
    lock_tag_catalog_for_definition, lock_tag_catalog_for_usage, validate_scope_registry_in_transaction,
    validate_scope_token,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::hardware_graph::{
    lock_hardware_graph, prepare_explicit_hardware_replay,
};
use crate::contexts::sync::domain::inventory_sync::{
    CommitPull, InventoryCommitStats, InventoryValue, LocalInventoryCatalogView,
    LocalInventoryResource, LocalInventoryResourceField, LocalInventoryTarget,
    MappingConfigDocument, SourceRecord, SourceSnapshot, TypedInventoryBatch, TypedInventoryRow,
};
use crate::contexts::sync::domain::inventory_target_schema::{
    Absent, FieldKind, InventoryEntity, InventoryField, PgType,
};
use crate::contexts::sync::domain::sync_job::{SyncJob, SyncStatus};
use crate::contexts::sync::infrastructure::providers::immutable_inventory_source::{
    ImmutableInventorySource, VerifiedTagRelationships,
};

mod counters;
mod custom_field_references;
mod dataflow_selectors;
mod interface_graph;
mod ownership;
mod reconciliation;
mod source_cache_assertions;
mod tag_memberships;
mod tag_scopes;
mod template_validation;
mod transfer_fence;
use crate::contexts::sync::domain::inventory_export::digest;
use crate::contexts::sync::domain::inventory_preflight::{
    Capability, DependencyFact, Finding, FindingCode, IdentityFact, PreparationAssessment,
    Severity, TargetPlanAssessment,
};
use crate::contexts::sync::domain::inventory_transfer::{
    PinnedTransferDescriptor, RetainedAbsence, TRANSFER_FORMAT_VERSION, TransferOutcome,
    TransferReceipt, TransferReceiptStats, TransferTarget,
};
use crate::contexts::sync::domain::{
    EntityMapping, FieldMapping, InventoryBinding, InventoryCommitAuthorisation, InventoryIdentity,
    SourceEntity, SyncJobStep, SyncScope, TransformDirection, evaluate_expression,
};
use crate::contexts::sync::infrastructure::persistence::repositories::sync_job_repository;
use crate::errors::AppError;

/// A provider-scoped reference to another record, captured during `prepare` and
/// resolved to a local id only inside the commit transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceBinding {
    /// Local target entity, or `None` for an out-of-scope relation.
    pub target: Option<InventoryEntity>,
    /// Canonical `app_label.model` token for a generic reference.
    pub type_token: Option<String>,
    /// The provider-side id used to look the binding up.
    pub external_id: String,
}

/// Source observations belong to the provider binding, not local object dates.
/// An omitted value preserves a previous observation; an explicit null clears it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SourceTimestamps {
    created: Option<Option<DateTime<chrono::Utc>>>,
    updated: Option<Option<DateTime<chrono::Utc>>>,
}

impl SourceTimestamps {
    fn from_record(record: &SourceRecord) -> Result<Self, Vec<&'static str>> {
        let mut timestamps = Self::default();
        let mut invalid = Vec::new();
        for (field, target) in [
            ("created", &mut timestamps.created),
            ("last_updated", &mut timestamps.updated),
        ] {
            *target = match record.fields.get(field) {
                None => None,
                Some(Value::Null) => Some(None),
                Some(value @ Value::String(raw)) => {
                    // Chrono ignores fractional digits after the ninth. Check the
                    // original input so nonzero sub-microseconds cannot disappear.
                    let precision_is_exact = raw.split_once('.').is_none_or(|(_, fraction)| {
                        fraction
                            .bytes()
                            .take_while(u8::is_ascii_digit)
                            .skip(6)
                            .all(|digit| digit == b'0')
                    });
                    match serde_json::from_value::<DateTime<chrono::Utc>>(value.clone()) {
                        // PostgreSQL preserves microseconds, not finer precision or
                        // leap-second representations. Refuse lossy observations.
                        Ok(value)
                            if precision_is_exact
                                && value.timestamp_subsec_nanos() < 1_000_000_000
                                && value.timestamp_subsec_nanos().is_multiple_of(1_000) =>
                        {
                            Some(Some(value))
                        }
                        _ => {
                            invalid.push(field);
                            None
                        }
                    }
                }
                Some(_) => {
                    invalid.push(field);
                    None
                }
            };
        }
        if invalid.is_empty() {
            Ok(timestamps)
        } else {
            Err(invalid)
        }
    }
}

/// One prepared row: identity, typed scalars, and the relations to resolve at
/// commit time.
#[derive(Debug, Clone)]
pub struct PreparedRow {
    entity: InventoryEntity,
    source_entity: String,
    external_id: String,
    local_id: Uuid,
    source_timestamps: SourceTimestamps,
    scalars: BTreeMap<InventoryField, InventoryValue>,
    references: Vec<(InventoryField, ReferenceBinding)>,
    /// Relations whose source value was explicitly null: their columns must be
    /// set to SQL NULL (GFK type + id together), never left stale.
    cleared: Vec<InventoryField>,
    junctions: Vec<(InventoryField, Vec<ReferenceBinding>)>,
    /// Ordered typed selector rows of the optional `data-flows` namespace.
    selectors: Vec<(InventoryField, Vec<ReferenceBinding>)>,
    custom_fields: Option<serde_json::Map<String, Value>>,
    /// Canonical, already-validated definition for a `CustomField` row, so the
    /// commit preflight can seed the incoming-definition cache without the raw
    /// source record.
    prepared_definition: Option<CustomFieldDefinition>,
    tags: Option<Vec<crate::contexts::sync::domain::SourceIdentity>>,
}

/// Independently observed mapping facts, not a row eligible for writing. A
/// failure in another field must not erase a known slug or embedded tag list.
struct PreparationFacts {
    identity: crate::contexts::sync::domain::SourceIdentity,
    entity: InventoryEntity,
    definition_key: Option<(&'static str, String)>,
    tags: Option<Vec<String>>,
    tags_relevant: bool,
}

fn fact_finding(fact: &PreparationFacts, code: FindingCode, field: &str) -> Finding {
    let mut finding = Finding::blocker(code, Capability::MappingValidation);
    finding.entity = Some(fact.entity.source_key().into());
    finding.record_identity_sha256 = Some(digest(fact.identity.external_id.as_bytes()));
    finding.field_path = Some(field.into());
    finding
}

fn preparation_facts(
    entity: InventoryEntity,
    record: &SourceRecord,
    columns: &PreparedColumns,
    ambiguous: &HashSet<&str>,
    failed: &HashSet<&str>,
) -> PreparationFacts {
    let text = |name: &str| {
        if ambiguous.contains(name) {
            return None;
        }
        entity
            .field(name)
            .and_then(|field| columns.scalars.get(&field))
            .and_then(|value| match value {
                InventoryValue::Text { value } => Some(value.clone()),
                _ => None,
            })
    };
    let tag_slug = (entity == InventoryEntity::Tag)
        .then(|| text("slug"))
        .flatten();
    let definition_key = if entity == InventoryEntity::CustomField {
        text("name").map(|name| ("name", name))
    } else {
        tag_slug.clone().map(|slug| ("slug", slug))
    };
    PreparationFacts {
        identity: record.identity.clone(),
        entity,
        definition_key,
        tags: if ambiguous.contains("tags") {
            None
        } else {
            columns
                .tags
                .as_ref()
                .map(|tags| tags.iter().map(|tag| tag.external_id.clone()).collect())
        },
        tags_relevant: record.fields.contains_key("tags")
            || columns.tags.is_some()
            || ambiguous.contains("tags")
            || failed.contains("tags"),
    }
}

impl PreparedRow {
    /// Whether this row explicitly carries an intent for the catalog field.
    /// Explicit nulls and empty relation sets count as intents.
    pub(crate) fn has_field(&self, field: InventoryField) -> bool {
        field.entity() == self.entity
            && (self.scalars.contains_key(&field)
                || self
                    .references
                    .iter()
                    .any(|(candidate, _)| *candidate == field)
                || self.cleared.contains(&field)
                || self
                    .junctions
                    .iter()
                    .any(|(candidate, _)| *candidate == field)
                || (matches!(field.kind(), FieldKind::Tags) && self.tags.is_some()))
    }

    /// Whether a custom-field object was explicitly supplied, including empty.
    pub(crate) fn has_custom_fields(&self) -> bool {
        self.custom_fields.is_some()
    }

    /// Canonical local id derived for this source record.
    #[must_use]
    pub fn local_id(&self) -> Uuid {
        self.local_id
    }

    /// The binding target key for this row.
    #[must_use]
    pub fn target_key(&self) -> String {
        self.entity.target_key()
    }

    /// Provider-side identity string (never a field value).
    #[must_use]
    pub fn external_id(&self) -> &str {
        &self.external_id
    }
}

impl TypedInventoryRow for PreparedRow {
    fn entity(&self) -> InventoryEntity {
        self.entity
    }

    fn value(&self, field: InventoryField) -> Option<&InventoryValue> {
        self.scalars.get(&field)
    }
}

/// A prepared batch of typed rows.
#[derive(Debug, Clone, Default)]
pub struct LocalInventoryBatch {
    source_provider_id: Uuid,
    source_snapshot: bool,
    coverage: Option<crate::contexts::sync::domain::inventory_sync::CompletedSourceCoverage>,
    mapping_digest: String,
    generation: Option<ConfiguredGeneration>,
    rows: Vec<PreparedRow>,
    extracted_count: u64,
    tag_membership_count: u64,
    entities_extracted: BTreeMap<String, u64>,
    cable_sides: Vec<interface_graph::SideAssertion>,
}

#[derive(Debug, Clone)]
struct ConfiguredGeneration {
    generation: crate::contexts::sync::domain::SourceGeneration,
    coverage: crate::contexts::sync::domain::inventory_sync::CompletedSourceCoverage,
    binding: crate::contexts::sync::domain::absence::ProviderGenerationBinding,
    mapping: crate::contexts::sync::domain::MappingRef,
    mapping_digest: String,
    policy: crate::contexts::sync::domain::AbsencePolicy,
}

/// The credential owner revalidates and locks the captured binding in this transaction.
pub trait TransactionGenerationAuthority: Send + Sync {
    fn revalidate<'a, 't>(
        &'a self,
        transaction: &'a mut sqlx::Transaction<'t, sqlx::Postgres>,
        binding: &'a crate::contexts::sync::domain::absence::ProviderGenerationBinding,
    ) -> futures_util::future::BoxFuture<'a, Result<(), AppError>>;
}

impl LocalInventoryBatch {
    /// Collector evidence remains observational; generation authority is separate.
    #[must_use]
    pub fn source_coverage(
        &self,
    ) -> Option<&crate::contexts::sync::domain::inventory_sync::CompletedSourceCoverage> {
        self.coverage.as_ref()
    }

    #[must_use]
    pub fn mapping_digest(&self) -> &str {
        &self.mapping_digest
    }

    /// The provider this batch belongs to.
    #[must_use]
    pub fn source_provider_id(&self) -> Uuid {
        self.source_provider_id
    }
}

impl TypedInventoryBatch for LocalInventoryBatch {
    type Row = PreparedRow;

    fn rows(&self) -> &[Self::Row] {
        &self.rows
    }

    fn extracted_count(&self) -> u64 {
        self.extracted_count
    }
}

/// The concrete SQL local inventory target.
#[derive(Clone)]
pub struct LocalInventoryCatalog {
    pool: PgPool,
    generation_authority: Option<std::sync::Arc<dyn TransactionGenerationAuthority>>,
}

impl LocalInventoryCatalog {
    /// Build the target over a connection pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            generation_authority: None,
        }
    }

    pub fn with_generation_authority(
        mut self,
        authority: std::sync::Arc<dyn TransactionGenerationAuthority>,
    ) -> Self {
        self.generation_authority = Some(authority);
        self
    }

    async fn revalidate_generation(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        generation: &ConfiguredGeneration,
    ) -> Result<(), AppError> {
        let authority = self.generation_authority.as_ref().ok_or_else(|| {
            AppError::Validation("sync.source_generation_authority_missing".into())
        })?;
        authority
            .revalidate(transaction, &generation.binding)
            .await?;
        let provider_type: String =
            sqlx::query_scalar("SELECT provider_type FROM sync_provider WHERE id = $1")
                .bind(generation.binding.provider_id)
                .fetch_one(&mut **transaction)
                .await?;
        if provider_type != "netbox" {
            return Err(AppError::Validation(
                "sync.source_generation_provider_changed".into(),
            ));
        }
        let mapping = crate::contexts::sync::infrastructure::persistence::SqlxMappingConfigurationRepository::lock_document(transaction, generation.mapping).await?.ok_or_else(|| AppError::Validation("sync.source_generation_mapping_missing".into()))?;
        if mapping.status != crate::contexts::sync::domain::MappingConfigurationStatus::Active
            || mapping.source_provider_id != generation.binding.provider_id
            || mapping.absence_policy != generation.policy
            || mapping.content_digest() != generation.mapping_digest
            || generation.coverage.seal_mapping(&mapping)? != generation.generation
        {
            return Err(AppError::Validation(
                "sync.source_generation_mapping_changed".into(),
            ));
        }
        Ok(())
    }

    /// Observe the exact target and resolved plan to pin before authorisation.
    pub async fn transfer_observation(
        &self,
        batch: &LocalInventoryBatch,
    ) -> Result<(TransferTarget, String), AppError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await?;
        let (target, schema) = transfer_fence::target_and_schema(&mut transaction).await?;
        let fingerprint = fingerprint_target(&mut transaction).await?;
        let analysis = plan_prewrite(batch, &mut transaction).await?;
        let planned = analysis.planned.ok_or_else(|| {
            blocked_assessment_error(
                "inventory_transfer:target_graph_blocked",
                &analysis.findings,
                analysis.complete,
            )
        })?;
        let plan_digest = resolved_plan_digest(batch, &planned)?;
        ensure_tag_scope_plan(batch, &planned, &mut transaction).await?;
        transaction.rollback().await?;
        Ok((
            TransferTarget {
                identity_sha256: digest(target.as_bytes()),
                fingerprint_sha256: fingerprint,
                plan_digest,
            },
            schema,
        ))
    }

    /// Read the committed receipt without executing inventory mutation.
    pub async fn transfer_receipt(
        &self,
        run_id: Uuid,
        descriptor_digest: &str,
    ) -> Result<Option<TransferReceipt>, AppError> {
        if run_id.is_nil()
            || !crate::contexts::sync::domain::inventory_export::valid_digest(descriptor_digest)
        {
            return Err(AppError::Validation(
                "inventory_transfer:receipt_identity".into(),
            ));
        }
        let Some(value) = transfer_fence::receipt(&self.pool, run_id, descriptor_digest).await?
        else {
            return Ok(None);
        };
        let receipt: TransferReceipt = serde_json::from_value(value)
            .map_err(|_| AppError::Database("inventory_transfer:receipt_invalid".into()))?;
        receipt.verify(run_id, descriptor_digest)?;
        Ok(Some(receipt))
    }

    /// Commit one pinned transfer, or return its existing durable receipt.
    pub async fn apply_transfer(
        &self,
        batch: LocalInventoryBatch,
        pinned: &PinnedTransferDescriptor,
        authorisation: std::sync::Arc<dyn InventoryCommitAuthorisation>,
    ) -> Result<TransferReceipt, AppError> {
        self.apply_transfer_inner(batch, pinned, authorisation)
            .await
            .map_err(|error| match error {
                AppError::Database(message) if message.starts_with("inventory_transfer:") => {
                    AppError::Database(message)
                }
                AppError::Database(_) | AppError::DatabaseSource(_) => {
                    AppError::Database("inventory_transfer:database_error".into())
                }
                other => other,
            })
    }

    async fn apply_transfer_inner(
        &self,
        batch: LocalInventoryBatch,
        pinned: &PinnedTransferDescriptor,
        authorisation: std::sync::Arc<dyn InventoryCommitAuthorisation>,
    ) -> Result<TransferReceipt, AppError> {
        let descriptor = pinned.descriptor();
        descriptor.validate()?;
        if batch.source_provider_id != descriptor.provider_id {
            return Err(AppError::Validation(
                "inventory_transfer:provider_mismatch".into(),
            ));
        }
        if let Some(receipt) = self
            .transfer_receipt(descriptor.run_id, pinned.descriptor_digest())
            .await?
        {
            return Ok(receipt);
        }
        transfer_fence::register(
            &self.pool,
            descriptor.run_id,
            pinned.descriptor_digest(),
            pinned.bytes(),
        )
        .await?;
        let mut tx = transfer_fence::begin(&self.pool, descriptor.provider_id).await?;
        if let Some(value) =
            transfer_fence::lock_run(&mut tx, descriptor.run_id, pinned.descriptor_digest()).await?
        {
            let receipt: TransferReceipt = serde_json::from_value(value)
                .map_err(|_| AppError::Database("inventory_transfer:receipt_invalid".into()))?;
            receipt.verify(descriptor.run_id, pinned.descriptor_digest())?;
            return Ok(receipt);
        }
        authorisation.ensure_owned().await?;
        descriptor.ensure_authority_current(chrono::Utc::now())?;
        if batch
            .rows
            .iter()
            .any(|row| row.entity == InventoryEntity::Tag)
        {
            lock_tag_catalog_for_definition(&mut tx).await?;
        } else {
            lock_tag_catalog_for_usage(&mut tx).await?;
        }
        lock_hardware_graph(&mut tx).await?;
        transfer_fence::lock_observation_tables(&mut tx).await?;
        let (target, schema) = transfer_fence::target_and_schema(&mut tx).await?;
        let fingerprint = fingerprint_target(&mut tx).await?;
        if digest(target.as_bytes()) != descriptor.target.identity_sha256
            || schema != descriptor.schema_digest
            || fingerprint != descriptor.target.fingerprint_sha256
        {
            return Err(AppError::Conflict("inventory_transfer:stale_target".into()));
        }
        let analysis = plan_prewrite(&batch, &mut tx).await?;
        let planned = analysis.planned.ok_or_else(|| {
            blocked_assessment_error(
                "inventory_transfer:target_graph_blocked",
                &analysis.findings,
                analysis.complete,
            )
        })?;
        if resolved_plan_digest(&batch, &planned)? != descriptor.target.plan_digest {
            return Err(AppError::Conflict("inventory_transfer:stale_plan".into()));
        }
        let incoming: HashSet<_> = batch
            .rows
            .iter()
            .map(|row| (row.source_entity.clone(), row.external_id.clone()))
            .collect();
        let bindings = load_bindings(&mut tx, batch.source_provider_id).await?;
        let mut retained_absent = Vec::new();
        for ((source, external_id), id) in bindings.valid {
            if incoming.contains(&(source.clone(), external_id.clone())) {
                continue;
            }
            let entity = InventoryEntity::from_source_key(&source)
                .ok_or_else(|| AppError::Validation("inventory_transfer:binding_invalid".into()))?;
            let table = entity
                .table()
                .ok_or_else(|| AppError::Validation("inventory_transfer:binding_invalid".into()))?;
            let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
            query.push(table).push(" WHERE id=").push_bind(id).push(")");
            let exists: bool = query.build_query_scalar().fetch_one(&mut *tx).await?;
            if !exists {
                return Err(AppError::Conflict(
                    "inventory_transfer:bound_row_missing".into(),
                ));
            }
            retained_absent.push(RetainedAbsence::new(&source, &external_id, id)?);
        }
        retained_absent.sort_by(|a, b| {
            (&a.source_entity, &a.external_id_sha256, a.local_id).cmp(&(
                &b.source_entity,
                &b.external_id_sha256,
                b.local_id,
            ))
        });
        let (receipt_stats, changed) =
            transfer_change_stats(&mut tx, &planned, batch.tag_membership_count).await?;
        if (receipt_stats.created > 0 && !descriptor.scope.creates)
            || ((receipt_stats.updated > 0 || receipt_stats.unchanged > 0)
                && !descriptor.scope.updates)
            || (batch.rows.iter().any(|row| row.custom_fields.is_some())
                && !descriptor.scope.custom_fields)
            || (batch
                .rows
                .iter()
                .any(|row| !row.references.is_empty() || !row.junctions.is_empty())
                && !descriptor.scope.object_references)
        {
            return Err(AppError::Validation(
                "inventory_transfer:scope_mismatch".into(),
            ));
        }
        template_validation::authorize_mutations(
            &mut tx,
            &planned,
            &changed,
            authorisation.as_ref(),
        )
        .await?;
        template_validation::authorize_original_admitted_references(
            &mut tx,
            &planned,
            authorisation.as_ref(),
        )
        .await?;
        let admitted_mutations = template_validation::authorize_admitted_mutations(
            &mut tx,
            &planned,
            &changed,
            authorisation.as_ref(),
        )
        .await?;
        ensure_tag_scope_plan(&batch, &planned, &mut tx).await?;
        let now = chrono::Utc::now();
        let mut job = SyncJob {
            id: descriptor.run_id,
            provider_id: descriptor.provider_id,
            mapping_version: descriptor.mapping_ref.to_string(),
            status: SyncStatus::Running,
            started_at: Some(now),
            finished_at: None,
            stats: Value::Object(serde_json::Map::new()),
        };
        sqlx::query("INSERT INTO sync_job(id, provider_id, mapping_version, status, started_at, stats) VALUES ($1,$2,$3,'running',$4,'{}'::jsonb)")
            .bind(job.id).bind(job.provider_id).bind(&job.mapping_version).bind(now)
            .execute(&mut *tx).await?;
        let mut stats = InventoryCommitStats {
            extracted: batch.extracted_count(),
            ..InventoryCommitStats::default()
        };
        let counter_device_owners = counters::device_owners(&mut tx, &planned).await?;
        commit_planned_rows(
            &mut tx,
            batch.source_provider_id,
            job.id,
            &planned,
            &mut stats,
            Some(&changed),
        )
        .await?;
        let changed_rows: Vec<_> = planned
            .iter()
            .filter(|row| changed.contains(&(row.entity, row.local_id)))
            .cloned()
            .collect();
        rebuild_touched_trees(&mut tx, &changed_rows).await?;
        template_validation::validate(&mut tx, &changed_rows, authorisation.as_ref()).await?;
        template_validation::authorize_final_admitted_references(
            &mut tx,
            &planned,
            authorisation.as_ref(),
        )
        .await?;
        template_validation::authorize_final_admitted_mutations(
            &mut tx,
            &planned,
            admitted_mutations,
            authorisation.as_ref(),
        )
        .await?;
        recompute_counts(&mut tx, &changed_rows, &counter_device_owners).await?;
        stats.tag_memberships_committed = batch.tag_membership_count;
        stats.entities_extracted = batch.entities_extracted;
        stats.source_capability = batch
            .coverage
            .as_ref()
            .and_then(|coverage| coverage.capability())
            .map(str::to_owned);
        stats.entities_committed.insert(
            InventoryEntity::TaggedObject.source_key().into(),
            batch.tag_membership_count,
        );
        job.complete();
        job.stats = serde_json::to_value(&stats)
            .map_err(|_| AppError::Internal("inventory_transfer:stats_encoding".into()))?;
        if let Some(object) = job.stats.as_object_mut() {
            object.insert(
                "retained_absent".into(),
                serde_json::json!(retained_absent.len()),
            );
        } else {
            return Err(AppError::Internal(
                "inventory_transfer:stats_encoding".into(),
            ));
        }
        update_job(&mut tx, &job).await?;
        let receipt = TransferReceipt {
            format_version: TRANSFER_FORMAT_VERSION,
            run_id: descriptor.run_id,
            descriptor_digest: pinned.descriptor_digest().into(),
            outcome: TransferOutcome::Committed,
            sync_job_id: job.id,
            committed_at: job
                .finished_at
                .ok_or_else(|| AppError::Internal("inventory_transfer:job_incomplete".into()))?,
            target_fingerprint_sha256: fingerprint,
            stats: receipt_stats,
            retained_absent,
        };
        let value = serde_json::to_value(&receipt)
            .map_err(|_| AppError::Internal("inventory_transfer:receipt_encoding".into()))?;
        transfer_fence::complete(
            &mut tx,
            descriptor.run_id,
            pinned.descriptor_digest(),
            &value,
        )
        .await?;
        authorisation.ensure_owned().await?;
        descriptor.ensure_authority_current(chrono::Utc::now())?;
        if tx.commit().await.is_err() {
            return match self
                .transfer_receipt(descriptor.run_id, pinned.descriptor_digest())
                .await
            {
                Ok(Some(committed)) => Ok(committed),
                _ => Err(AppError::Database(
                    "inventory_transfer:commit_uncertain".into(),
                )),
            };
        }
        Ok(receipt)
    }

    /// Uses the same pre-write planner as commit, under an enforced read-only
    /// repeatable-read snapshot. No lease, job, writer or bootstrap is called.
    pub async fn plan_read_only(
        &self,
        batch: &LocalInventoryBatch,
    ) -> Result<TargetPlanAssessment, AppError> {
        let safe_error =
            || AppError::Database("inventory_preflight:target_observation_failed".into());
        let mut transaction = self.pool.begin().await.map_err(|_| safe_error())?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(|_| safe_error())?;
        let snapshot_identity: String = sqlx::query_scalar("SELECT pg_current_snapshot()::text")
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| safe_error())?;
        let snapshot_fingerprint = fingerprint_target(&mut transaction)
            .await
            .map_err(|_| safe_error())?;
        let mut report = TargetPlanAssessment {
            snapshot_identity,
            snapshot_fingerprint,
            fingerprint_scope: crate::contexts::sync::domain::inventory_preflight::TargetFingerprintScope::CatalogGraphAndBindings,
            identities: Vec::new(),
            dependencies: Vec::new(),
            findings: Vec::new(),
            analysis_complete: false,
            cyclic_components: Vec::new(),
        };
        let graph = plan_prewrite(batch, &mut transaction)
            .await
            .map_err(|_| safe_error())?;
        report.identities = graph.identities;
        report.dependencies = graph.dependencies;
        report.findings = graph.findings;
        report.analysis_complete = graph.complete;
        if let Some(planned) = &graph.planned {
            report.findings.extend(
                tag_scopes::assess(batch, planned, &mut transaction)
                    .await
                    .map_err(|_| safe_error())?,
            );
        }
        report.cyclic_components =
            crate::contexts::sync::domain::inventory_preflight::cyclic_components(
                &report.dependencies,
            );
        report.findings.extend(
            crate::contexts::sync::domain::inventory_preflight::future_two_pass_findings(
                &report.dependencies,
                &report.identities,
            ),
        );
        report.findings.sort();
        report.findings.dedup();
        transaction.rollback().await.map_err(|_| safe_error())?;
        Ok(report)
    }
}

struct GraphAnalysis {
    planned: Option<Vec<PlannedRow>>,
    identities: Vec<IdentityFact>,
    dependencies: Vec<DependencyFact>,
    findings: Vec<Finding>,
    complete: bool,
}

async fn ensure_tag_scope_plan(
    batch: &LocalInventoryBatch,
    planned: &[PlannedRow],
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AppError> {
    let findings = tag_scopes::assess(batch, planned, transaction).await?;
    if findings
        .iter()
        .any(|finding| finding.severity == Severity::Blocker)
    {
        return Err(blocked_assessment_error(
            "inventory_transfer:target_graph_blocked",
            &findings,
            true,
        ));
    }
    Ok(())
}

fn reference_finding(
    row: &PreparedRow,
    code: FindingCode,
    field: &str,
    binding: &ReferenceBinding,
    local_id: Option<Uuid>,
) -> Finding {
    let mut finding = prepared_finding(row, code, Some(field), Capability::TargetGraph);
    finding.edge = Some(
        crate::contexts::sync::domain::inventory_preflight::EdgeContext {
            target_entity: binding.target.map(|entity| entity.source_key().into()),
            target_identity_sha256: Some(digest(binding.external_id.as_bytes())),
            target_local_id: local_id,
        },
    );
    finding
}

async fn observed_content_type(
    transaction: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<String, Uuid>,
    token: &str,
) -> Result<Option<Uuid>, AppError> {
    match content_type_id(transaction, cache, token).await {
        Err(AppError::Validation(_)) => Ok(None),
        result => result,
    }
}

/// One SELECT-only planner for commit and assessment. Validation failures are
/// accumulated; operational database failures abort the observation.
async fn plan_prewrite(
    batch: &LocalInventoryBatch,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<GraphAnalysis, AppError> {
    let bindings = load_bindings(transaction, batch.source_provider_id).await?;
    let mut local_ids = merge_local_ids(batch, bindings.valid.clone());
    local_ids.retain(|key, _| !bindings.invalid_keys.contains(key));
    let mut findings = bindings.findings;
    let mut complete = true;
    let mut identities = Vec::new();
    for row in &batch.rows {
        let key = (row.source_entity.clone(), row.external_id.clone());
        if let Some(id) = local_ids.get(&key).copied() {
            // A binding is an identity claim, not permission to recreate a
            // deleted target. Conversely an unbound deterministic UUID must
            // never adopt an unrelated existing row.
            if let Some(table) = row.entity.table() {
                let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
                query.push(table).push(" WHERE id=").push_bind(id).push(")");
                let exists: bool = query
                    .build_query_scalar()
                    .fetch_one(&mut **transaction)
                    .await?;
                if exists != bindings.valid.contains_key(&key) {
                    findings.push(prepared_finding(
                        row,
                        FindingCode::TargetBindingsInvalid,
                        Some("binding.local_id"),
                        Capability::TargetGraph,
                    ));
                    local_ids.remove(&key);
                    complete = false;
                    continue;
                }
            }
            identities.push(IdentityFact {
                entity: row.source_entity.clone(),
                record_identity_sha256: digest(row.external_id.as_bytes()),
                local_id: id,
            });
        } else {
            findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some("identity"),
                Capability::TargetGraph,
            ));
            complete = false;
        }
    }
    assess_tag_slug_conflicts(transaction, batch, &local_ids, &mut findings, &mut complete).await?;
    let mut content_types = HashMap::new();
    let definitions =
        prepare_custom_field_definitions(batch, &local_ids, &mut content_types, transaction)
            .await?;
    findings.extend(definitions.findings.clone());
    complete &= definitions.complete;
    let incoming: HashSet<_> = batch
        .rows
        .iter()
        .filter_map(|row| {
            local_ids
                .get(&(row.source_entity.clone(), row.external_id.clone()))
                .map(|id| (row.entity, *id))
        })
        .collect();
    let mut existence = HashMap::new();
    let mut dependencies = Vec::new();
    let mut tentative = Vec::new();
    for row in &batch.rows {
        let before = findings.len();
        let from = local_ids
            .get(&(row.source_entity.clone(), row.external_id.clone()))
            .copied();
        let tag_bindings: Vec<_> = row
            .tags
            .iter()
            .flatten()
            .map(|tag| ReferenceBinding {
                target: Some(InventoryEntity::Tag),
                type_token: None,
                external_id: tag.external_id.clone(),
            })
            .collect();
        let mut references: Vec<_> = row
            .references
            .iter()
            .map(|(field, binding)| (*field, binding, None))
            .chain(row.junctions.iter().flat_map(|(field, bindings)| {
                bindings
                    .iter()
                    .enumerate()
                    .map(move |(index, binding)| (*field, binding, Some(index)))
            }))
            .collect();
        if let Some(field) = row.entity.field("tags") {
            references.extend(
                tag_bindings
                    .iter()
                    .enumerate()
                    .map(|(index, binding)| (field, binding, Some(index))),
            );
        }
        for (field, binding, index) in references {
            let path = index.map_or_else(
                || field.name().into(),
                |index| format!("{}[{index}]", field.name()),
            );
            if row.entity == InventoryEntity::CustomField && field.name() == "related_object_type" {
                let scope = if let Some(token) = &binding.type_token {
                    observed_content_type(transaction, &mut content_types, token).await?
                } else {
                    None
                };
                if let (Some(from), Some(to)) = (from, scope) {
                    dependencies.push(DependencyFact {
                        from,
                        from_entity: row.source_entity.clone(),
                        to,
                        to_entity: "inventory.content-types".into(),
                        field: path,
                    });
                } else if scope.is_none() {
                    findings.push(prepared_finding(
                        row,
                        FindingCode::CustomFieldScopeInvalid,
                        Some(&path),
                        Capability::TargetGraph,
                    ));
                }
                continue;
            }
            if matches!(field.kind(), FieldKind::ContentTypeJunction { .. }) {
                let scope = if let Some(token) = &binding.type_token {
                    observed_content_type(transaction, &mut content_types, token).await?
                } else {
                    None
                };
                if let Some(to) = scope {
                    if let Some(from) = from {
                        dependencies.push(DependencyFact {
                            from,
                            from_entity: row.source_entity.clone(),
                            to,
                            to_entity: "inventory.content-types".into(),
                            field: path,
                        });
                    }
                } else {
                    let mut finding = prepared_finding(
                        row,
                        FindingCode::CustomFieldScopeInvalid,
                        Some(&path),
                        Capability::TargetGraph,
                    );
                    finding.edge = Some(
                        crate::contexts::sync::domain::inventory_preflight::EdgeContext {
                            target_entity: None,
                            target_identity_sha256: binding
                                .type_token
                                .as_ref()
                                .map(|token| digest(token.as_bytes())),
                            target_local_id: None,
                        },
                    );
                    findings.push(finding);
                }
                continue;
            }
            if matches!(field.kind(), FieldKind::GenericReference { .. }) {
                let scope = if let Some(token) = &binding.type_token {
                    observed_content_type(transaction, &mut content_types, token).await?
                } else {
                    None
                };
                if scope.is_none() {
                    findings.push(reference_finding(
                        row,
                        FindingCode::CustomFieldScopeInvalid,
                        &path,
                        binding,
                        None,
                    ));
                }
            }
            let Some(entity) = binding.target else {
                findings.push(reference_finding(
                    row,
                    FindingCode::FieldUnsupported,
                    &path,
                    binding,
                    None,
                ));
                complete = false;
                continue;
            };
            let Some(to) = resolve_reference(binding, &local_ids) else {
                let unknown = bindings
                    .invalid_keys
                    .contains(&(entity.source_key().into(), binding.external_id.clone()));
                findings.push(reference_finding(
                    row,
                    if unknown {
                        FindingCode::AnalysisDependencyUnavailable
                    } else {
                        FindingCode::TargetReferenceMissing
                    },
                    &path,
                    binding,
                    None,
                ));
                if unknown {
                    complete = false;
                }
                continue;
            };
            if let Some(from) = from {
                dependencies.push(DependencyFact {
                    from,
                    from_entity: row.source_entity.clone(),
                    to,
                    to_entity: entity.source_key().into(),
                    field: path.clone(),
                });
            }
            let exists = if incoming.contains(&(entity, to)) {
                true
            } else if let Some(exists) = existence.get(&(entity, to)) {
                *exists
            } else if let Some(table) = entity.table() {
                let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
                query.push(table).push(" WHERE id=").push_bind(to).push(")");
                let exists: bool = query
                    .build_query_scalar()
                    .fetch_one(&mut **transaction)
                    .await?;
                existence.insert((entity, to), exists);
                exists
            } else {
                false
            };
            if !exists {
                findings.push(reference_finding(
                    row,
                    FindingCode::TargetReferenceMissing,
                    &path,
                    binding,
                    Some(to),
                ));
            }
        }
        let merged = resolve_custom_field_data(
            transaction,
            &definitions,
            &mut content_types,
            row,
            from,
            &local_ids,
            &incoming,
            &mut dependencies,
            &mut findings,
            &mut complete,
        )
        .await?;
        if from.is_some() && findings.len() == before {
            match plan_row(row, &local_ids, merged, &mut content_types, transaction).await {
                Ok(plan) => tentative.push(plan),
                Err(AppError::Validation(_)) => {
                    findings.push(prepared_finding(
                        row,
                        FindingCode::TargetRowInvalid,
                        Some("row.plan"),
                        Capability::TargetGraph,
                    ));
                    complete = false;
                }
                Err(error) => return Err(error),
            }
        } else {
            findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some("row.plan"),
                Capability::TargetGraph,
            ));
            complete = false;
        }
    }
    assess_natural_key_conflicts(transaction, &tentative, &mut findings, &mut complete).await?;
    assess_hierarchies(batch, &local_ids, transaction, &mut findings, &mut complete).await?;
    let (cable_findings, cables_complete) =
        interface_graph::assess_target(batch, &tentative, &local_ids, transaction).await?;
    findings.extend(cable_findings);
    complete &= cables_complete;
    identities.sort_by(|a, b| {
        (&a.entity, &a.record_identity_sha256).cmp(&(&b.entity, &b.record_identity_sha256))
    });
    dependencies.sort_by(|a, b| {
        (&a.from_entity, a.from, &a.to_entity, a.to, &a.field).cmp(&(
            &b.from_entity,
            b.from,
            &b.to_entity,
            b.to,
            &b.field,
        ))
    });
    findings.sort();
    findings.dedup();
    let planned = (complete
        && !findings
            .iter()
            .any(|finding| finding.severity == Severity::Blocker))
    .then_some(tentative);
    Ok(GraphAnalysis {
        planned,
        identities,
        dependencies,
        findings,
        complete,
    })
}

/// Overlay independent parent edges even when an unrelated row/definition failed.
/// SCCs locate every cyclic component; the existing forest walker still checks
/// hierarchy semantics and bookkeeping bounds.
async fn assess_hierarchies(
    batch: &LocalInventoryBatch,
    local_ids: &HashMap<(String, String), Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
    findings: &mut Vec<Finding>,
    complete: &mut bool,
) -> Result<(), AppError> {
    for entity in crate::contexts::sync::domain::inventory_target_schema::ENTITIES
        .iter()
        .map(|meta| meta.entity)
        .filter(|entity| entity.is_tree())
    {
        if !batch.rows.iter().any(|row| row.entity == entity) {
            continue;
        }
        let table = entity
            .table()
            .ok_or_else(|| AppError::Internal("inventory_preflight:tree_catalog".into()))?;
        let mut parents = load_parent_graph(transaction, table).await?;
        for row in batch.rows.iter().filter(|row| row.entity == entity) {
            let Some(id) = local_ids
                .get(&(row.source_entity.clone(), row.external_id.clone()))
                .copied()
            else {
                *complete = false;
                continue;
            };
            parents.entry(id).or_insert(None);
            if row
                .cleared
                .iter()
                .any(|field| field.column() == "parent_id")
            {
                parents.insert(id, None);
            }
            for (field, binding) in row
                .references
                .iter()
                .filter(|(field, _)| field.column() == "parent_id")
            {
                if let Some(parent) = resolve_reference(binding, local_ids) {
                    parents.insert(id, Some(parent));
                } else {
                    parents.remove(&id);
                    *complete = false;
                    findings.push(reference_finding(
                        row,
                        FindingCode::AnalysisDependencyUnavailable,
                        field.name(),
                        binding,
                        None,
                    ));
                }
            }
        }
        let edges: Vec<_> = parents
            .iter()
            .filter_map(|(from, parent)| {
                parent
                    .filter(|to| parents.contains_key(to))
                    .map(|to| DependencyFact {
                        from: *from,
                        from_entity: entity.source_key().into(),
                        to,
                        to_entity: entity.source_key().into(),
                        field: "parent".into(),
                    })
            })
            .collect();
        let components =
            crate::contexts::sync::domain::inventory_preflight::cyclic_components(&edges);
        for component in &components {
            for node in component {
                let id = &node.local_id;
                let row = batch.rows.iter().find(|row| {
                    row.entity == entity
                        && local_ids.get(&(row.source_entity.clone(), row.external_id.clone()))
                            == Some(id)
                });
                let mut finding = if let Some(row) = row {
                    prepared_finding(
                        row,
                        FindingCode::HierarchyCycle,
                        Some("parent"),
                        Capability::TargetGraph,
                    )
                } else {
                    let mut finding =
                        Finding::blocker(FindingCode::HierarchyCycle, Capability::TargetGraph);
                    finding.entity = Some(entity.source_key().into());
                    finding.field_path = Some("parent".into());
                    finding
                };
                finding.affected_count = component.len() as u64;
                finding.component = Some(component.clone());
                finding.edge = Some(
                    crate::contexts::sync::domain::inventory_preflight::EdgeContext {
                        target_entity: Some(entity.source_key().into()),
                        target_identity_sha256: None,
                        target_local_id: parents[id],
                    },
                );
                findings.push(finding);
            }
        }
        let (children, roots) = build_forest_hierarchy(&parents);
        if walk_forest(&roots, &children, parents.len()).is_err() && components.is_empty() {
            let mut finding =
                Finding::blocker(FindingCode::HierarchyInvalid, Capability::TargetGraph);
            finding.entity = Some(entity.source_key().into());
            finding.field_path = Some("parent".into());
            findings.push(finding);
            *complete = false;
        }
    }
    Ok(())
}

/// Hash actual ordered row values/graphs observed in the same transaction as
/// planning. Table identifiers originate only in the closed catalog.
async fn fingerprint_target(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<String, AppError> {
    let tables = fingerprint_tables();
    let mut observations = BTreeMap::new();
    for table in tables {
        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM ",
        );
        query.push(table).push(" t");
        let rows: Value = query
            .build_query_scalar()
            .fetch_one(&mut **transaction)
            .await?;
        observations.insert(table, rows);
    }
    let bytes = serde_json::to_vec(&observations)
        .map_err(|_| AppError::Internal("inventory_preflight:observation_encoding".into()))?;
    Ok(digest(&bytes))
}

/// The same closed set is used for the observation and its writer fence.
fn fingerprint_tables() -> std::collections::BTreeSet<&'static str> {
    let mut tables =
        std::collections::BTreeSet::from(["sync_inventory_binding", "inventory_content_type"]);
    for meta in crate::contexts::sync::domain::inventory_target_schema::ENTITIES {
        if let Some(table) = meta.entity.table() {
            tables.insert(table);
        }
        for field in meta.fields {
            match field.kind {
                FieldKind::Junction { table, .. }
                | FieldKind::ContentTypeJunction { table, .. } => {
                    tables.insert(table);
                }
                _ => {}
            }
        }
    }
    tables
}

/// Map one closed entity to the read-only resource view the routes serve.
fn resource_of(entity: InventoryEntity) -> Option<LocalInventoryResource> {
    let source_entity = SourceEntity::new(entity.source_key()).ok()?;
    Some(LocalInventoryResource {
        source_entity,
        target_entity: entity.target_key(),
        fields: entity
            .field_iter()
            .map(|field| LocalInventoryResourceField {
                name: field.name().to_string(),
                field_type: format!("{:?}", field.sql_type()).to_lowercase(),
                required: !field.nullable(),
            })
            .collect(),
    })
}

impl LocalInventoryCatalogView for LocalInventoryCatalog {
    fn resources(&self) -> Vec<LocalInventoryResource> {
        // The mandatory core manifest. Optional plugin entities join the pull
        // only from the live provider schema, never from this catalogue.
        InventoryEntity::core()
            .chain([
                InventoryEntity::TaggedObject,
                InventoryEntity::CableTermination,
            ])
            .filter_map(resource_of)
            .collect()
    }

    fn resource(&self, source_entity: &SourceEntity) -> Option<LocalInventoryResource> {
        InventoryEntity::from_source_key(source_entity.as_str()).and_then(resource_of)
    }

    fn binding_for_local(
        &self,
        entity: InventoryEntity,
        local_id: Uuid,
    ) -> futures_util::future::BoxFuture<'_, Result<Option<InventoryBinding>, AppError>> {
        Box::pin(async move {
            let row = sqlx::query("SELECT source_provider_id, source_entity, external_id FROM sync_inventory_binding WHERE target_entity = $1 AND local_id = $2")
                .bind(entity.target_key()).bind(local_id).fetch_optional(&self.pool).await?;
            row.map(|row| {
                let source: String = row.try_get("source_entity")?;
                if source != entity.source_key() {
                    return Err(AppError::Conflict(
                        "Inventory binding has a noncanonical source entity".into(),
                    ));
                }
                Ok(InventoryBinding {
                    identity: InventoryIdentity {
                        provider_id: row.try_get("source_provider_id")?,
                        entity,
                        external_id: row.try_get("external_id")?,
                    },
                    local_id,
                })
            })
            .transpose()
        })
    }

    fn apply_live_record<'a>(
        &'a self,
        config: &'a MappingConfigDocument,
        record: &'a SourceRecord,
        authorisation: std::sync::Arc<dyn InventoryCommitAuthorisation>,
    ) -> futures_util::future::BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            let snapshot = SourceSnapshot {
                coverage: None,
                records: vec![record.clone()],
            };
            let assessment = assess_live_preparation(config, &snapshot);
            let batch = assessment.batch.ok_or_else(|| {
                blocked_assessment_error(
                    "inventory_preflight:preparation_blocked",
                    &assessment.findings,
                    assessment.analysis_complete,
                )
            })?;
            let jobs =
                crate::contexts::sync::infrastructure::persistence::SqlxSyncJobRepository::new(
                    self.pool.clone(),
                );
            let job =
                crate::contexts::sync::infrastructure::persistence::SyncJobRepository::create(
                    &jobs,
                    SyncJob {
                        id: Uuid::now_v7(),
                        provider_id: config.source_provider_id,
                        mapping_version: config.reference.to_wire(),
                        status: SyncStatus::Running,
                        started_at: Some(chrono::Utc::now()),
                        finished_at: None,
                        stats: serde_json::json!({}),
                    },
                )
                .await?;
            let job_row = job.clone();
            let result = self
                .commit_pull(CommitPull {
                    job,
                    batch,
                    authorisation,
                })
                .await;
            finish_job_run(&jobs, job_row, result, "LIVE_RECORD_FAILED").await?;
            Ok(())
        })
    }

    fn delete_live_record<'a>(
        &'a self,
        config: &'a MappingConfigDocument,
        identity: &'a InventoryIdentity,
        authorisation: std::sync::Arc<dyn InventoryCommitAuthorisation>,
    ) -> futures_util::future::BoxFuture<'a, Result<bool, AppError>> {
        Box::pin(async move {
            let Some(table) = identity.entity.table() else {
                return Err(AppError::Validation(format!(
                    "source entity '{}' has no deletable local row",
                    identity.entity.source_key()
                )));
            };
            let mut transaction =
                begin_provider_transaction(&self.pool, identity.provider_id).await?;
            authorisation.ensure_owned().await?;
            let Some(local_id) = find_binding(&mut transaction, identity).await? else {
                return Ok(false);
            };
            crate::contexts::inventory::infrastructure::persistence::repositories::delete_guard::delete_inventory_record_in_transaction(
                &mut transaction, table, local_id).await?;
            remove_binding(
                &mut transaction,
                &InventoryBinding {
                    identity: identity.clone(),
                    local_id,
                },
            )
            .await?;
            let now = chrono::Utc::now();
            let job = SyncJob {
                id: Uuid::now_v7(),
                provider_id: identity.provider_id,
                mapping_version: config.reference.to_wire(),
                status: SyncStatus::Completed,
                started_at: Some(now),
                finished_at: Some(now),
                stats: serde_json::json!({"deleted_count": 1}),
            };
            sqlx::query("INSERT INTO sync_job(id, provider_id, mapping_version, status, started_at, stats) VALUES ($1,$2,$3,'running',$4,'{}'::jsonb)")
                .bind(job.id).bind(job.provider_id).bind(&job.mapping_version).bind(job.started_at)
                .execute(&mut *transaction).await?;
            update_job(&mut transaction, &job).await?;
            authorisation.ensure_owned().await?;
            transaction.commit().await?;
            Ok(true)
        })
    }
}

/// Persist the outcome of one live record job whose write already ran.
///
/// A durable completion wins over a lost commit response; any other failure is
/// recorded so the job never stays `running`.
async fn finish_job_run(
    jobs: &crate::contexts::sync::infrastructure::persistence::SqlxSyncJobRepository,
    mut job: SyncJob,
    result: Result<SyncJob, AppError>,
    error_code: &str,
) -> Result<SyncJob, AppError> {
    use crate::contexts::sync::infrastructure::persistence::SyncJobRepository;
    match result {
        Ok(completed) => Ok(completed),
        Err(error) => {
            if let Some(persisted) = jobs.get_by_id(job.id).await?
                && persisted.status == SyncStatus::Completed
            {
                return Ok(persisted);
            }
            job.status = SyncStatus::Failed;
            job.finished_at = Some(chrono::Utc::now());
            job.stats = serde_json::json!({"failed_count": 1, "error_code": error_code});
            let _ = jobs.fail_if_running(&job).await?;
            Err(error)
        }
    }
}

pub(crate) async fn begin_provider_transaction(
    pool: &PgPool,
    provider: Uuid,
) -> Result<Transaction<'_, Postgres>, AppError> {
    begin_provider_transaction_with_mode(pool, provider, false).await
}

async fn begin_provider_transaction_with_mode(
    pool: &PgPool,
    provider: Uuid,
    bounded: bool,
) -> Result<Transaction<'_, Postgres>, AppError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *transaction)
        .await?;
    if bounded {
        sqlx::query("SET LOCAL lock_timeout = '250ms'")
            .execute(&mut *transaction)
            .await?;
        let acquired: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(provider.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        if !acquired {
            return Err(AppError::Conflict("inventory_transfer:busy".into()));
        }
    } else {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(provider.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    Ok(transaction)
}

pub(crate) async fn find_binding(
    transaction: &mut Transaction<'_, Postgres>,
    identity: &InventoryIdentity,
) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query("SELECT local_id FROM sync_inventory_binding WHERE source_provider_id = $1 AND source_entity = $2 AND external_id = $3 AND target_entity = $4")
        .bind(identity.provider_id).bind(identity.entity.source_key()).bind(&identity.external_id)
        .bind(identity.entity.target_key()).fetch_optional(&mut **transaction).await?;
    row.map(|row| row.try_get("local_id").map_err(AppError::from))
        .transpose()
}

pub(crate) async fn write_binding(
    transaction: &mut Transaction<'_, Postgres>,
    binding: &InventoryBinding,
    job: Option<Uuid>,
    source_timestamps: Option<SourceTimestamps>,
) -> Result<(), AppError> {
    let identity = &binding.identity;
    let timestamps = source_timestamps.unwrap_or_default();
    let mut query = QueryBuilder::<Postgres>::new(
        "INSERT INTO sync_inventory_binding (source_provider_id, source_entity, external_id, target_entity, local_id, last_seen_job_id",
    );
    if timestamps.created.is_some() {
        query.push(", source_created_at");
    }
    if timestamps.updated.is_some() {
        query.push(", source_updated_at");
    }
    query.push(") VALUES (");
    query.push_bind(identity.provider_id);
    query.push(", ").push_bind(identity.entity.source_key());
    query.push(", ").push_bind(&identity.external_id);
    query.push(", ").push_bind(identity.entity.target_key());
    query.push(", ").push_bind(binding.local_id);
    query.push(", ").push_bind(job);
    if let Some(value) = timestamps.created {
        query.push(", ").push_bind(value);
    }
    if let Some(value) = timestamps.updated {
        query.push(", ").push_bind(value);
    }
    query.push(") ON CONFLICT (source_provider_id, source_entity, external_id, target_entity) DO UPDATE SET last_seen_job_id = COALESCE(EXCLUDED.last_seen_job_id, sync_inventory_binding.last_seen_job_id), updated_at = now()");
    if timestamps.created.is_some() {
        query.push(", source_created_at = EXCLUDED.source_created_at");
    }
    if timestamps.updated.is_some() {
        query.push(", source_updated_at = EXCLUDED.source_updated_at");
    }
    query.push(" WHERE sync_inventory_binding.local_id = EXCLUDED.local_id");
    let result = query.build().execute(&mut **transaction).await?;
    if result.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "Inventory identity cannot be re-keyed".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn remove_binding(
    transaction: &mut Transaction<'_, Postgres>,
    binding: &InventoryBinding,
) -> Result<(), AppError> {
    let identity = &binding.identity;
    sqlx::query("DELETE FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity=$2 AND external_id=$3 AND target_entity=$4 AND local_id=$5")
        .bind(identity.provider_id).bind(identity.entity.source_key()).bind(&identity.external_id)
        .bind(identity.entity.target_key()).bind(binding.local_id).execute(&mut **transaction).await?;
    Ok(())
}

/// Derive the deterministic local id for one source record. The tuple is joined
/// with U+001F (unit separator), which cannot appear in a canonical source key
/// or a provider id, so the encoding is unambiguous.
#[must_use]
fn derive_local_id(
    source_provider_id: Uuid,
    source_entity: &str,
    external_id: &str,
    target_key: &str,
) -> Uuid {
    let entity = InventoryEntity::from_source_key(source_entity).expect("checked source entity");
    debug_assert_eq!(target_key, entity.target_key());
    InventoryIdentity {
        provider_id: source_provider_id,
        entity,
        external_id: external_id.into(),
    }
    .local_id()
}

fn external_id_of(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map.get("id").and_then(scalar_to_string),
        Value::Number(_) | Value::String(_) | Value::Bool(_) => scalar_to_string(value),
        _ => None,
    }
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn tag_references(
    value: &Value,
    provider_id: Uuid,
) -> Result<Vec<crate::contexts::sync::domain::SourceIdentity>, AppError> {
    let items = value
        .as_array()
        .ok_or_else(|| AppError::Validation("tags is not a list".into()))?;
    let mut seen = HashSet::new();
    items
        .iter()
        .map(|item| {
            let external_id = tag_memberships::embedded_id(item)
                .ok_or_else(|| AppError::Validation("a tag has no provider identity".into()))?;
            if !seen.insert(external_id.clone()) {
                return Err(AppError::Validation(
                    "duplicate tag provider identity".into(),
                ));
            }
            Ok(crate::contexts::sync::domain::SourceIdentity {
                source_provider_id: provider_id,
                entity: SourceEntity::new(InventoryEntity::Tag.source_key())?,
                external_id,
            })
        })
        .collect()
}

/// Normalise a NetBox choice object to its `.value`; a non-object is returned
/// unchanged.
fn normalize_choice(value: &Value) -> Result<Value, AppError> {
    if let Value::Object(map) = value {
        match map.get("value") {
            Some(inner) => Ok(inner.clone()),
            None => Err(AppError::Validation(
                "choice object has no value".to_string(),
            )),
        }
    } else {
        Ok(value.clone())
    }
}

fn wrong_shape(spec: InventoryField) -> AppError {
    AppError::Validation(format!(
        "field {}::{} has a value of the wrong shape",
        spec.entity().source_key(),
        spec.name()
    ))
}

fn convert_scalar(spec: InventoryField, value: &Value) -> Result<InventoryValue, AppError> {
    use crate::contexts::inventory::domain::dcim::cooling::CoolingMeasurement;
    use crate::contexts::inventory::domain::dcim::interface::InterfaceRfMeasurement;

    // Inspect the original JSON token/string before as_f64 can erase a tail.
    if spec.entity().is_hardware() {
        let measurement = match spec.name() {
            "cooling_capacity" => Some(CoolingMeasurement::Capacity),
            "diameter" => Some(CoolingMeasurement::Diameter),
            "max_flow" => Some(CoolingMeasurement::MaxFlow),
            _ => None,
        };
        if let Some(measurement) = measurement {
            return Ok(InventoryValue::Number {
                value: measurement.validate_source(value)?,
            });
        }
    }
    if spec.entity() == InventoryEntity::Interface {
        let measurement = match spec.name() {
            "rf_channel_frequency" => Some(InterfaceRfMeasurement::Frequency),
            "rf_channel_width" => Some(InterfaceRfMeasurement::Width),
            _ => None,
        };
        if let Some(measurement) = measurement {
            return Ok(InventoryValue::Number {
                value: measurement.validate_source(value)?,
            });
        }
    }
    let metadata = spec.spec();
    let value = if metadata.choice_normalized {
        normalize_choice(value)?
    } else {
        value.clone()
    };
    let converted = match metadata.sql_type {
        PgType::Text | PgType::Inet | PgType::Cidr | PgType::MacAddr => InventoryValue::Text {
            value: value.as_str().ok_or_else(|| wrong_shape(spec))?.to_string(),
        },
        PgType::Bool => InventoryValue::Boolean {
            value: value.as_bool().ok_or_else(|| wrong_shape(spec))?,
        },
        PgType::Integer => {
            let number = value.as_i64().ok_or_else(|| wrong_shape(spec))?;
            let narrowed = i32::try_from(number).map_err(|_| {
                AppError::Validation(format!(
                    "field {}::{} exceeds the integer range",
                    spec.entity().source_key(),
                    spec.name()
                ))
            })?;
            InventoryValue::Integer {
                value: i64::from(narrowed),
            }
        }
        PgType::BigInt | PgType::SmallInt => InventoryValue::Integer {
            value: value.as_i64().ok_or_else(|| wrong_shape(spec))?,
        },
        PgType::Numeric => {
            // DjangoJSONEncoder represents ORM Decimal values as strings. Keep
            // their exact decimal token through the numeric SQL bind; f64 would
            // both reject that source shape and round sufficiently long values.
            let decimal = match &value {
                Value::Number(_) => value.clone(),
                Value::String(raw) if raw.trim() == raw => {
                    match serde_json::from_str::<Value>(raw) {
                        Ok(number @ Value::Number(_)) => number,
                        _ => return Err(wrong_shape(spec)),
                    }
                }
                _ => return Err(wrong_shape(spec)),
            };
            InventoryValue::Json { value: decimal }
        }
        PgType::Double | PgType::Real => InventoryValue::Number {
            value: value.as_f64().ok_or_else(|| wrong_shape(spec))?,
        },
        PgType::Uuid => {
            let raw = value.as_str().ok_or_else(|| wrong_shape(spec))?;
            let parsed = Uuid::parse_str(raw).map_err(|_| wrong_shape(spec))?;
            InventoryValue::Text {
                value: parsed.to_string(),
            }
        }
        PgType::TextArray => {
            let strings = value
                .as_array()
                .ok_or_else(|| wrong_shape(spec))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| wrong_shape(spec))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let strings =
                if spec.entity() == InventoryEntity::Service && spec.name() == "port_mappings" {
                    validate_port_mappings(&strings)?
                } else {
                    strings
                };
            InventoryValue::Json {
                value: serde_json::json!(strings),
            }
        }
        PgType::Jsonb
        | PgType::Json
        | PgType::SmallIntArray
        | PgType::IntArray
        | PgType::UuidArray
        | PgType::BigIntArray => InventoryValue::Json { value },
        PgType::Date => {
            let raw = value.as_str().ok_or_else(|| wrong_shape(spec))?;
            let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .map_err(|_| wrong_shape(spec))?;
            if date.format("%Y-%m-%d").to_string() != raw {
                return Err(wrong_shape(spec));
            }
            InventoryValue::Text {
                value: raw.to_owned(),
            }
        }
        PgType::Time | PgType::Timestamp | PgType::TimestampTz => {
            return Err(AppError::Validation(format!(
                "field {}::{} has no supported temporal conversion",
                spec.entity().source_key(),
                spec.name()
            )));
        }
    };
    if let InventoryValue::Text { value } = &converted {
        use crate::contexts::inventory::domain::dcim::cooling::{
            DIAMETER_UNITS, FLOW_RATE_UNITS, validate_cooling_method, validate_rack_cooling,
        };
        match spec.name() {
            "cooling_method" => validate_cooling_method(Some(value))?,
            "cooling_capability" => validate_rack_cooling(Some(value), None)?,
            "diameter_unit" if !DIAMETER_UNITS.contains(&value.as_str()) => {
                return Err(wrong_shape(spec));
            }
            "max_flow_unit" if !FLOW_RATE_UNITS.contains(&value.as_str()) => {
                return Err(wrong_shape(spec));
            }
            _ => {}
        }
    }
    if !interface_graph::valid_scalar(spec, &converted) {
        return Err(wrong_shape(spec));
    }
    Ok(converted)
}

fn record_value<'a>(record: &'a SourceRecord, name: &str) -> Option<&'a Value> {
    record.fields.get(name)
}

/// Ordinary pulls retain the same privacy-safe findings as assessment callers.
/// Only typed findings cross this boundary, never nested validator error bodies.
fn blocked_assessment_error(
    code: &'static str,
    findings: &[Finding],
    analysis_complete: bool,
) -> AppError {
    AppError::Validation(
        serde_json::json!({
            "code": code,
            "analysis_complete": analysis_complete,
            "findings": findings,
        })
        .to_string(),
    )
}

impl LocalInventoryTarget for LocalInventoryCatalog {
    type Batch = LocalInventoryBatch;

    fn prepare(
        &self,
        config: &MappingConfigDocument,
        snapshot: &SourceSnapshot,
    ) -> Result<Self::Batch, AppError> {
        let assessment = assess_preparation(config, snapshot);
        assessment.batch.ok_or_else(|| {
            blocked_assessment_error(
                "inventory_preflight:preparation_blocked",
                &assessment.findings,
                assessment.analysis_complete,
            )
        })
    }

    async fn commit_pull(&self, commit: CommitPull<Self::Batch>) -> Result<SyncJob, AppError> {
        let CommitPull {
            mut job,
            batch,
            authorisation,
        } = commit;
        let mut transaction =
            begin_provider_transaction(&self.pool, batch.source_provider_id).await?;

        authorisation.ensure_owned().await?;
        if let Some(generation) = &batch.generation {
            if job.provider_id != generation.binding.provider_id
                || crate::contexts::sync::domain::MappingRef::parse(&job.mapping_version)?
                    != generation.mapping
            {
                return Err(AppError::Validation(
                    "sync.source_generation_job_mismatch".into(),
                ));
            }
            self.revalidate_generation(&mut transaction, generation)
                .await?;
        }
        if batch
            .rows
            .iter()
            .any(|row| row.entity == InventoryEntity::Tag)
        {
            lock_tag_catalog_for_definition(&mut transaction).await?;
        } else {
            lock_tag_catalog_for_usage(&mut transaction).await?;
        }
        // Planning may lock hardware rows and observe selected Cable sides.
        // Match native Tag -> graph -> resource ordering before either occurs;
        // retain the graph lock through commit, including omitted-side movers
        // and endpoints not yet attached to a selected Cable at observation time.
        if batch.source_snapshot
            || batch
                .rows
                .iter()
                .any(|row| row.entity.uses_hardware_graph())
        {
            lock_hardware_graph(&mut transaction).await?;
        }
        if batch.source_snapshot {
            transfer_fence::lock_observation_tables(&mut transaction).await?;
            ownership::lock_proofs(&mut transaction, &batch.rows).await?;
        }
        let analysis = plan_prewrite(&batch, &mut transaction).await?;
        let mut planned = analysis.planned.ok_or_else(|| {
            blocked_assessment_error(
                "inventory_preflight:target_graph_blocked",
                &analysis.findings,
                analysis.complete,
            )
        })?;
        let source_selected = if batch.source_snapshot {
            let source_selected = planned.clone();
            ownership::retain_memberships(&mut transaction, &mut planned).await?;
            Some(source_selected)
        } else {
            None
        };
        let mutation_rows: Vec<_> = planned
            .iter()
            .filter(|row| {
                template_validation::kind(row.entity).is_some()
                    || row.entity == InventoryEntity::PortTemplateMapping
                    || template_validation::admitted_native_mutation(row.entity)
            })
            .cloned()
            .collect();
        let (_, changed_mutations) =
            transfer_change_stats(&mut transaction, &mutation_rows, 0).await?;
        template_validation::authorize_mutations(
            &mut transaction,
            &planned,
            &changed_mutations,
            authorisation.as_ref(),
        )
        .await?;
        template_validation::authorize_original_admitted_references(
            &mut transaction,
            &planned,
            authorisation.as_ref(),
        )
        .await?;
        let admitted_mutations = template_validation::authorize_admitted_mutations(
            &mut transaction,
            &planned,
            &changed_mutations,
            authorisation.as_ref(),
        )
        .await?;
        ensure_tag_scope_plan(&batch, &planned, &mut transaction).await?;
        if job.provider_id != batch.source_provider_id {
            return Err(AppError::Conflict(
                "inventory_ownership:provider_mismatch".into(),
            ));
        }
        let ownership_context = if batch.source_snapshot {
            let mapping = crate::contexts::sync::domain::MappingRef::parse(&job.mapping_version)?;
            let observations =
                ownership::observe(&mut transaction, batch.source_provider_id, &planned).await?;
            let junctions = ownership::observe_junctions(
                &mut transaction,
                batch.source_provider_id,
                source_selected.as_deref().expect("snapshot selection"),
            )
            .await?;
            let tags = ownership::observe_tags(
                &mut transaction,
                batch.source_provider_id,
                source_selected.as_deref().expect("snapshot selection"),
            )
            .await?;
            let cables = ownership::observe_cables(
                &mut transaction,
                batch.source_provider_id,
                source_selected.as_deref().expect("snapshot selection"),
            )
            .await?;
            Some((mapping, observations, junctions, tags, cables))
        } else {
            None
        };
        let mut stats = InventoryCommitStats {
            extracted: batch.extracted_count(),
            ..InventoryCommitStats::default()
        };

        let counter_device_owners = counters::device_owners(&mut transaction, &planned).await?;
        commit_planned_rows(
            &mut transaction,
            batch.source_provider_id,
            job.id,
            &planned,
            &mut stats,
            None,
        )
        .await?;

        // Rebuild the nested-set bookkeeping of every touched MPTT table from
        // the whole parent graph, so hierarchy filters stay valid.
        rebuild_touched_trees(&mut transaction, &planned).await?;
        template_validation::validate(&mut transaction, &planned, authorisation.as_ref()).await?;
        template_validation::authorize_final_admitted_references(
            &mut transaction,
            &planned,
            authorisation.as_ref(),
        )
        .await?;
        template_validation::authorize_final_admitted_mutations(
            &mut transaction,
            &planned,
            admitted_mutations,
            authorisation.as_ref(),
        )
        .await?;

        recompute_counts(&mut transaction, &planned, &counter_device_owners).await?;
        if let Some((mapping, observations, junctions, tags, cables)) = ownership_context {
            ownership::attest(&mut transaction, job.id, mapping, observations).await?;
            ownership::attest_junctions(&mut transaction, job.id, mapping, junctions).await?;
            ownership::attest_tags(&mut transaction, job.id, mapping, tags).await?;
            ownership::attest_cables(&mut transaction, job.id, mapping, cables).await?;
        }

        // Reconcile the complete observation against the units this source
        // created. A destructive pass that cannot finish safely refuses the whole
        // pull: the rollback below covers inventory, bindings, proofs, absence
        // counters and job completion alike.
        if let (Some(generation), Some(selected)) =
            (batch.generation.as_ref(), source_selected.as_deref())
        {
            let pass = reconciliation::Pass {
                job_id: job.id,
                generation: &generation.generation,
                policy: generation.policy,
                provider_id: batch.source_provider_id,
                completed_entities: generation.coverage.completed_entities(),
                source_selected: selected,
                authorisation: authorisation.as_ref(),
            };
            match reconciliation::reconcile(&mut transaction, &pass).await {
                Ok(report) => {
                    let mut step = SyncJobStep::new(
                        job.id,
                        "reconciliation".to_string(),
                        serde_json::to_value(&report).map_err(|error| {
                            AppError::Internal(format!(
                                "Failed to encode the reconciliation report: {error}"
                            ))
                        })?,
                    );
                    step.complete();
                    sync_job_repository::write_step(&mut transaction, &step).await?;
                    stats.reconciliation = Some(report);
                }
                Err(refusal) => {
                    transaction.rollback().await.map_err(|error| {
                        AppError::Database(format!(
                            "Failed to roll back a refused reconciliation: {error}"
                        ))
                    })?;
                    let mut failed = job.clone();
                    failed.fail();
                    let mut failed_stats = stats.clone();
                    failed_stats.reconciliation = Some(refusal.report.clone());
                    failed.stats = serde_json::to_value(&failed_stats).map_err(|error| {
                        AppError::Internal(format!("Failed to encode sync stats: {error}"))
                    })?;
                    let mut report_transaction = self.pool.begin().await.map_err(|error| {
                        AppError::Database(format!(
                            "Failed to open the reconciliation report transaction: {error}"
                        ))
                    })?;
                    update_job(&mut report_transaction, &failed).await?;
                    let mut step = SyncJobStep::new(
                        job.id,
                        "reconciliation".to_string(),
                        serde_json::to_value(&refusal.report).map_err(|error| {
                            AppError::Internal(format!(
                                "Failed to encode the reconciliation report: {error}"
                            ))
                        })?,
                    );
                    step.fail();
                    sync_job_repository::write_step(&mut report_transaction, &step).await?;
                    report_transaction.commit().await.map_err(|error| {
                        AppError::Database(format!(
                            "Failed to commit the reconciliation report: {error}"
                        ))
                    })?;
                    return Err(refusal.error);
                }
            }
        }

        job.complete();
        stats.tag_memberships_committed = batch.tag_membership_count;
        stats.entities_extracted = batch.entities_extracted;
        stats.source_capability = batch
            .coverage
            .as_ref()
            .and_then(|coverage| coverage.capability())
            .map(str::to_owned);
        stats.entities_committed.insert(
            InventoryEntity::TaggedObject.source_key().into(),
            batch.tag_membership_count,
        );
        job.stats = serde_json::to_value(&stats)
            .map_err(|error| AppError::Internal(format!("Failed to encode sync stats: {error}")))?;
        let updated = update_job(&mut transaction, &job).await?;

        authorisation.ensure_owned().await?;
        if let Some(generation) = &batch.generation {
            self.revalidate_generation(&mut transaction, generation)
                .await?;
        }
        authorisation.ensure_owned().await?;
        transaction.commit().await.map_err(|error| {
            AppError::Database(format!("Failed to commit the sync transaction: {error}"))
        })?;
        Ok(updated)
    }
}

/// Pure shared validation entrypoint. It requires no pool or service context.
/// Independent rows/fields are inspected even after blockers. Only an entirely
/// valid assessment can yield a batch accepted by the existing commit owner.
pub fn assess_preparation(
    config: &MappingConfigDocument,
    snapshot: &SourceSnapshot,
) -> PreparationAssessment<LocalInventoryBatch> {
    assess_preparation_with_evidence(config, snapshot, None, SyncScope::Snapshot)
}

/// Plan one record delivered by a provider's live event stream.
///
/// The batch covers exactly that record, so nothing absent from it is
/// withdrawn; the live scope additionally accepts the bidirectional mapping
/// that carries the inbound stream.
pub fn assess_live_preparation(
    config: &MappingConfigDocument,
    snapshot: &SourceSnapshot,
) -> PreparationAssessment<LocalInventoryBatch> {
    assess_preparation_with_evidence(config, snapshot, None, SyncScope::LiveRecord)
}

/// Prepare an immutable source using its verified, exact-snapshot relationship
/// census. Ordinary snapshots keep the independent-membership requirement.
pub fn assess_preparation_from_source(
    config: &MappingConfigDocument,
    source: &ImmutableInventorySource,
) -> PreparationAssessment<LocalInventoryBatch> {
    assess_preparation_with_evidence(
        config,
        &source.snapshot(),
        source.verified_tag_relationships(),
        SyncScope::Snapshot,
    )
}

fn assess_preparation_with_evidence(
    config: &MappingConfigDocument,
    snapshot: &SourceSnapshot,
    tag_evidence: Option<&VerifiedTagRelationships>,
    scope: SyncScope,
) -> PreparationAssessment<LocalInventoryBatch> {
    let mut findings = Vec::new();
    let mut analysis_complete = true;
    if config
        .target_sync_request()
        .ensure_supported_for(scope)
        .is_err()
    {
        findings.push(Finding::blocker(
            FindingCode::RequestUnsupported,
            Capability::MappingValidation,
        ));
    }
    let mappings = enabled_entity_mappings(config, &mut findings);
    let fields_by_mapping = field_mappings_by_entity(config);
    let mut rows = Vec::new();
    let mut facts = Vec::new();
    let mut identity_counts = HashMap::new();
    for record in &snapshot.records {
        *identity_counts
            .entry((
                record.identity.source_provider_id,
                record.identity.entity.clone(),
                record.identity.external_id.clone(),
            ))
            .or_insert(0usize) += 1;
    }
    let mut seen = HashSet::new();
    for record in &snapshot.records {
        let before = findings.len();
        let checked = check_record_identity(record, config.source_provider_id, &mut seen);
        let identity_known = checked.is_ok()
            && identity_counts.get(&(
                record.identity.source_provider_id,
                record.identity.entity.clone(),
                record.identity.external_id.clone(),
            )) == Some(&1);
        let Some(entity) = InventoryEntity::from_source_key(record.identity.entity.as_str()) else {
            findings.push(
                Finding::blocker(FindingCode::ModelUnsupported, Capability::MappingValidation)
                    .for_record(record),
            );
            analysis_complete = false;
            continue;
        };
        if checked.is_err() {
            findings.push(
                Finding::blocker(
                    FindingCode::RecordIdentityInvalid,
                    Capability::MappingValidation,
                )
                .for_record(record),
            );
        }
        // ORM snapshots carry authoritative mapping rows: all six relationship
        // values are required on each one. Absence of a mapping record is not a
        // request to clear a retained mapping.
        if entity == InventoryEntity::PortTemplateMapping {
            for field in [
                "front_port_position",
                "rear_port_position",
                "front_port",
                "rear_port",
                "device_type",
                "module_type",
            ] {
                if !record.fields.contains_key(field) {
                    let mut finding = Finding::blocker(
                        FindingCode::FieldValueInvalid,
                        Capability::MappingValidation,
                    )
                    .for_record(record);
                    finding.field_path = Some(field.into());
                    findings.push(finding);
                }
            }
        }
        for (name, value) in &record.fields {
            if source_value_populated(value)
                && entity.field(name).is_none()
                && !entity
                    .source_field_disposition(name)
                    .is_some_and(|d| d.permits_populated())
            {
                let mut finding =
                    Finding::blocker(FindingCode::FieldUnsupported, Capability::MappingValidation)
                        .for_record(record);
                finding.field_path = Some(if entity.source_field_disposition(name).is_some() {
                    name.clone()
                } else {
                    format!("sha256:{}", digest(name.as_bytes()))
                });
                findings.push(finding);
            }
        }
        if entity.is_membership() {
            continue;
        }
        if !mappings.contains_key(record.identity.entity.as_str()) {
            findings.push(
                Finding::blocker(FindingCode::MappingMissing, Capability::MappingValidation)
                    .for_record(record),
            );
            analysis_complete = false;
            if identity_known {
                facts.push(preparation_facts(
                    entity,
                    record,
                    &PreparedColumns::default(),
                    &HashSet::new(),
                    &HashSet::new(),
                ));
            }
            continue;
        }
        let fields: Vec<_> = config
            .entity_mappings
            .iter()
            .filter(|m| {
                m.enabled
                    && m.source_entity == entity.source_key()
                    && m.target_entity == entity.target_key()
            })
            .flat_map(|mapping| {
                fields_by_mapping
                    .get(&mapping.id)
                    .into_iter()
                    .flatten()
                    .copied()
            })
            .collect();
        if entity == InventoryEntity::PortTemplateMapping {
            for name in [
                "front_port_position",
                "rear_port_position",
                "front_port",
                "rear_port",
                "device_type",
                "module_type",
            ] {
                if let Some(mapping) = fields.iter().find(|mapping| mapping.target_field == name) {
                    let identity = serde_json::json!({"op": "identity", "args": [{"field": name}]});
                    if mapping.source_field != name || mapping.forward_transform_json != identity {
                        let mut finding = Finding::blocker(
                            FindingCode::MappingInvalid,
                            Capability::MappingValidation,
                        )
                        .for_record(record);
                        finding.field_path = Some(name.into());
                        findings.push(finding);
                    }
                }
            }
        }
        for field in entity.field_iter() {
            if (record.fields.contains_key(field.name()) || field.absent() == Absent::Reject)
                && !fields
                    .iter()
                    .any(|mapping| mapping.target_field == field.name())
            {
                let mut finding = Finding::blocker(
                    FindingCode::FieldMappingMissing,
                    Capability::MappingValidation,
                )
                .for_record(record);
                finding.field_path = Some(field.name().into());
                findings.push(finding);
            }
        }
        let source_timestamps = match SourceTimestamps::from_record(record) {
            Ok(timestamps) => timestamps,
            Err(fields) => {
                for field in fields {
                    let mut finding = Finding::blocker(
                        FindingCode::FieldValueInvalid,
                        Capability::MappingValidation,
                    )
                    .for_record(record);
                    finding.field_path = Some(field.into());
                    findings.push(finding);
                }
                SourceTimestamps::default()
            }
        };
        let definition = if entity == InventoryEntity::CustomField {
            match validate_source_custom_field(record) {
                Ok(definition) => Some(definition),
                Err(_) => {
                    let mut finding = Finding::blocker(
                        FindingCode::DefinitionInvalid,
                        Capability::MappingValidation,
                    )
                    .for_record(record);
                    finding.field_path = Some("definition".into());
                    findings.push(finding);
                    analysis_complete = false;
                    None
                }
            }
        } else {
            None
        };
        let mut columns = PreparedColumns::default();
        let sample = Value::Object(record.fields.clone());
        let mut mapped = HashSet::new();
        let mut ambiguous = HashSet::new();
        let mut failed = HashSet::new();
        for field in fields {
            if !mapped.insert(field.target_field.as_str()) {
                ambiguous.insert(field.target_field.as_str());
                let mut finding =
                    Finding::blocker(FindingCode::MappingInvalid, Capability::MappingValidation)
                        .for_record(record);
                finding.field_path = Some(format!(
                    "mapping.fields.sha256:{}",
                    digest(field.target_field.as_bytes())
                ));
                findings.push(finding);
            }
            let mut field_columns = PreparedColumns::default();
            if let Err((code, _)) = bind_mapping(entity, record, field, &sample, &mut field_columns)
            {
                failed.insert(field.target_field.as_str());
                let mut finding =
                    Finding::blocker(code, Capability::MappingValidation).for_record(record);
                finding.field_path = Some(
                    if entity.field(&field.target_field).is_some()
                        || field.target_field == "custom_fields"
                    {
                        field.target_field.clone()
                    } else {
                        format!("sha256:{}", digest(field.target_field.as_bytes()))
                    },
                );
                findings.push(finding);
            } else {
                columns.append(field_columns);
            }
        }
        if columns.custom_fields.is_none() {
            match custom_fields_from_record(record) {
                Ok(value) => columns.custom_fields = value,
                Err(_) => {
                    let mut finding = Finding::blocker(
                        FindingCode::FieldValueInvalid,
                        Capability::MappingValidation,
                    )
                    .for_record(record);
                    finding.field_path = Some("custom_fields".into());
                    findings.push(finding);
                }
            }
        }
        if identity_known {
            facts.push(preparation_facts(
                entity, record, &columns, &ambiguous, &failed,
            ));
        } else if entity == InventoryEntity::Tag
            || record.fields.contains_key("tags")
            || columns.tags.is_some()
            || failed.contains("tags")
        {
            let mut finding = Finding::blocker(
                FindingCode::AnalysisDependencyUnavailable,
                Capability::MappingValidation,
            )
            .for_record(record);
            finding.field_path = Some("tags.graph.identity".into());
            findings.push(finding);
            analysis_complete = false;
        }
        if findings.len() == before {
            match prepared_row(
                entity,
                config.source_provider_id,
                record,
                columns,
                definition,
                source_timestamps,
            ) {
                Ok(row) => rows.push(row),
                Err(_) => {
                    let mut finding = Finding::blocker(
                        FindingCode::FieldValueInvalid,
                        Capability::MappingValidation,
                    )
                    .for_record(record);
                    finding.field_path = Some("custom_fields".into());
                    findings.push(finding);
                }
            }
        }
    }
    let mut definition_owners = BTreeMap::<_, Vec<_>>::new();
    for fact in &facts {
        if let Some((field, value)) = &fact.definition_key {
            definition_owners
                .entry((fact.entity, *field, value.clone()))
                .or_default()
                .push(fact);
        }
    }
    for ((_, field, _), owners) in definition_owners
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
    {
        for fact in &owners {
            let mut finding = fact_finding(fact, FindingCode::DefinitionInvalid, field);
            finding.affected_count = owners.len() as u64;
            findings.push(finding);
        }
    }
    let (cache_findings, caches_complete) =
        source_cache_assertions::assess(snapshot, &rows, config.source_provider_id);
    findings.extend(cache_findings);
    analysis_complete &= caches_complete;
    let (tag_findings, tags_complete) = match tag_evidence {
        Some(evidence) => tag_memberships::assess_with_evidence(
            snapshot,
            &facts,
            config.source_provider_id,
            Some(evidence),
        ),
        None => tag_memberships::assess(snapshot, &facts, config.source_provider_id),
    };
    findings.extend(tag_findings);
    analysis_complete &= tags_complete;
    let (interface_findings, interfaces_complete, cable_sides) =
        interface_graph::assess(snapshot, config.source_provider_id, &mut rows);
    findings.extend(interface_findings);
    analysis_complete &= interfaces_complete;
    let generation = match snapshot.coverage.as_ref().and_then(|coverage| {
        coverage
            .generation_binding()
            .map(|evidence| (coverage, evidence))
    }) {
        Some((coverage, evidence))
            if evidence.binding().sync_mode.as_deref() == Some("pull_only") =>
        {
            use crate::contexts::sync::domain::inventory_sync::ConfiguredSourceGenerationAuthority;
            match coverage.capture(config, snapshot) {
                Ok(generation) => Some(ConfiguredGeneration {
                    generation,
                    coverage: coverage.clone(),
                    binding: evidence.binding().clone(),
                    mapping: config.reference,
                    mapping_digest: config.content_digest(),
                    policy: config.absence_policy,
                }),
                Err(_) => {
                    findings.push(Finding::blocker(
                        FindingCode::SourceAuthorityMismatch,
                        Capability::SourceCoverage,
                    ));
                    analysis_complete = false;
                    None
                }
            }
        }
        // Retain imports from other modes or unsealed sources make no generation claim.
        _ => None,
    };
    // Destructive policy needs a capturable configured generation: two completed
    // observations of the *same* sealed source. Without one, an "absence" would
    // describe a source this run cannot prove it observed, so nothing may be
    // withdrawn. A capture that failed is already reported as a mismatch, which
    // states the more precise cause and is not repeated as an unattested source.
    if config.absence_policy.removes_source_owned_units()
        && generation.is_none()
        && !findings
            .iter()
            .any(|finding| finding.code == FindingCode::SourceAuthorityMismatch)
    {
        findings.push(Finding::blocker(
            FindingCode::SourceAuthorityUnattested,
            Capability::SourceCoverage,
        ));
        analysis_complete = false;
    }
    findings.sort();
    // A mapped identity and raw capture may observe the same shape failure.
    findings.dedup();
    let batch = (!findings
        .iter()
        .any(|finding| finding.severity == Severity::Blocker))
    .then(|| LocalInventoryBatch {
        source_provider_id: config.source_provider_id,
        source_snapshot: matches!(scope, SyncScope::Snapshot),
        coverage: snapshot
            .coverage
            .as_ref()
            .filter(|coverage| {
                let completed: HashSet<_> = coverage
                    .completed_entities()
                    .iter()
                    .map(SourceEntity::as_str)
                    .collect();
                // The core catalogue and both membership owners stay mandatory.
                // Optional plugin entities may be present, and an unavailable
                // optional namespace is withheld rather than counted as absent.
                let required: HashSet<_> = InventoryEntity::core()
                    .chain([
                        InventoryEntity::TaggedObject,
                        InventoryEntity::CableTermination,
                    ])
                    .map(InventoryEntity::source_key)
                    .collect();
                required.iter().all(|entity| completed.contains(*entity))
                    && coverage.matches(snapshot, config)
            })
            .cloned(),
        mapping_digest: config.content_digest(),
        generation,
        rows,
        extracted_count: snapshot.records.len() as u64,
        tag_membership_count: count_tag_memberships(snapshot, tag_evidence),
        entities_extracted: count_entities_extracted(snapshot),
        cable_sides,
    });
    PreparationAssessment {
        batch,
        findings,
        analysis_complete,
    }
}

fn prepared_finding(
    row: &PreparedRow,
    code: FindingCode,
    field: Option<&str>,
    capability: Capability,
) -> Finding {
    let mut finding = Finding::blocker(code, capability);
    finding.entity = Some(row.source_entity.clone());
    finding.record_identity_sha256 = Some(digest(row.external_id.as_bytes()));
    finding.field_path = field.map(str::to_owned);
    finding
}

/// Collect the enabled entity mappings, keyed by their source entity key. A
/// disabled mapping is a whitelist miss, so an entity is pulled only through
/// its enabled mapping.
fn enabled_entity_mappings<'a>(
    config: &'a MappingConfigDocument,
    findings: &mut Vec<Finding>,
) -> HashMap<&'a str, &'a EntityMapping> {
    let mut mappings = HashMap::new();
    let mut ids = HashSet::new();
    for mapping in &config.entity_mappings {
        let entity = InventoryEntity::from_source_key(&mapping.source_entity);
        let duplicate_id = !ids.insert(mapping.id);
        let duplicate_source =
            mapping.enabled && mappings.contains_key(mapping.source_entity.as_str());
        let issues = [
            (duplicate_id, "mapping.id"),
            (
                mapping.mapping_config_logical_id != config.reference.logical_id
                    || mapping.mapping_config_version_id != config.reference.version_id,
                "mapping.reference",
            ),
            (mapping.enabled && entity.is_none(), "mapping.source_entity"),
            (
                mapping.enabled && entity.is_some_and(|e| mapping.target_entity != e.target_key()),
                "mapping.target_entity",
            ),
            (duplicate_source, "mapping.source_entity"),
        ];
        for (invalid, field) in issues {
            if invalid {
                let mut finding =
                    Finding::blocker(FindingCode::MappingInvalid, Capability::MappingValidation);
                finding.entity = entity.map(|e| e.source_key().into());
                finding.record_identity_sha256 = Some(digest(mapping.id.as_bytes()));
                finding.field_path = Some(field.into());
                findings.push(finding);
            }
        }
        if mapping.enabled
            && !duplicate_source
            && entity.is_some_and(|e| mapping.target_entity == e.target_key())
        {
            mappings.insert(mapping.source_entity.as_str(), mapping);
        }
    }
    let mut field_ids = HashSet::new();
    let mut targets = HashSet::new();
    for field in &config.field_mappings {
        let owner = config
            .entity_mappings
            .iter()
            .find(|mapping| mapping.id == field.entity_mapping_id);
        let entity =
            owner.and_then(|mapping| InventoryEntity::from_source_key(&mapping.source_entity));
        let expressions = std::iter::once(("forward_transform", &field.forward_transform_json))
            .chain(
                field
                    .reverse_transform_json
                    .as_ref()
                    .map(|expression| ("reverse_transform", expression)),
            );
        for (slot, expression) in expressions {
            let mut expression_issues = Vec::new();
            crate::contexts::sync::application::mapping_validator::validate_transform_expression(
                "expression",
                expression,
                &mut expression_issues,
                true,
            );
            for issue in expression_issues {
                let mut finding = Finding::blocker(
                    FindingCode::ExpressionInvalid,
                    Capability::MappingValidation,
                );
                finding.entity = entity.map(|entity| entity.source_key().into());
                finding.record_identity_sha256 = Some(digest(field.id.as_bytes()));
                finding.field_path = Some(format!("mapping.{slot}.{}", issue.path));
                findings.push(finding);
            }
        }
        let invalid_target = owner.is_some_and(|mapping| mapping.enabled)
            && entity.is_some_and(|entity| {
                field.target_field != "custom_fields" && entity.field(&field.target_field).is_none()
            });
        let duplicate_id = !field_ids.insert(field.id);
        let duplicate_target =
            !targets.insert((field.entity_mapping_id, field.target_field.as_str()));
        for (invalid, part) in [
            (duplicate_id, "id"),
            (owner.is_none(), "owner"),
            (invalid_target, "target"),
            (duplicate_target, "duplicate_target"),
        ] {
            if !invalid {
                continue;
            }
            let mut finding =
                Finding::blocker(FindingCode::MappingInvalid, Capability::MappingValidation);
            finding.entity = entity.map(|e| e.source_key().into());
            finding.record_identity_sha256 = Some(digest(field.id.as_bytes()));
            finding.field_path = Some(format!(
                "mapping.fields.{part}.sha256:{}",
                digest(field.target_field.as_bytes())
            ));
            findings.push(finding);
        }
    }
    mappings
}

/// Group the immutable document's field mappings by the entity mapping they
/// belong to.
fn field_mappings_by_entity(config: &MappingConfigDocument) -> HashMap<Uuid, Vec<&FieldMapping>> {
    let mut fields_by_mapping: HashMap<Uuid, Vec<&FieldMapping>> = HashMap::new();
    for field in &config.field_mappings {
        fields_by_mapping
            .entry(field.entity_mapping_id)
            .or_default()
            .push(field);
    }
    fields_by_mapping
}

/// Validate one snapshot record's identity and return its approved entity.
///
/// A duplicate `(provider, entity, external_id)` identity, a record belonging
/// to a different provider, or an unsupported source entity blocks the pull.
fn check_record_identity(
    record: &SourceRecord,
    source_provider_id: Uuid,
    seen: &mut HashSet<(Uuid, SourceEntity, String)>,
) -> Result<InventoryEntity, AppError> {
    if !seen.insert((
        record.identity.source_provider_id,
        record.identity.entity.clone(),
        record.identity.external_id.clone(),
    )) {
        return Err(AppError::Validation(format!(
            "Source snapshot contains duplicate object identities for {}",
            record.identity.entity.as_str(),
        )));
    }
    if record.identity.source_provider_id != source_provider_id {
        return Err(AppError::Validation(
            "snapshot record belongs to a different source provider".to_string(),
        ));
    }
    InventoryEntity::from_source_key(record.identity.entity.as_str()).ok_or_else(|| {
        AppError::Validation(format!(
            "unsupported source entity {}",
            record.identity.entity.as_str()
        ))
    })
}

/// Whether a source value counts as populated for the closed-catalogue check.
fn source_value_populated(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(properties) => !properties.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// Count the extracted rows per source entity for `sync_job.stats`.
fn count_entities_extracted(snapshot: &SourceSnapshot) -> BTreeMap<String, u64> {
    let mut entities_extracted = BTreeMap::new();
    for record in &snapshot.records {
        *entities_extracted
            .entry(record.identity.entity.as_str().to_string())
            .or_insert(0) += 1;
    }
    entities_extracted
}

/// Count preserved membership edges without inventing source entity rows.
fn count_tag_memberships(
    snapshot: &SourceSnapshot,
    evidence: Option<&VerifiedTagRelationships>,
) -> u64 {
    let independent = snapshot
        .records
        .iter()
        .filter(|record| {
            record.identity.entity.as_str() == InventoryEntity::TaggedObject.source_key()
        })
        .count() as u64;
    if independent == 0
        && let Some(evidence) = evidence
    {
        evidence
            .edges()
            .values()
            .map(|tags| tags.len() as u64)
            .sum()
    } else {
        independent
    }
}

/// Merge the batch's deterministic ids with the already-recorded bindings; a
/// recorded binding always wins so a re-run never re-keys an existing row.
fn merge_local_ids(
    batch: &LocalInventoryBatch,
    existing: HashMap<(String, String), Uuid>,
) -> HashMap<(String, String), Uuid> {
    let mut local_ids: HashMap<(String, String), Uuid> = HashMap::new();
    for row in &batch.rows {
        local_ids.insert(
            (row.source_entity.clone(), row.external_id.clone()),
            row.local_id,
        );
    }
    for (key, value) in existing {
        local_ids.insert(key, value);
    }
    local_ids
}

/// Inspect every tag definition; a name match never adopts an operator row.
/// Check catalog-backed natural-key indexes without adopting a matching row.
/// Unknown partial/expression indexes or unavailable components are blockers,
/// not evidence that the target is conflict-free.
fn effective_device_index_value(
    row: &PlannedRow,
    current: Option<&Value>,
    column: &str,
    nullable: bool,
) -> Option<Value> {
    if let Some(planned) = row.columns.iter().find(|value| value.column == column) {
        if current.is_none() || planned.updatable {
            return match &planned.value {
                ColumnValue::Uuid(value) => Some(serde_json::json!(value)),
                ColumnValue::Scalar(_, InventoryValue::Null) => Some(Value::Null),
                ColumnValue::Scalar(_, InventoryValue::Text { value }) => {
                    Some(Value::String(value.clone()))
                }
                _ => None,
            };
        }
    }
    match current {
        Some(value) => value.get(column).cloned(),
        None if nullable => Some(Value::Null),
        None => None,
    }
}

/// Reproduce the closed Device and VM indexes using the database's own lower and
/// collation semantics. Source values are bound, never interpolated as SQL.
async fn assess_device_name_index(
    transaction: &mut Transaction<'_, Postgres>,
    row: &PlannedRow,
    current: Option<&Value>,
    index_name: &str,
    incoming: &mut Vec<(String, Uuid, Uuid, Option<Uuid>, String)>,
    findings: &mut Vec<Finding>,
    complete: &mut bool,
) -> Result<(), AppError> {
    let vm = row.entity == InventoryEntity::VirtualMachine;
    let scope_column = if vm { "cluster_id" } else { "site_id" };
    let values = (
        effective_device_index_value(row, current, "name", true),
        effective_device_index_value(row, current, scope_column, vm),
        effective_device_index_value(row, current, "tenant_id", true),
    );
    let parsed = match values {
        (Some(name), Some(site), Some(tenant)) => {
            let name = if name.is_null() {
                Some(None)
            } else {
                name.as_str().map(|value| Some(value.to_string()))
            };
            let site = if site.is_null() && vm {
                Some(None)
            } else {
                site.as_str()
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .map(Some)
            };
            let tenant = if tenant.is_null() {
                Some(None)
            } else {
                tenant
                    .as_str()
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .map(Some)
            };
            name.zip(site)
                .zip(tenant)
                .map(|((name, site), tenant)| (name, site, tenant))
        }
        _ => None,
    };
    let mut finding = Finding::blocker(
        FindingCode::AnalysisDependencyUnavailable,
        Capability::TargetGraph,
    );
    finding.entity = Some(row.source_entity.clone());
    finding.record_identity_sha256 = Some(digest(row.external_id.as_bytes()));
    finding.field_path = Some("natural_key".into());
    let Some((name, site, tenant)) = parsed else {
        findings.push(finding);
        *complete = false;
        return Ok(());
    };
    // Standard unique indexes do not consider NULL values equal.
    let Some(name) = name else { return Ok(()) };
    let Some(site) = site else { return Ok(()) };
    let root = index_name == "dcim_device_unique_name_site"
        || index_name == "virtualization_virtualmachine_unique_name_cluster";
    if root != tenant.is_none() {
        return Ok(());
    }
    let mut duplicate = false;
    for (seen_index, seen_id, seen_site, seen_tenant, seen_name) in incoming.iter() {
        if seen_index == index_name
            && *seen_id != row.local_id
            && *seen_site == site
            && *seen_tenant == tenant
        {
            let equal: bool = sqlx::query_scalar("SELECT lower($1::text COLLATE public.natural_sort) = lower($2::text COLLATE public.natural_sort)")
                .bind(&name).bind(seen_name).fetch_one(&mut **transaction).await?;
            duplicate |= equal;
        }
    }
    incoming.push((index_name.into(), row.local_id, site, tenant, name.clone()));
    let table = if vm {
        "virtualization_virtualmachine"
    } else {
        "dcim_device"
    };
    let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
    query.push(table).push(" d WHERE d.id <> ");
    query
        .push_bind(row.local_id)
        .push(" AND d.")
        .push(scope_column)
        .push(" = ")
        .push_bind(site);
    if let Some(tenant) = tenant {
        query.push(" AND d.tenant_id = ").push_bind(tenant);
    } else {
        query.push(" AND d.tenant_id IS NULL");
    }
    query
        .push(" AND lower(d.name::text) = lower(")
        .push_bind(name)
        .push("::text COLLATE public.natural_sort))");
    let conflict: bool = query
        .build_query_scalar()
        .fetch_one(&mut **transaction)
        .await?;
    if duplicate || conflict {
        finding.code = FindingCode::TargetDefinitionConflict;
        findings.push(finding);
    }
    Ok(())
}

async fn assess_natural_key_conflicts(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    findings: &mut Vec<Finding>,
    complete: &mut bool,
) -> Result<(), AppError> {
    let mut indexes: HashMap<
        InventoryEntity,
        Vec<(
            Vec<String>,
            bool,
            Option<String>,
            bool,
            String,
            Option<String>,
        )>,
    > = HashMap::new();
    let mut incoming =
        BTreeMap::<(String, String), (Vec<&str>, Vec<(Uuid, Value, Finding, bool)>)>::new();
    let mut device_incoming = Vec::new();
    for row in planned {
        let Some(table) = row.entity.table() else {
            continue;
        };
        if !indexes.contains_key(&row.entity) {
            let discovered: Vec<(Vec<String>, bool, Option<String>, bool, String, Option<String>)> = sqlx::query_as(
                "SELECT array_agg(COALESCE(a.attname::text, '__expression__') ORDER BY k.ord), i.indnullsnotdistinct, pg_get_expr(i.indpred, i.indrelid), (i.indexprs IS NOT NULL), idx.relname::text, CASE WHEN i.indexprs IS NOT NULL THEN pg_get_indexdef(i.indexrelid, 1, false) END \
                 FROM pg_index i JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum,ord) ON k.ord <= i.indnkeyatts \
                 LEFT JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum \
                 JOIN pg_class idx ON idx.oid=i.indexrelid \
                 WHERE i.indrelid=to_regclass($1) AND i.indisunique AND NOT i.indisprimary \
                 GROUP BY i.indexrelid, i.indnullsnotdistinct, pg_get_expr(i.indpred, i.indrelid), (i.indexprs IS NOT NULL), idx.relname"
            ).bind(table).fetch_all(&mut **transaction).await?;
            indexes.insert(row.entity, discovered);
        }
        let mut query = QueryBuilder::<Postgres>::new("SELECT to_jsonb(t) FROM ");
        query
            .push(table)
            .push(" t WHERE id=")
            .push_bind(row.local_id);
        let current: Option<Value> = query
            .build_query_scalar()
            .fetch_optional(&mut **transaction)
            .await?;
        for (names, nulls_not_distinct, predicate, expression, index_name, expression_sql) in
            &indexes[&row.entity]
        {
            let device_index = row.entity == InventoryEntity::Device
                && *expression
                && !*nulls_not_distinct
                && expression_sql.as_deref() == Some("lower((name)::text)")
                && ((index_name == "dcim_device_unique_name_site"
                    && names
                        .iter()
                        .map(String::as_str)
                        .eq(["__expression__", "site_id"])
                    && predicate.as_deref() == Some("(tenant_id IS NULL)"))
                    || (index_name == "dcim_device_unique_name_site_tenant"
                        && names.iter().map(String::as_str).eq([
                            "__expression__",
                            "site_id",
                            "tenant_id",
                        ])
                        && predicate.is_none()));
            if device_index {
                assess_device_name_index(
                    transaction,
                    row,
                    current.as_ref(),
                    index_name,
                    &mut device_incoming,
                    findings,
                    complete,
                )
                .await?;
                continue;
            }
            let vm_index = row.entity == InventoryEntity::VirtualMachine
                && *expression
                && !*nulls_not_distinct
                && expression_sql.as_deref() == Some("lower((name)::text)")
                && ((index_name == "virtualization_virtualmachine_unique_name_cluster"
                    && names
                        .iter()
                        .map(String::as_str)
                        .eq(["__expression__", "cluster_id"])
                    && predicate.as_deref() == Some("(tenant_id IS NULL)"))
                    || (index_name == "virtualization_virtualmachine_unique_name_cluster_tenant"
                        && names.iter().map(String::as_str).eq([
                            "__expression__",
                            "cluster_id",
                            "tenant_id",
                        ])
                        && predicate.is_none()));
            if vm_index {
                assess_device_name_index(
                    transaction,
                    row,
                    current.as_ref(),
                    index_name,
                    &mut device_incoming,
                    findings,
                    complete,
                )
                .await?;
                continue;
            }
            let root_column = match (row.entity, predicate.as_deref()) {
                (_, Some("(parent_id IS NULL)")) => Some("parent_id"),
                (InventoryEntity::Platform, Some("(manufacturer_id IS NULL)")) => {
                    Some("manufacturer_id")
                }
                (InventoryEntity::Tenant, Some("(group_id IS NULL)")) => Some("group_id"),
                _ => None,
            };
            let root_predicate = root_column.is_some();
            let partial_or_expression = *expression || (predicate.is_some() && !root_predicate);
            if root_predicate {
                let parent = row.columns.iter().find(|column| {
                    Some(column.column) == root_column && (current.is_none() || column.updatable)
                });
                let parent_null = match parent.map(|column| &column.value) {
                    Some(ColumnValue::Uuid(value)) => Some(value.is_none()),
                    Some(_) => None,
                    None => match &current {
                        Some(value) => value
                            .get(root_column.unwrap_or_default())
                            .map(Value::is_null),
                        None => row
                            .entity
                            .field_iter()
                            .find(|field| Some(field.column()) == root_column)
                            .and_then(|field| {
                                (field.nullable() && field.spec().default.is_none()).then_some(true)
                            }),
                    },
                };
                if parent_null == Some(false) {
                    continue;
                }
                if parent_null.is_none() {
                    let mut finding = Finding::blocker(
                        FindingCode::AnalysisDependencyUnavailable,
                        Capability::TargetGraph,
                    );
                    finding.entity = Some(row.source_entity.clone());
                    finding.record_identity_sha256 = Some(digest(row.external_id.as_bytes()));
                    finding.field_path = Some("natural_key".into());
                    findings.push(finding);
                    *complete = false;
                    continue;
                }
            }
            if current.is_some()
                && !partial_or_expression
                && !root_predicate
                && !names.iter().any(|name| {
                    row.columns
                        .iter()
                        .any(|column| column.updatable && column.column == name)
                })
            {
                continue;
            }
            let mut values = Vec::new();
            let mut columns = Vec::new();
            let mut unknown = partial_or_expression;
            for name in names {
                // This legacy physical column is not imported. Inserts leave it
                // NULL; updates preserve the target value.
                if row.entity == InventoryEntity::Cluster && name == "_site_id" {
                    columns.push("_site_id");
                    if let Some(current) = &current {
                        if let Some(value) = current.get(name) {
                            values.push(value.clone());
                        } else {
                            unknown = true;
                            break;
                        }
                    } else {
                        values.push(Value::Null);
                    }
                    continue;
                }
                // A generic reference owns both physical columns. Its type
                // companion participates in native uniqueness just like its id.
                let Some((field, physical_column)) = row.entity.field_iter().find_map(|field| {
                    if field.column() == name {
                        return Some((field, field.column()));
                    }
                    match field.kind() {
                        FieldKind::GenericReference { type_column, .. } if type_column == name => {
                            Some((field, type_column))
                        }
                        _ => None,
                    }
                }) else {
                    unknown = true;
                    break;
                };
                columns.push(physical_column);
                let value =
                    if let Some(column) = row.columns.iter().find(|column| {
                        column.column == name && (current.is_none() || column.updatable)
                    }) {
                        match &column.value {
                            ColumnValue::Uuid(value) => serde_json::json!(value),
                            ColumnValue::Scalar(_, InventoryValue::Null) => Value::Null,
                            ColumnValue::Scalar(_, InventoryValue::Boolean { value }) => {
                                serde_json::json!(value)
                            }
                            ColumnValue::Scalar(_, InventoryValue::Integer { value }) => {
                                serde_json::json!(value)
                            }
                            ColumnValue::Scalar(_, InventoryValue::Number { value }) => {
                                serde_json::json!(value)
                            }
                            ColumnValue::Scalar(_, InventoryValue::Text { value })
                            | ColumnValue::Copy(value) => serde_json::json!(value),
                            ColumnValue::Scalar(_, InventoryValue::Json { value }) => value.clone(),
                            ColumnValue::Tags(value) => serde_json::json!(value),
                            ColumnValue::Literal(_) => {
                                unknown = true;
                                break;
                            }
                        }
                    } else if let Some(current) = &current {
                        match current.get(name) {
                            Some(value) => value.clone(),
                            None => {
                                unknown = true;
                                break;
                            }
                        }
                    } else if field.nullable() && field.spec().default.is_none() {
                        Value::Null
                    } else {
                        unknown = true;
                        break;
                    };
                values.push(value);
            }
            let mut finding = Finding::blocker(
                if unknown {
                    FindingCode::AnalysisDependencyUnavailable
                } else {
                    FindingCode::TargetDefinitionConflict
                },
                Capability::TargetGraph,
            );
            finding.entity = Some(row.source_entity.clone());
            finding.record_identity_sha256 = Some(digest(row.external_id.as_bytes()));
            finding.field_path = Some("natural_key".into());
            if unknown {
                findings.push(finding);
                *complete = false;
                continue;
            }
            if !nulls_not_distinct && values.iter().any(Value::is_null) {
                continue;
            }
            let object = Value::Object(
                columns
                    .iter()
                    .zip(&values)
                    .map(|(column, value)| ((*column).to_string(), value.clone()))
                    .collect(),
            );
            let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
            query
                .push(table)
                .push(" t WHERE id <> ")
                .push_bind(row.local_id);
            if row.entity == InventoryEntity::PortTemplateMapping {
                let selected: Vec<Uuid> = planned
                    .iter()
                    .filter(|plan| plan.entity == row.entity)
                    .map(|plan| plan.local_id)
                    .collect();
                query
                    .push(" AND NOT (t.id = ANY(")
                    .push_bind(selected)
                    .push("))");
            }
            if root_predicate {
                query
                    .push(" AND t.")
                    .push(root_column.unwrap_or_default())
                    .push(" IS NULL");
            }
            for column in &columns {
                query
                    .push(" AND t.")
                    .push(*column)
                    .push(" IS NOT DISTINCT FROM (jsonb_populate_record(NULL::")
                    .push(table)
                    .push(", ")
                    .push_bind(&object)
                    .push(")).")
                    .push(*column);
            }
            query.push(")");
            let conflict: bool = query
                .build_query_scalar()
                .fetch_one(&mut **transaction)
                .await?;
            incoming
                .entry((table.to_string(), index_name.clone()))
                .or_insert_with(|| (columns.clone(), Vec::new()))
                .1
                .push((row.local_id, object, finding.clone(), conflict));
            if conflict {
                findings.push(finding);
            }
        }
    }
    // Group typed values in PostgreSQL once per index. Window partitions use
    // the native column types and collations, including NULLS NOT DISTINCT.
    for ((table, _), (columns, rows)) in incoming {
        if rows.len() < 2 {
            continue;
        }
        let records: Vec<Value> = rows
            .iter()
            .map(|(id, key, _, _)| serde_json::json!({"id": id, "key": key}))
            .collect();
        let mut query = QueryBuilder::<Postgres>::new(
            "WITH typed AS (SELECT e.ord AS __ordinal, e.item->>'id' AS __id",
        );
        for column in &columns {
            query.push(", r.").push(*column);
        }
        query.push(" FROM jsonb_array_elements(").push_bind(serde_json::json!(records))
            .push(") WITH ORDINALITY AS e(item, ord) CROSS JOIN LATERAL jsonb_populate_record(NULL::")
            .push(&table).push(", e.item->'key') r), ranked AS (SELECT __ordinal, __id, min(__id) OVER w AS lo, max(__id) OVER w AS hi FROM typed WINDOW w AS (PARTITION BY ");
        for (position, column) in columns.iter().enumerate() {
            if position != 0 {
                query.push(", ");
            }
            query.push(*column);
        }
        query.push(" ORDER BY __ordinal ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING)) SELECT __ordinal FROM ranked WHERE lo <> __id OR hi <> __id ORDER BY __ordinal");
        let duplicates: Vec<i64> = query
            .build_query_scalar()
            .fetch_all(&mut **transaction)
            .await?;
        for ordinal in duplicates {
            let index = usize::try_from(ordinal - 1)
                .map_err(|_| AppError::Internal("inventory_transfer:natural_key_ordinal".into()))?;
            let Some((_, _, finding, conflict)) = rows.get(index) else {
                return Err(AppError::Internal(
                    "inventory_transfer:natural_key_ordinal".into(),
                ));
            };
            if !conflict {
                findings.push(finding.clone());
            }
        }
    }
    Ok(())
}

async fn assess_tag_slug_conflicts(
    transaction: &mut Transaction<'_, Postgres>,
    batch: &LocalInventoryBatch,
    local_ids: &HashMap<(String, String), Uuid>,
    findings: &mut Vec<Finding>,
    complete: &mut bool,
) -> Result<(), AppError> {
    for row in batch
        .rows
        .iter()
        .filter(|row| row.entity == InventoryEntity::Tag)
    {
        let slug = row
            .entity
            .field("slug")
            .and_then(|field| row.scalars.get(&field))
            .and_then(|value| match value {
                InventoryValue::Text { value } => Some(value),
                _ => None,
            });
        let Some(slug) = slug else {
            findings.push(prepared_finding(
                row,
                FindingCode::TagDefinitionInvalid,
                Some("slug"),
                Capability::TargetGraph,
            ));
            continue;
        };
        let local_id = local_ids.get(&(row.source_entity.clone(), row.external_id.clone()));
        let existing: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM inventory_tag WHERE slug=$1 ORDER BY id")
                .bind(slug)
                .fetch_all(&mut **transaction)
                .await?;
        let Some(local_id) = local_id else {
            findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some("identity"),
                Capability::TargetGraph,
            ));
            *complete = false;
            continue;
        };
        for id in existing.into_iter().filter(|id| id != local_id) {
            let mut finding = prepared_finding(
                row,
                FindingCode::TargetDefinitionConflict,
                Some("slug"),
                Capability::TargetGraph,
            );
            finding.edge = Some(
                crate::contexts::sync::domain::inventory_preflight::EdgeContext {
                    target_entity: Some(row.source_entity.clone()),
                    target_identity_sha256: None,
                    target_local_id: Some(id),
                },
            );
            findings.push(finding);
        }
    }
    Ok(())
}

/// Write every planned row, its junctions and its binding, accumulating commit
/// statistics.
async fn commit_planned_rows(
    transaction: &mut Transaction<'_, Postgres>,
    source_provider_id: Uuid,
    job_id: Uuid,
    planned: &[PlannedRow],
    stats: &mut InventoryCommitStats,
    changed: Option<&HashSet<(InventoryEntity, Uuid)>>,
) -> Result<(), AppError> {
    for plan in planned {
        upsert_binding(transaction, source_provider_id, plan, job_id).await?;
    }
    if let Some(changed) = changed {
        let changed_rows: Vec<_> = planned
            .iter()
            .filter(|row| changed.contains(&(row.entity, row.local_id)))
            .cloned()
            .collect();
        apply_planned_rows(transaction, &changed_rows).await?;
    } else {
        apply_planned_rows(transaction, planned).await?;
    }
    refresh_scope_caches(transaction, planned).await?;
    for plan in planned {
        stats.committed += 1;
        *stats
            .entities_committed
            .entry(plan.source_entity.clone())
            .or_insert(0) += 1;
        for (name, _) in &plan.relations {
            *stats.foreign_keys.entry(name.clone()).or_insert(0) += 1;
        }
    }
    Ok(())
}

/// Reconstruct native denormalized scope columns from the final Device owners.
/// Include unchanged selected components: their Device may have moved in this
/// batch even when the component's own mapped columns did not change.
async fn refresh_scope_caches(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
) -> Result<(), AppError> {
    for entity in InventoryEntity::all() {
        let columns = entity.scope_cache_columns();
        if columns.is_empty() {
            continue;
        }
        let ids: Vec<_> = planned
            .iter()
            .filter(|row| row.entity == entity)
            .map(|row| row.local_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let table = entity.table().expect("native scope owner metadata");
        let assignments = columns
            .iter()
            .map(|(cache, canonical)| format!("{cache}=device.{canonical}"))
            .collect::<Vec<_>>()
            .join(",");
        let different = columns
            .iter()
            .map(|(cache, canonical)| {
                format!("component.{cache} IS DISTINCT FROM device.{canonical}")
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut query = QueryBuilder::<Postgres>::new("UPDATE ");
        query.push(table).push(" AS component SET ").push(assignments)
            .push(" FROM dcim_device AS device WHERE component.device_id=device.id AND component.id=ANY(")
            .push_bind(ids).push(") AND (").push(different).push(")");
        query.build().execute(&mut **transaction).await?;
    }
    Ok(())
}

fn owns_bay_categories(entity: InventoryEntity) -> bool {
    matches!(
        entity,
        InventoryEntity::ModuleType
            | InventoryEntity::ModuleBay
            | InventoryEntity::ModuleBayTemplate
    )
}

async fn apply_planned_rows(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
) -> Result<(), AppError> {
    // This owner is also used in transaction-level replay tests, without
    // commit_pull's boundary. Never acquire graph after a resource row lock;
    // retain exclusive Tag definition intent before graph when needed.
    if planned.iter().any(|row| row.entity.uses_hardware_graph()) {
        if planned.iter().any(|row| row.entity == InventoryEntity::Tag) {
            lock_tag_catalog_for_definition(transaction).await?;
        }
        lock_hardware_graph(transaction).await?;
    }
    // Nullable references between incoming rows can be installed after all
    // identities exist. Hardware and tree rows retain native graph ordering and
    // validation; they are never relaxed by this pass.
    let incoming: HashSet<_> = planned
        .iter()
        .map(|row| (row.entity, row.local_id))
        .collect();
    let mut first_pass = planned.to_vec();
    let mut deferred = Vec::new();
    for row in &mut first_pass {
        let mut columns = Vec::new();
        for column in &mut row.columns {
            if column.column == "custom_field_data" && column.updatable {
                columns.push(column.clone());
                // Inserts require NOT NULL, while updates preserve the old JSON
                // until every referenced identity has been written.
                column.value = ColumnValue::Literal("'{}'::jsonb");
                column.updatable = false;
            }
        }
        let mut deferred_columns = HashSet::new();
        for field in row.entity.field_iter().filter(|field| {
            row.entity.allows_deferred_references()
                || (row.entity == InventoryEntity::InterfaceTemplate && field.name() == "bridge")
        }) {
            if !field.nullable() {
                continue;
            }
            if !matches!(
                field.kind(),
                FieldKind::ForeignKey { .. } | FieldKind::GenericReference { .. }
            ) {
                continue;
            }
            if row.columns.iter().any(|column| {
                column.column == field.column()
                    && matches!(&column.value, ColumnValue::Uuid(Some(id)) if row.reference_is_incoming(field.column(), *id, &incoming))
            }) {
                deferred_columns.insert(field.column());
                if let FieldKind::GenericReference { type_column, .. } = field.kind() {
                    deferred_columns.insert(type_column);
                }
            }
        }
        row.columns.retain(|column| {
            if deferred_columns.contains(column.column) {
                columns.push(column.clone());
                false
            } else {
                true
            }
        });
        if !columns.is_empty() {
            deferred.push((row.entity, row.local_id, columns));
        }
    }
    // Check the remaining required dependencies before any mutation.
    let ordered = hardware_write_order(&first_pass)?;
    // Mapping positions have immediate uniqueness constraints. Remove only the
    // selected rows before reinserting their final states so position swaps are
    // atomic; bindings and omitted mapping rows are not removed.
    for row in planned.iter().filter(|row| {
        row.entity == InventoryEntity::PortTemplateMapping
            && [
                "front_port_id",
                "rear_port_id",
                "front_port_position",
                "rear_port_position",
                "device_type_id",
                "module_type_id",
            ]
            .iter()
            .all(|name| row.columns.iter().any(|column| column.column == *name))
    }) {
        sqlx::query("DELETE FROM dcim_porttemplatemapping WHERE id=$1")
            .bind(row.local_id)
            .execute(&mut **transaction)
            .await?;
    }
    // Remove obsolete selected category members before inserting final members. Empty
    // sets are natively unrestricted, so an existing occupant is not rejected
    // against a mixture of old and final categories. Omitted junctions have no
    // plan entry and are never cleared. Missing owner IDs are harmless here.
    let mut bay_categories_changed = false;
    for plan in planned
        .iter()
        .filter(|plan| owns_bay_categories(plan.entity))
    {
        for junction in &plan.junctions {
            bay_categories_changed |=
                junction_changed(transaction, plan.local_id, junction).await?;
        }
    }
    for plan in planned
        .iter()
        .filter(|plan| owns_bay_categories(plan.entity))
    {
        if bay_categories_changed {
            for junction in &plan.junctions {
                clear_junction(transaction, plan.local_id, junction).await?;
            }
        }
    }
    // Selected owners and their complete final category sets precede missing
    // Module installations, once all referenced category rows exist.
    for plan in ordered {
        upsert_row(transaction, plan).await?;
        if plan.entity == InventoryEntity::Tag {
            // Scope is a complete set before dependent native memberships are
            // written. Deferred checks still allow selected existing uses to clear.
            replace_junctions(transaction, plan).await?;
        }
        if owns_bay_categories(plan.entity) && bay_categories_changed {
            for junction in &plan.junctions {
                populate_junction(transaction, plan.local_id, junction).await?;
            }
        }
    }
    for (entity, id, columns) in &deferred {
        let table = entity.table().ok_or_else(|| {
            AppError::Internal("a membership resource cannot be written as a row".into())
        })?;
        let mut query = QueryBuilder::<Postgres>::new("UPDATE ");
        query.push(table).push(" SET ");
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                query.push(", ");
            }
            query.push(column.column).push(" = ");
            push_column_value(&mut query, &column.value)?;
        }
        query.push(" WHERE id = ").push_bind(*id);
        query.push(" AND (");
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                query.push(" OR ");
            }
            query.push(column.column).push(" IS DISTINCT FROM ");
            push_column_value(&mut query, &column.value)?;
        }
        query.push(")");
        query.build().execute(&mut **transaction).await?;
    }
    for plan in planned
        .iter()
        .filter(|plan| !owns_bay_categories(plan.entity) && plan.entity != InventoryEntity::Tag)
    {
        replace_junctions(transaction, plan).await?;
        replace_selectors(transaction, plan).await?;
    }
    if planned
        .iter()
        .any(|plan| plan.entity == InventoryEntity::Tag)
    {
        validate_scope_registry_in_transaction(transaction).await?;
        for plan in planned
            .iter()
            .filter(|plan| plan.entity == InventoryEntity::Tag)
        {
            // This owner is also called by rollback-only replay. A deferred
            // constraint alone would not run before those callers roll back.
            sqlx::query("SELECT inventory_tag_scope_check($1)")
                .bind(plan.local_id)
                .execute(&mut **transaction)
                .await?;
        }
    }
    Ok(())
}

fn hardware_write_order(planned: &[PlannedRow]) -> Result<Vec<&PlannedRow>, AppError> {
    let imported: HashSet<_> = planned
        .iter()
        .map(|row| (row.entity, row.local_id))
        .collect();
    let mut written = HashSet::new();
    let mut ordered = Vec::with_capacity(planned.len());
    // Tags retain their established ordering before any native tags[] writer.
    let mut pending: Vec<_> = planned.iter().collect();
    pending.sort_by_key(|row| row.entity != InventoryEntity::Tag);
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|row| {
            let ready = row.columns.iter().all(|column| {
                match (&column.value, row.reference_target(column.column)) {
                    (ColumnValue::Uuid(Some(id)), Some(target)) => {
                        !imported.contains(&(target, *id)) || written.contains(&(target, *id))
                    }
                    _ => true,
                }
            }) && (!owns_bay_categories(row.entity)
                || row.junctions.iter().all(|junction| {
                    junction.refs.iter().all(|id| {
                        row.junction_target(junction).is_none_or(|target| {
                            !imported.contains(&(target, *id)) || written.contains(&(target, *id))
                        })
                    })
                }));
            if ready {
                written.insert((row.entity, row.local_id));
                ordered.push(*row);
            }
            !ready
        });
        if pending.len() == before {
            return Err(AppError::Validation(
                "imported required or protected dependencies contain a cycle".into(),
            ));
        }
    }
    Ok(ordered)
}

/// Rebuild the nested-set bookkeeping of every touched MPTT table from the
/// whole parent graph, so hierarchy filters stay valid.
async fn rebuild_touched_trees(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
) -> Result<(), AppError> {
    let mut tree_entities: Vec<InventoryEntity> = planned
        .iter()
        .map(|plan| plan.entity)
        .filter(|entity| entity.is_tree())
        .collect();
    tree_entities.sort_unstable();
    tree_entities.dedup();
    for entity in tree_entities {
        recompute_tree(transaction, entity).await?;
    }
    Ok(())
}

/// Recompute the maintained counters for the entities that own one.
async fn recompute_counts(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    previous_device_owners: &[Uuid],
) -> Result<(), AppError> {
    let ids_for = |entity| {
        planned
            .iter()
            .filter(|plan| plan.entity == entity)
            .map(|plan| plan.local_id)
            .collect::<Vec<_>>()
    };
    let mut devices = ids_for(InventoryEntity::Device);
    devices.extend_from_slice(previous_device_owners);
    devices.extend(counters::device_owners(transaction, planned).await?);
    devices.sort_unstable();
    devices.dedup();
    counters::recompute(
        transaction,
        &devices,
        &ids_for(InventoryEntity::VirtualMachine),
        &ids_for(InventoryEntity::Prefix),
    )
    .await?;
    Ok(())
}

/// Whether the source property feeding a field mapping is present in the
/// record. A declared `literal null` is always an explicit null; otherwise the
/// referenced source field (or the mapping's `source_field`) decides.
fn property_present(
    record: &SourceRecord,
    field_mapping: &FieldMapping,
    expression: &Value,
) -> bool {
    if is_literal_null(expression) {
        return true;
    }
    if let Some(field) = referenced_field(expression) {
        return record.fields.contains_key(field);
    }
    record.fields.contains_key(&field_mapping.source_field)
}

fn is_literal_null(expression: &Value) -> bool {
    expression.get("op").and_then(Value::as_str) == Some("literal")
        && expression
            .get("args")
            .and_then(Value::as_array)
            .and_then(|args| args.first())
            .is_none_or(Value::is_null)
}

fn referenced_field(expression: &Value) -> Option<&str> {
    expression
        .get("args")
        .and_then(Value::as_array)
        .and_then(|args| args.first())
        .and_then(|arg| arg.get("field"))
        .and_then(Value::as_str)
}

/// One object/null boundary for raw and explicitly mapped custom fields.
fn custom_fields_object(value: &Value) -> Result<serde_json::Map<String, Value>, AppError> {
    if value.is_null() {
        return Ok(serde_json::Map::new());
    }
    let Some(map) = value.as_object() else {
        return Err(AppError::Validation(
            "inventory_preflight:custom_fields_object_required".into(),
        ));
    };
    Ok(map.clone())
}

/// Missing preserves values; explicit null clears them. A present malformed
/// value is never reclassified as absent.
fn custom_fields_from_record(
    record: &SourceRecord,
) -> Result<Option<serde_json::Map<String, Value>>, AppError> {
    record
        .fields
        .get("custom_fields")
        .map(custom_fields_object)
        .transpose()
}

/// Mutable accumulator for the per-field decisions `prepare_row` makes before
/// the row is handed to the commit transaction.
#[derive(Default)]
struct PreparedColumns {
    scalars: BTreeMap<InventoryField, InventoryValue>,
    references: Vec<(InventoryField, ReferenceBinding)>,
    cleared: Vec<InventoryField>,
    junctions: Vec<(InventoryField, Vec<ReferenceBinding>)>,
    selectors: Vec<(InventoryField, Vec<ReferenceBinding>)>,
    custom_fields: Option<serde_json::Map<String, Value>>,
    tags: Option<Vec<crate::contexts::sync::domain::SourceIdentity>>,
}

impl PreparedColumns {
    /// Merge only a successfully bound field. Failed binders may have touched
    /// their private accumulator; those partial decisions are discarded.
    fn append(&mut self, mut field: Self) {
        self.scalars.append(&mut field.scalars);
        self.references.append(&mut field.references);
        self.cleared.append(&mut field.cleared);
        self.junctions.append(&mut field.junctions);
        self.selectors.append(&mut field.selectors);
        if field.custom_fields.is_some() {
            self.custom_fields = field.custom_fields;
        }
        if field.tags.is_some() {
            self.tags = field.tags;
        }
    }
}

/// Immutable per-field inputs shared by the binding helpers: the source record,
/// its field mapping, the owning source key, the field name and the mapped value.
struct FieldContext<'a> {
    record: &'a SourceRecord,
    field_mapping: &'a FieldMapping,
    source_entity: &'a str,
    name: &'a str,
    value: &'a Value,
}

/// Capture a mapped `custom_fields` object when the mapping produced a value or
/// the source property is present; otherwise leave the accumulator untouched so
/// the raw-record fallback applies.
fn mapped_custom_fields_value(
    record: &SourceRecord,
    field_mapping: &FieldMapping,
    value: &Value,
) -> Result<Option<serde_json::Map<String, Value>>, AppError> {
    if value.is_null()
        && !property_present(record, field_mapping, &field_mapping.forward_transform_json)
    {
        return Ok(None);
    }
    Ok(Some(custom_fields_object(value)?))
}

/// Apply the explicit-null encoding for one field: clear the mapped column or
/// its catalogue-approved empty encoding, never leaving a stale relation. A
/// truly absent property preserves the column unless it is required on insert.
fn apply_explicit_null(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let source_entity = context.source_entity;
    let name = context.name;
    if !property_present(
        context.record,
        context.field_mapping,
        &context.field_mapping.forward_transform_json,
    ) {
        // A truly absent property preserves the local column, unless the
        // column is required on insert with no default.
        if field.absent() == Absent::Reject {
            return Err(AppError::Validation(format!(
                "required field {source_entity}::{name} is absent"
            )));
        }
        return Ok(());
    }
    // Explicit null: clear the mapped column (or its catalogue-approved empty
    // encoding); never leave a stale relation or a non-null value.
    match field.kind() {
        FieldKind::Scalar => apply_null_scalar(columns, field, context),
        FieldKind::Tags => Err(AppError::Validation("tags cannot be null".into())),
        FieldKind::Junction { .. } | FieldKind::ContentTypeJunction { .. } => {
            if !field.nullable() {
                return Err(AppError::Validation(
                    "inventory_preflight:relation_null".into(),
                ));
            }
            // An explicit null clears the relation to empty; the definition row
            // itself is still validated and written by the generic path.
            columns.junctions.push((field, Vec::new()));
            Ok(())
        }
        FieldKind::ForeignKey { .. } | FieldKind::GenericReference { .. } => {
            apply_null_relation(columns, field, context)
        }
        FieldKind::SelectorRows { .. } => {
            if !field.nullable() {
                return Err(AppError::Validation(
                    "inventory_preflight:relation_null".into(),
                ));
            }
            columns.selectors.push((field, Vec::new()));
            Ok(())
        }
    }
}

/// Encode an explicit null for a scalar column using its catalogue `Absent`
/// policy.
fn apply_null_scalar(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let source_entity = context.source_entity;
    let name = context.name;
    match field.absent() {
        Absent::EmptyText => {
            columns.scalars.insert(
                field,
                InventoryValue::Text {
                    value: String::new(),
                },
            );
        }
        Absent::EmptyArray => {
            columns.scalars.insert(
                field,
                InventoryValue::Json {
                    value: Value::Array(Vec::new()),
                },
            );
        }
        Absent::Zero => {
            columns
                .scalars
                .insert(field, InventoryValue::Integer { value: 0 });
        }
        Absent::Reject => {
            return Err(AppError::Validation(format!(
                "required field {source_entity}::{name} is null"
            )));
        }
        Absent::Preserve => {
            if field.nullable() {
                columns.scalars.insert(field, InventoryValue::Null);
            } else {
                return Err(AppError::Validation(format!(
                    "required field {source_entity}::{name} cannot be null"
                )));
            }
        }
    }
    Ok(())
}

/// Clear a nullable explicit-null relation; a non-nullable one is rejected.
fn apply_null_relation(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    if !field.nullable() {
        return Err(AppError::Validation(format!(
            "required relation {}::{} cannot be null",
            context.source_entity, context.name
        )));
    }
    columns.cleared.push(field);
    Ok(())
}

/// Bind one present (non-null) mapped value to its column, reference or junction
/// according to the field's kind.
fn bind_present_value(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    match field.kind() {
        FieldKind::Scalar | FieldKind::Tags => bind_scalar_or_tags(columns, field, context),
        FieldKind::ForeignKey { target } => bind_foreign_key(columns, field, target, context),
        FieldKind::GenericReference {
            source_type_field,
            allowed,
            ..
        } => bind_generic_reference(columns, field, source_type_field, allowed, context),
        FieldKind::ContentTypeJunction { .. } => {
            if field.entity() == InventoryEntity::Tag {
                let tokens = context.value.as_array().ok_or_else(|| {
                    AppError::Validation("inventory_preflight:tag_scope_invalid".into())
                })?;
                let mut seen = HashSet::new();
                for token in tokens {
                    let token = token.as_str().ok_or_else(|| {
                        AppError::Validation("inventory_preflight:tag_scope_invalid".into())
                    })?;
                    validate_scope_token(token)?;
                    if !seen.insert(token) {
                        return Err(AppError::Validation(
                            "inventory_preflight:tag_scope_duplicate".into(),
                        ));
                    }
                }
            }
            columns
                .junctions
                .push((field, content_type_references(context)?));
            Ok(())
        }
        FieldKind::Junction { target, .. } => {
            columns
                .junctions
                .push((field, junction_references(target, context)?));
            Ok(())
        }
        FieldKind::SelectorRows { .. } => {
            let bindings = dataflow_selectors::bind(
                &field.kind(),
                context.source_entity,
                context.name,
                context.value,
            )?;
            columns.selectors.push((field, bindings));
            Ok(())
        }
    }
}

fn bind_scalar_or_tags(
    columns: &mut PreparedColumns,
    field: InventoryField,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    if matches!(field.kind(), FieldKind::Tags) {
        columns.tags = Some(tag_references(
            context.value,
            context.record.identity.source_provider_id,
        )?);
    } else {
        columns
            .scalars
            .insert(field, convert_scalar(field, context.value)?);
    }
    Ok(())
}

fn bind_foreign_key(
    columns: &mut PreparedColumns,
    field: InventoryField,
    target: Option<InventoryEntity>,
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    if field.entity() == InventoryEntity::CustomField && field.name() == "related_object_type" {
        columns
            .references
            .push((field, custom_field_references::type_binding(context.value)?));
        return Ok(());
    }
    let external_id = external_id_of(context.value).ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} has no provider id",
            context.source_entity, context.name
        ))
    })?;
    columns.references.push((
        field,
        ReferenceBinding {
            target,
            type_token: None,
            external_id,
        },
    ));
    Ok(())
}

fn bind_generic_reference(
    columns: &mut PreparedColumns,
    field: InventoryField,
    source_type_field: &str,
    allowed: &[InventoryEntity],
    context: &FieldContext<'_>,
) -> Result<(), AppError> {
    let external_id = external_id_of(context.value).ok_or_else(|| {
        AppError::Validation(format!(
            "generic relation {}::{} has no provider id",
            context.source_entity, context.name
        ))
    })?;
    let token = record_value(context.record, source_type_field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "generic relation {}::{} has no type token",
                context.source_entity, context.name
            ))
        })?;
    let resolved = InventoryEntity::from_content_type_token(token).ok_or_else(|| {
        AppError::Validation(format!(
            "generic relation {}::{} has an unknown type",
            context.source_entity, context.name
        ))
    })?;
    if !allowed.contains(&resolved) {
        return Err(AppError::Validation(format!(
            "generic relation {}::{} points at a disallowed type",
            context.source_entity, context.name
        )));
    }
    columns.references.push((
        field,
        ReferenceBinding {
            target: Some(resolved),
            type_token: Some(token.to_string()),
            external_id,
        },
    ));
    Ok(())
}

/// Resolve a content-type junction value list into canonical type-token
/// bindings.
fn content_type_references(context: &FieldContext<'_>) -> Result<Vec<ReferenceBinding>, AppError> {
    let items = context.value.as_array().ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} is not a list",
            context.source_entity, context.name
        ))
    })?;
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        let token = item.as_str().ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} has a non-token entry",
                context.source_entity, context.name
            ))
        })?;
        refs.push(ReferenceBinding {
            target: None,
            type_token: Some(token.to_string()),
            external_id: token.to_string(),
        });
    }
    Ok(refs)
}

/// Resolve a junction value list into provider-scoped reference bindings.
fn junction_references(
    target: Option<InventoryEntity>,
    context: &FieldContext<'_>,
) -> Result<Vec<ReferenceBinding>, AppError> {
    let items = context.value.as_array().ok_or_else(|| {
        AppError::Validation(format!(
            "relation {}::{} is not a list",
            context.source_entity, context.name
        ))
    })?;
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        let external_id = external_id_of(item).ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} has an entry without a provider id",
                context.source_entity, context.name
            ))
        })?;
        refs.push(ReferenceBinding {
            target,
            type_token: None,
            external_id,
        });
    }
    Ok(refs)
}

/// Shared evaluator/binder for one mapping. Typed stage codes are assigned where
/// errors originate, never inferred from their human-readable bodies.
fn bind_mapping(
    entity: InventoryEntity,
    record: &SourceRecord,
    field_mapping: &FieldMapping,
    sample: &Value,
    columns: &mut PreparedColumns,
) -> Result<(), (FindingCode, AppError)> {
    let name = field_mapping.target_field.as_str();
    let source_entity = entity.source_key();
    let value = evaluate_expression(
        &field_mapping.forward_transform_json,
        sample,
        TransformDirection::Forward,
    )
    .map_err(|error| (FindingCode::ExpressionInvalid, error))?;
    if name == "custom_fields" {
        if let Some(mapped) = mapped_custom_fields_value(record, field_mapping, &value)
            .map_err(|error| (FindingCode::FieldValueInvalid, error))?
        {
            columns.custom_fields = Some(mapped);
        }
        return Ok(());
    }
    let field = entity.field(name).ok_or_else(|| {
        (
            FindingCode::MappingInvalid,
            AppError::Validation("inventory_preflight:target_field_invalid".into()),
        )
    })?;
    let context = FieldContext {
        record,
        field_mapping,
        source_entity,
        name,
        value: &value,
    };
    if value.is_null() {
        apply_explicit_null(columns, field, &context)
    } else {
        bind_present_value(columns, field, &context)
    }
    .map_err(|error| (FindingCode::FieldValueInvalid, error))
}

fn prepared_row(
    entity: InventoryEntity,
    source_provider_id: Uuid,
    record: &SourceRecord,
    mut columns: PreparedColumns,
    prepared_definition: Option<CustomFieldDefinition>,
    source_timestamps: SourceTimestamps,
) -> Result<PreparedRow, AppError> {
    if let Some(definition) = &prepared_definition {
        // Definition validation and the SQL writer must see the same mapped
        // values. Do not validate one source value then persist a transformed
        // one under the same definition name.
        let expected = serde_json::to_value(definition)
            .map_err(|_| AppError::Internal("inventory_transfer:definition_encoding".into()))?;
        for (field, value) in &columns.scalars {
            let key = match field.name() {
                "type" => "field_type",
                "unique" => "enforce_unique",
                "default" => "default_value",
                name => name,
            };
            let Some(expected_value) = expected.get(key) else {
                return Err(AppError::Validation(
                    "inventory_transfer:definition_mapping_unverified".into(),
                ));
            };
            let actual = match value {
                InventoryValue::Null => Value::Null,
                InventoryValue::Boolean { value } => serde_json::json!(value),
                InventoryValue::Integer { value } => serde_json::json!(value),
                InventoryValue::Number { value } => serde_json::json!(value),
                InventoryValue::Text { value } => serde_json::json!(value),
                InventoryValue::Json { value } => value.clone(),
            };
            if &actual != expected_value {
                return Err(AppError::Validation(
                    "inventory_transfer:definition_mapping_mismatch".into(),
                ));
            }
        }
        for (field, binding) in &columns.references {
            if field.name() == "related_object_type"
                && binding.type_token.as_deref()
                    != record
                        .fields
                        .get("related_object_type")
                        .and_then(Value::as_str)
            {
                return Err(AppError::Validation(
                    "inventory_transfer:definition_mapping_mismatch".into(),
                ));
            }
        }
    }
    let source_entity = entity.source_key().to_string();
    let local_id = derive_local_id(
        source_provider_id,
        &source_entity,
        &record.identity.external_id,
        &entity.target_key(),
    );
    if columns.custom_fields.is_none() {
        columns.custom_fields = custom_fields_from_record(record)?;
    }
    Ok(PreparedRow {
        entity,
        source_entity,
        external_id: record.identity.external_id.clone(),
        local_id,
        source_timestamps,
        scalars: columns.scalars,
        references: columns.references,
        cleared: columns.cleared,
        junctions: columns.junctions,
        selectors: columns.selectors,
        custom_fields: columns.custom_fields,
        prepared_definition,
        tags: columns.tags,
    })
}

// Source-local binder tests retain direct access to the same canonical path.
#[cfg(test)]
fn prepare_row(
    entity: InventoryEntity,
    source_provider_id: Uuid,
    record: &SourceRecord,
    fields: &[&FieldMapping],
) -> Result<PreparedRow, AppError> {
    let definition = if entity == InventoryEntity::CustomField {
        Some(validate_source_custom_field(record)?)
    } else {
        None
    };
    let mut columns = PreparedColumns::default();
    let sample = Value::Object(record.fields.clone());
    for field in fields {
        bind_mapping(entity, record, field, &sample, &mut columns).map_err(|(_, error)| error)?;
    }
    let source_timestamps = SourceTimestamps::from_record(record)
        .map_err(|_| AppError::Validation("Invalid source timestamps".into()))?;
    prepared_row(
        entity,
        source_provider_id,
        record,
        columns,
        definition,
        source_timestamps,
    )
}

/// A fully resolved row ready to write.
#[derive(Clone)]
struct PlannedRow {
    entity: InventoryEntity,
    source_entity: String,
    external_id: String,
    local_id: Uuid,
    source_timestamps: SourceTimestamps,
    columns: Vec<PlannedColumn>,
    reference_targets: HashMap<&'static str, InventoryEntity>,
    /// Relation name to resolved count, for `sync_job.stats`.
    relations: Vec<(String, usize)>,
    junctions: Vec<PlannedJunction>,
    selectors: Vec<dataflow_selectors::PlannedSelectorRows>,
}

impl PlannedRow {
    fn reference_is_incoming(
        &self,
        column: &str,
        id: Uuid,
        incoming: &HashSet<(InventoryEntity, Uuid)>,
    ) -> bool {
        self.reference_target(column)
            .is_some_and(|entity| incoming.contains(&(entity, id)))
    }

    fn reference_target(&self, column: &str) -> Option<InventoryEntity> {
        self.reference_targets.get(column).copied().or_else(|| {
            self.entity
                .field_iter()
                .find(|field| field.column() == column)
                .and_then(|field| match field.kind() {
                    FieldKind::ForeignKey { target } => target,
                    _ => None,
                })
        })
    }

    fn junction_target(&self, junction: &PlannedJunction) -> Option<InventoryEntity> {
        self.entity
            .field_iter()
            .find_map(|field| match field.kind() {
                FieldKind::Junction {
                    table,
                    local_column,
                    remote_column,
                    target,
                } if table == junction.table
                    && local_column == junction.local_column
                    && remote_column == junction.remote_column =>
                {
                    target
                }
                _ => None,
            })
    }
}

#[derive(Clone)]
struct PlannedColumn {
    column: &'static str,
    value: ColumnValue,
    updatable: bool,
}

#[derive(Clone)]
struct PlannedJunction {
    table: &'static str,
    local_column: &'static str,
    remote_column: &'static str,
    refs: Vec<Uuid>,
}

/// A column value ready to bind.
#[derive(Clone)]
enum ColumnValue {
    Scalar(PgType, InventoryValue),
    Uuid(Option<Uuid>),
    Tags(Vec<String>),
    Literal(&'static str),
    Copy(String),
}

/// Hash the actual resolved values and edges, not only graph observations.
/// The representation is transient and is never logged or persisted.
fn resolved_plan_digest(
    batch: &LocalInventoryBatch,
    planned: &[PlannedRow],
) -> Result<String, AppError> {
    fn timestamp(value: Option<Option<DateTime<chrono::Utc>>>) -> Value {
        match value {
            None => serde_json::json!(["omitted"]),
            Some(None) => serde_json::json!(["null"]),
            Some(Some(value)) => serde_json::json!(["value", value]),
        }
    }
    let mut rows = Vec::with_capacity(planned.len());
    for row in planned {
        let columns: Vec<_> = row
            .columns
            .iter()
            .map(|column| {
                let value = match &column.value {
                    ColumnValue::Scalar(kind, value) => {
                        serde_json::json!(["scalar", format!("{kind:?}"), value])
                    }
                    ColumnValue::Uuid(value) => serde_json::json!(["uuid", value]),
                    ColumnValue::Tags(value) => serde_json::json!(["tags", value]),
                    ColumnValue::Literal(value) => serde_json::json!(["literal", value]),
                    ColumnValue::Copy(value) => serde_json::json!(["copy", value]),
                };
                serde_json::json!([column.column, column.updatable, value])
            })
            .collect();
        let junctions: Vec<_> = row
            .junctions
            .iter()
            .map(|junction| {
                serde_json::json!([
                    junction.table,
                    junction.local_column,
                    junction.remote_column,
                    junction.refs
                ])
            })
            .collect();
        let selectors: Vec<_> = row
            .selectors
            .iter()
            .map(dataflow_selectors::PlannedSelectorRows::digest)
            .collect();
        rows.push(serde_json::json!({
            "entity": row.entity.source_key(),
            "external_id": row.external_id,
            "local_id": row.local_id,
            "source_created": timestamp(row.source_timestamps.created),
            "source_updated": timestamp(row.source_timestamps.updated),
            "columns": columns,
            "relations": row.relations,
            "reference_targets": row.reference_targets.iter().map(|(column, entity)| (*column, entity.source_key())).collect::<BTreeMap<_, _>>(),
            "junctions": junctions,
            "selectors": selectors,
        }));
    }
    let plan = serde_json::json!({
        "provider_id": batch.source_provider_id,
        "extracted_count": batch.extracted_count,
        "tag_membership_count": batch.tag_membership_count,
        "entities_extracted": batch.entities_extracted,
        "rows": rows,
    });
    let bytes = serde_json::to_vec(&plan)
        .map_err(|_| AppError::Internal("inventory_transfer:plan_encoding".into()))?;
    Ok(digest(&bytes))
}

async fn transfer_change_stats(
    transaction: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    tag_memberships: u64,
) -> Result<(TransferReceiptStats, HashSet<(InventoryEntity, Uuid)>), AppError> {
    let mut stats = TransferReceiptStats {
        tag_memberships,
        ..TransferReceiptStats::default()
    };
    let mut changed_rows = HashSet::new();
    for row in planned {
        let table = row
            .entity
            .table()
            .ok_or_else(|| AppError::Internal("inventory_transfer:membership_row".into()))?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
        query
            .push(table)
            .push(" WHERE id=")
            .push_bind(row.local_id)
            .push(")");
        let exists: bool = query
            .build_query_scalar()
            .fetch_one(&mut **transaction)
            .await?;
        if !exists {
            stats.created += 1;
            changed_rows.insert((row.entity, row.local_id));
            continue;
        }
        let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
        query
            .push(table)
            .push(" WHERE id = ")
            .push_bind(row.local_id)
            .push(" AND (");
        let mut count = 0;
        for column in &row.columns {
            if !column.updatable {
                continue;
            }
            if count > 0 {
                query.push(" OR ");
            }
            query.push(column.column).push(" IS DISTINCT FROM ");
            push_column_value(&mut query, &column.value)?;
            count += 1;
        }
        if count == 0 {
            query.push("false");
        }
        query.push("))");
        let mut changed: bool = query
            .build_query_scalar()
            .fetch_one(&mut **transaction)
            .await?;
        for selectors in &row.selectors {
            changed |= dataflow_selectors::changed(transaction, row.local_id, selectors).await?;
        }
        for junction in &row.junctions {
            let mut query = QueryBuilder::<Postgres>::new("SELECT ");
            query
                .push(junction.remote_column)
                .push(" FROM ")
                .push(junction.table)
                .push(" WHERE ")
                .push(junction.local_column)
                .push("=")
                .push_bind(row.local_id)
                .push(" ORDER BY ")
                .push(junction.remote_column);
            let existing: Vec<Uuid> = query
                .build_query_scalar()
                .fetch_all(&mut **transaction)
                .await?;
            let mut desired = junction.refs.clone();
            desired.sort_unstable();
            desired.dedup();
            changed |= existing != desired;
        }
        if changed {
            stats.updated += 1;
            changed_rows.insert((row.entity, row.local_id));
        } else {
            stats.unchanged += 1;
        }
    }
    Ok((stats, changed_rows))
}

struct BindingContext {
    valid: HashMap<(String, String), Uuid>,
    invalid_keys: HashSet<(String, String)>,
    findings: Vec<Finding>,
}

async fn load_bindings(
    transaction: &mut Transaction<'_, Postgres>,
    source_provider_id: Uuid,
) -> Result<BindingContext, AppError> {
    let rows = sqlx::query("SELECT source_entity, external_id, local_id, target_entity FROM sync_inventory_binding WHERE source_provider_id=$1 ORDER BY source_entity, external_id, target_entity")
        .bind(source_provider_id).fetch_all(&mut **transaction).await?;
    let mut context = BindingContext {
        valid: HashMap::new(),
        invalid_keys: HashSet::new(),
        findings: Vec::new(),
    };
    for row in rows {
        let source: String = row.try_get("source_entity")?;
        let target: String = row.try_get("target_entity")?;
        let external_id: String = row.try_get("external_id")?;
        let local_id: Uuid = row.try_get("local_id")?;
        let entity = InventoryEntity::from_source_key(&source);
        if local_id.is_nil() || entity.is_none_or(|entity| entity.target_key() != target) {
            let mut finding =
                Finding::blocker(FindingCode::TargetBindingsInvalid, Capability::TargetGraph);
            finding.entity = entity.map(|entity| entity.source_key().into());
            finding.record_identity_sha256 = Some(digest(external_id.as_bytes()));
            finding.field_path = Some("binding.target_entity".into());
            context.findings.push(finding);
            context.invalid_keys.insert((source, external_id));
        } else {
            context.valid.insert((source, external_id), local_id);
        }
    }
    context
        .valid
        .retain(|key, _| !context.invalid_keys.contains(key));
    Ok(context)
}

async fn content_type_id(
    transaction: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<String, Uuid>,
    token: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Some(cached) = cache.get(token) {
        return Ok(Some(*cached));
    }
    let (app_label, model) = token
        .split_once('.')
        .ok_or_else(|| AppError::Validation("content type token is malformed".to_string()))?;
    let row =
        sqlx::query("SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2")
            .bind(app_label)
            .bind(model)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to resolve a content type: {error}"))
            })?;
    match row {
        Some(row) => {
            let id: Uuid = row.try_get("id")?;
            cache.insert(token.to_string(), id);
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

fn resolve_reference(
    binding: &ReferenceBinding,
    local_ids: &HashMap<(String, String), Uuid>,
) -> Option<Uuid> {
    let target = binding.target?;
    local_ids
        .get(&(target.source_key().to_string(), binding.external_id.clone()))
        .copied()
}

/// Validate and merge a record's source custom-field values into the entity's
/// stored `custom_field_data`, scoped to the entity's content type. Returns
/// `None` for an absent field (preserve); an explicit empty map clears values.
struct DefinitionContext {
    available: HashMap<(String, Uuid), CustomFieldDefinition>,
    by_name: HashMap<String, CustomFieldDefinition>,
    unknown_names: HashSet<String>,
    findings: Vec<Finding>,
    complete: bool,
}

/// Omitted fields preserve existing database values, rather than the offline
/// source defaults. Refuse a definition if that effective projection differs
/// from the validator's security- and value-relevant view.
async fn definition_projection_matches(
    transaction: &mut Transaction<'_, Postgres>,
    row: &PreparedRow,
    id: Option<Uuid>,
    definition: &CustomFieldDefinition,
) -> Result<bool, AppError> {
    let persisted: Option<Value> = if let Some(id) = id {
        sqlx::query_scalar("SELECT to_jsonb(cf) FROM custom_field cf WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
    } else {
        None
    };
    let defaults = serde_json::json!({
        "field_type": "text", "required": false, "enforce_unique": false,
        "is_secret": false, "default_value": null, "validation_minimum": null,
        "validation_maximum": null, "validation_regex": "", "validation_schema": null,
        "choice_set_id": null, "related_object_filter": null
    });
    let effective = persisted.as_ref().unwrap_or(&defaults);
    let expected = serde_json::to_value(definition)
        .map_err(|_| AppError::Internal("inventory_transfer:definition_encoding".into()))?;
    for (source_name, column) in [
        ("type", "field_type"),
        ("required", "required"),
        ("unique", "enforce_unique"),
        ("default", "default_value"),
        ("validation_minimum", "validation_minimum"),
        ("validation_maximum", "validation_maximum"),
        ("validation_regex", "validation_regex"),
        ("validation_schema", "validation_schema"),
        ("related_object_filter", "related_object_filter"),
        ("choice_set", "choice_set_id"),
        ("is_secret", "is_secret"),
    ] {
        if row.scalars.keys().any(|field| field.name() == source_name)
            || row
                .references
                .iter()
                .any(|(field, _)| field.name() == source_name)
            || row.cleared.iter().any(|field| field.name() == source_name)
        {
            continue;
        }
        if effective.get(column) != expected.get(column) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Validate every value independently using the canonical custom-field validator.
/// Unknown incoming scopes/definitions never fall back to an older stored version.
async fn resolve_custom_field_data(
    transaction: &mut Transaction<'_, Postgres>,
    definitions: &DefinitionContext,
    content_types: &mut HashMap<String, Uuid>,
    row: &PreparedRow,
    local_id: Option<Uuid>,
    local_ids: &HashMap<(String, String), Uuid>,
    incoming: &HashSet<(InventoryEntity, Uuid)>,
    dependencies: &mut Vec<DependencyFact>,
    findings: &mut Vec<Finding>,
    complete: &mut bool,
) -> Result<Option<Value>, AppError> {
    let Some(source_fields) = &row.custom_fields else {
        return Ok(None);
    };
    if source_fields.is_empty() {
        return Ok(Some(Value::Object(serde_json::Map::new())));
    }
    let token = row
        .entity
        .content_type()
        .map(|(app, model)| format!("{app}.{model}"));
    let scope = if let Some(token) = token {
        observed_content_type(transaction, content_types, &token).await?
    } else {
        None
    };
    let Some(scope) = scope else {
        for (key, value) in source_fields {
            if let Some(definition) = definitions.by_name.get(key) {
                findings.extend(assess_custom_value(
                    row,
                    &format!("custom_fields.sha256:{}", digest(key.as_bytes())),
                    definition,
                    value,
                ));
            }
            findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some(&format!("custom_fields.sha256:{}", digest(key.as_bytes()))),
                Capability::TargetGraph,
            ));
        }
        *complete = false;
        return Ok(None);
    };
    let mut merged = serde_json::Map::new();
    if let (Some(table), Some(id)) = (row.entity.table(), local_id) {
        let mut query = QueryBuilder::<Postgres>::new("SELECT custom_field_data FROM ");
        query.push(table).push(" WHERE id=").push_bind(id);
        let existing: Option<Value> = query
            .build_query_scalar()
            .fetch_optional(&mut **transaction)
            .await?;
        merged = existing
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
    }
    for (key, value) in source_fields {
        let field = format!("custom_fields.sha256:{}", digest(key.as_bytes()));
        let definition = if let Some(definition) = definitions.available.get(&(key.clone(), scope))
        {
            Some(definition.clone())
        } else if let Some(definition) = definitions.by_name.get(key) {
            findings.extend(assess_custom_value(row, &field, definition, value));
            let unknown = definitions.unknown_names.contains(key);
            findings.push(prepared_finding(
                row,
                if unknown {
                    FindingCode::AnalysisDependencyUnavailable
                } else {
                    FindingCode::CustomFieldMissing
                },
                Some(&field),
                Capability::TargetGraph,
            ));
            if unknown {
                *complete = false;
            }
            continue;
        } else {
            match load_custom_field_definition(transaction, key, scope).await {
                Ok(definition) => definition,
                Err(AppError::Validation(_)) => {
                    findings.push(prepared_finding(
                        row,
                        FindingCode::TargetDefinitionInvalid,
                        Some(&field),
                        Capability::TargetGraph,
                    ));
                    *complete = false;
                    continue;
                }
                Err(error) => return Err(error),
            }
        };
        let Some(definition) = definition else {
            findings.push(prepared_finding(
                row,
                FindingCode::CustomFieldMissing,
                Some(&field),
                Capability::TargetGraph,
            ));
            continue;
        };
        let converted = match custom_field_references::resolve_value(
            transaction,
            row,
            &definition,
            value,
            local_ids,
            incoming,
            &field,
            dependencies,
        )
        .await
        {
            Ok(value) => value,
            Err(AppError::Validation(_)) => {
                if definition.is_secret {
                    findings.push(prepared_finding(
                        row,
                        FindingCode::CustomFieldSecret,
                        Some(&field),
                        Capability::TargetGraph,
                    ));
                }
                findings.push(prepared_finding(
                    row,
                    FindingCode::CustomFieldValueInvalid,
                    Some(&field),
                    Capability::TargetGraph,
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        findings.extend(assess_custom_value(row, &field, &definition, &converted));
        if definition.id.is_nil() {
            findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some(&field),
                Capability::TargetGraph,
            ));
            *complete = false;
        }
        merged.insert(key.clone(), converted);
    }
    Ok(Some(Value::Object(merged)))
}

fn assess_custom_value(
    row: &PreparedRow,
    field: &str,
    definition: &CustomFieldDefinition,
    value: &Value,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    if definition.is_secret {
        findings.push(prepared_finding(
            row,
            FindingCode::CustomFieldSecret,
            Some(field),
            Capability::TargetGraph,
        ));
    }
    if validate_value(definition, value, &[]).is_err() {
        findings.push(prepared_finding(
            row,
            FindingCode::CustomFieldValueInvalid,
            Some(field),
            Capability::TargetGraph,
        ));
    }
    for finding in &mut findings {
        finding.edge = Some(
            crate::contexts::sync::domain::inventory_preflight::EdgeContext {
                target_entity: Some(InventoryEntity::CustomField.source_key().into()),
                target_identity_sha256: None,
                target_local_id: (!definition.id.is_nil()).then_some(definition.id),
            },
        );
    }
    findings
}

/// Resolve all incoming definition scopes/conflicts. Persisted binding UUIDs,
/// not derived candidate UUIDs, identify definitions on rerun.
async fn prepare_custom_field_definitions(
    batch: &LocalInventoryBatch,
    local_ids: &HashMap<(String, String), Uuid>,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<DefinitionContext, AppError> {
    let mut context = DefinitionContext {
        available: HashMap::new(),
        by_name: HashMap::new(),
        unknown_names: HashSet::new(),
        findings: Vec::new(),
        complete: true,
    };
    for row in batch
        .rows
        .iter()
        .filter(|row| row.entity == InventoryEntity::CustomField)
    {
        let Some(definition) = &row.prepared_definition else {
            context.findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some("definition"),
                Capability::TargetGraph,
            ));
            context.complete = false;
            continue;
        };
        let mut scopes = Vec::new();
        let mut unknown_scope = false;
        for (field, refs) in &row.junctions {
            if field.name() != "object_types" {
                continue;
            }
            for (index, binding) in refs.iter().enumerate() {
                let scope = if let Some(token) = &binding.type_token {
                    observed_content_type(transaction, content_types, token).await?
                } else {
                    None
                };
                if let Some(id) = scope {
                    scopes.push(id);
                } else {
                    let mut finding = prepared_finding(
                        row,
                        FindingCode::CustomFieldScopeInvalid,
                        Some(&format!("object_types[{index}]")),
                        Capability::TargetGraph,
                    );
                    finding.edge = Some(
                        crate::contexts::sync::domain::inventory_preflight::EdgeContext {
                            target_entity: None,
                            target_identity_sha256: binding
                                .type_token
                                .as_ref()
                                .map(|token| digest(token.as_bytes())),
                            target_local_id: None,
                        },
                    );
                    context.findings.push(finding);
                    unknown_scope = true;
                }
            }
        }
        let local_id = local_ids
            .get(&(row.source_entity.clone(), row.external_id.clone()))
            .copied();
        if local_id.is_none() {
            context.findings.push(prepared_finding(
                row,
                FindingCode::AnalysisDependencyUnavailable,
                Some("identity"),
                Capability::TargetGraph,
            ));
            context.complete = false;
        }
        let existing = sqlx::query("SELECT id, is_secret FROM custom_field WHERE name=$1")
            .bind(&definition.name)
            .fetch_optional(&mut **transaction)
            .await?;
        if let Some(existing) = existing {
            let existing_id: Uuid = existing.try_get("id")?;
            let existing_secret: bool = existing.try_get("is_secret")?;
            if local_id.is_some_and(|id| id != existing_id) {
                let mut finding = prepared_finding(
                    row,
                    FindingCode::TargetDefinitionConflict,
                    Some("name"),
                    Capability::TargetGraph,
                );
                finding.edge = Some(
                    crate::contexts::sync::domain::inventory_preflight::EdgeContext {
                        target_entity: Some(row.source_entity.clone()),
                        target_identity_sha256: None,
                        target_local_id: Some(existing_id),
                    },
                );
                context.findings.push(finding);
            }
            if existing_secret && !definition.is_secret {
                context.findings.push(prepared_finding(
                    row,
                    FindingCode::CustomFieldSecret,
                    Some("is_secret"),
                    Capability::TargetGraph,
                ));
            }
        }
        if local_id.is_none() || unknown_scope {
            context.unknown_names.insert(definition.name.clone());
        }
        let mut resolved_definition = definition.clone();
        resolved_definition.id = local_id.unwrap_or_else(Uuid::nil);
        if !definition_projection_matches(transaction, row, local_id, &resolved_definition).await? {
            context.findings.push(prepared_finding(
                row,
                FindingCode::TargetDefinitionInvalid,
                Some("definition.projection"),
                Capability::TargetGraph,
            ));
            context.unknown_names.insert(definition.name.clone());
            context.complete = false;
            continue;
        }
        match custom_field_references::resolve_definition_type(
            transaction,
            content_types,
            row,
            &mut resolved_definition,
        )
        .await
        {
            Ok(()) => {}
            Err(AppError::Validation(_)) => {
                context.findings.push(prepared_finding(
                    row,
                    FindingCode::TargetDefinitionInvalid,
                    Some("related_object_type"),
                    Capability::TargetGraph,
                ));
                context.unknown_names.insert(definition.name.clone());
                context.complete = false;
                continue;
            }
            Err(error) => return Err(error),
        }
        context
            .by_name
            .insert(definition.name.clone(), resolved_definition.clone());
        for scope in scopes {
            context.available.insert(
                (definition.name.clone(), scope),
                resolved_definition.clone(),
            );
        }
    }
    Ok(context)
}

async fn load_custom_field_definition(
    transaction: &mut Transaction<'_, Postgres>,
    name: &str,
    content_type_id: Uuid,
) -> Result<Option<CustomFieldDefinition>, AppError> {
    let row = sqlx::query(
        "SELECT cf.id, cf.name, cf.label, cf.field_type, cf.description, cf.group_name, \
         cf.required, cf.enforce_unique, cf.is_secret, cf.default_value, cf.weight, \
         cf.validation_minimum::float8 AS validation_minimum, \
         cf.validation_maximum::float8 AS validation_maximum, cf.validation_regex, \
         cf.validation_schema, cf.choice_set_id, cf.related_object_type_id, \
         cf.related_object_filter, cf.ui_visible, cf.ui_editable, cf.is_cloneable, \
         cf.search_weight, cf.filter_logic, cf.created_at, cf.updated_at \
         FROM custom_field cf \
         JOIN custom_field_object_type cfot ON cfot.custom_field_id = cf.id \
         WHERE cf.name = $1 AND cfot.content_type_id = $2",
    )
    .bind(name)
    .bind(content_type_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| AppError::Database(format!("Failed to load a custom field: {error}")))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let field_type_raw: String = row.try_get("field_type")?;
    let field_type = CustomFieldType::parse(&field_type_raw).ok_or_else(|| {
        AppError::Validation(format!("custom field '{name}' has an unknown type"))
    })?;
    let ui_visible_raw: String = row.try_get("ui_visible")?;
    let ui_editable_raw: String = row.try_get("ui_editable")?;
    let filter_logic_raw: String = row.try_get("filter_logic")?;
    let definition = CustomFieldDefinition {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        label: row.try_get("label")?,
        field_type,
        description: row.try_get("description")?,
        group_name: row.try_get("group_name")?,
        required: row.try_get("required")?,
        enforce_unique: row.try_get("enforce_unique")?,
        is_secret: row.try_get("is_secret")?,
        default_value: row.try_get("default_value")?,
        weight: row.try_get("weight")?,
        validation_minimum: row.try_get("validation_minimum")?,
        validation_maximum: row.try_get("validation_maximum")?,
        validation_regex: row.try_get("validation_regex")?,
        validation_schema: row.try_get("validation_schema")?,
        choice_set_id: row.try_get("choice_set_id")?,
        related_object_type_id: row.try_get("related_object_type_id")?,
        related_object_filter: row.try_get("related_object_filter")?,
        ui_visible: UiVisible::parse(&ui_visible_raw).ok_or_else(|| {
            AppError::Database(format!("custom field '{name}' has an invalid ui_visible"))
        })?,
        ui_editable: UiEditable::parse(&ui_editable_raw).ok_or_else(|| {
            AppError::Database(format!("custom field '{name}' has an invalid ui_editable"))
        })?,
        is_cloneable: row.try_get("is_cloneable")?,
        search_weight: row.try_get("search_weight")?,
        filter_logic: FilterLogic::parse(&filter_logic_raw).ok_or_else(|| {
            AppError::Database(format!("custom field '{name}' has an invalid filter_logic"))
        })?,
        object_types: Vec::new(),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    };
    validate_definition(&definition)?;
    Ok(Some(definition))
}

/// Validate an imported custom-field definition with the canonical checker
/// before it is written, instead of trusting the provider shape.
fn validate_source_custom_field(record: &SourceRecord) -> Result<CustomFieldDefinition, AppError> {
    fn text(record: &SourceRecord, name: &str) -> Option<String> {
        record
            .fields
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
    fn choice(record: &SourceRecord, name: &str) -> Option<String> {
        match record.fields.get(name) {
            Some(Value::Object(map)) => {
                map.get("value").and_then(Value::as_str).map(str::to_string)
            }
            Some(Value::String(value)) => Some(value.clone()),
            _ => None,
        }
    }
    fn flag(record: &SourceRecord, name: &str) -> bool {
        record
            .fields
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
    fn integer(record: &SourceRecord, name: &str) -> i32 {
        record
            .fields
            .get(name)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0)
    }

    let name = text(record, "name").unwrap_or_default();
    let field_type_raw = choice(record, "type")
        .or_else(|| text(record, "data_type"))
        .unwrap_or_default();
    let field_type = CustomFieldType::parse(&field_type_raw).ok_or_else(|| {
        AppError::Validation(format!("custom field '{name}' has an unknown type"))
    })?;
    let definition = CustomFieldDefinition {
        id: Uuid::nil(),
        name: name.clone(),
        label: text(record, "label").unwrap_or_default(),
        field_type,
        description: text(record, "description").unwrap_or_default(),
        group_name: text(record, "group_name").unwrap_or_default(),
        required: flag(record, "required"),
        enforce_unique: flag(record, "unique"),
        is_secret: flag(record, "is_secret"),
        default_value: record
            .fields
            .get("default")
            .filter(|value| !value.is_null())
            .cloned(),
        weight: integer(record, "weight"),
        validation_minimum: record
            .fields
            .get("validation_minimum")
            .and_then(Value::as_f64),
        validation_maximum: record
            .fields
            .get("validation_maximum")
            .and_then(Value::as_f64),
        validation_regex: text(record, "validation_regex").unwrap_or_default(),
        validation_schema: record
            .fields
            .get("validation_schema")
            .filter(|value| !value.is_null())
            .cloned(),
        choice_set_id: None,
        related_object_type_id: custom_field_references::source_related_type(record)?,
        related_object_filter: None,
        ui_visible: choice(record, "ui_visible")
            .and_then(|value| UiVisible::parse(&value))
            .unwrap_or(UiVisible::Always),
        ui_editable: choice(record, "ui_editable")
            .and_then(|value| UiEditable::parse(&value))
            .unwrap_or(UiEditable::Yes),
        is_cloneable: flag(record, "is_cloneable"),
        search_weight: integer(record, "search_weight"),
        filter_logic: choice(record, "filter_logic")
            .and_then(|value| FilterLogic::parse(&value))
            .unwrap_or(FilterLogic::Loose),
        object_types: Vec::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    validate_definition(&definition).map_err(|error| {
        AppError::Validation(format!(
            "custom field '{name}' definition is invalid: {error}"
        ))
    })?;
    Ok(definition)
}

async fn plan_row(
    row: &PreparedRow,
    local_ids: &HashMap<(String, String), Uuid>,
    merged_custom_fields: Option<Value>,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<PlannedRow, AppError> {
    let local_id = local_ids
        .get(&(row.source_entity.clone(), row.external_id.clone()))
        .copied()
        .ok_or_else(|| AppError::Internal("Prepared row has no allocated identity".into()))?;
    let mut columns: Vec<PlannedColumn> = Vec::new();
    for (field, value) in &row.scalars {
        columns.push(PlannedColumn {
            column: field.column(),
            value: ColumnValue::Scalar(field.sql_type(), value.clone()),
            updatable: true,
        });
    }
    let mut relations = Vec::new();
    append_reference_columns(
        row,
        local_ids,
        content_types,
        transaction,
        &mut columns,
        &mut relations,
    )
    .await?;
    for field in &row.cleared {
        // Explicit null clears the whole reference: a generic reference clears
        // its type column and its id column together.
        if let FieldKind::GenericReference { type_column, .. } = field.kind() {
            columns.push(PlannedColumn {
                column: type_column,
                value: ColumnValue::Uuid(None),
                updatable: true,
            });
        }
        columns.push(PlannedColumn {
            column: field.column(),
            value: ColumnValue::Uuid(None),
            updatable: true,
        });
    }
    if let Some(tags) = &row.tags {
        let ids = tags
            .iter()
            .map(|tag| {
                local_ids
                    .get(&(tag.entity.to_string(), tag.external_id.clone()))
                    .map(Uuid::to_string)
                    .ok_or_else(|| AppError::Validation("unresolved tag provider identity".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        columns.push(PlannedColumn {
            column: "tags",
            value: ColumnValue::Tags(ids),
            updatable: true,
        });
    }
    for &(column, kind, argument) in row.entity.internal_columns() {
        columns.push(internal_column(row, column, kind, argument)?);
    }
    // Native optional text has no database default on several admitted models.
    // Supply its catalogue-approved empty INSERT value without overwriting an
    // omitted UPDATE attribute. PostgreSQL checks NOT NULL before ON CONFLICT.
    for field in row.entity.field_iter().filter(|field| {
        matches!(field.kind(), FieldKind::Scalar)
            && !field.nullable()
            && field.spec().default.is_none()
            && field.absent() == Absent::EmptyText
    }) {
        if !columns.iter().any(|column| column.column == field.column()) {
            columns.push(PlannedColumn {
                column: field.column(),
                value: ColumnValue::Scalar(
                    field.sql_type(),
                    InventoryValue::Text {
                        value: String::new(),
                    },
                ),
                updatable: false,
            });
        }
    }
    apply_custom_field_data(&mut columns, merged_custom_fields);
    let mut junctions = Vec::new();
    for (field, refs) in &row.junctions {
        match field.kind() {
            FieldKind::Junction {
                table,
                local_column,
                remote_column,
                ..
            } => {
                let resolved: Vec<Uuid> = refs
                    .iter()
                    .map(|binding| {
                        resolve_reference(binding, local_ids).ok_or_else(|| {
                            AppError::Validation(format!(
                                "Unresolved relation {}::{}",
                                row.source_entity,
                                field.name()
                            ))
                        })
                    })
                    .collect::<Result<_, _>>()?;
                junctions.push(PlannedJunction {
                    table,
                    local_column,
                    remote_column,
                    refs: resolved,
                });
            }
            FieldKind::ContentTypeJunction {
                table,
                local_column,
                remote_column,
            } => {
                let mut resolved = Vec::with_capacity(refs.len());
                for binding in refs {
                    let token = binding.type_token.as_deref().ok_or_else(|| {
                        AppError::Validation("Content-type scope has no type".into())
                    })?;
                    let id = content_type_id(transaction, content_types, token)
                        .await?
                        .ok_or_else(|| {
                            AppError::Validation(format!(
                                "Unresolved scope {}::{}",
                                row.source_entity,
                                field.name()
                            ))
                        })?;
                    resolved.push(id);
                }
                junctions.push(PlannedJunction {
                    table,
                    local_column,
                    remote_column,
                    refs: resolved,
                });
            }
            _ => {}
        }
    }
    let mut selectors = Vec::with_capacity(row.selectors.len());
    for (field, refs) in &row.selectors {
        selectors.push(dataflow_selectors::plan(field.spec(), refs, local_ids)?);
    }
    Ok(PlannedRow {
        entity: row.entity,
        source_entity: row.source_entity.clone(),
        external_id: row.external_id.clone(),
        local_id,
        source_timestamps: row.source_timestamps,
        columns,
        reference_targets: row
            .references
            .iter()
            .filter_map(|(field, binding)| binding.target.map(|target| (field.column(), target)))
            .collect(),
        relations,
        junctions,
        selectors,
    })
}

/// Append the planned columns and relation statistics for a row's non-null
/// references, resolving each against the local bindings.
async fn append_reference_columns(
    row: &PreparedRow,
    local_ids: &HashMap<(String, String), Uuid>,
    content_types: &mut HashMap<String, Uuid>,
    transaction: &mut Transaction<'_, Postgres>,
    columns: &mut Vec<PlannedColumn>,
    relations: &mut Vec<(String, usize)>,
) -> Result<(), AppError> {
    for (field, binding) in &row.references {
        if row.entity == InventoryEntity::CustomField && field.name() == "related_object_type" {
            let token = binding.type_token.as_deref().ok_or_else(|| {
                AppError::Validation("custom field related type is missing".into())
            })?;
            let id = content_type_id(transaction, content_types, token)
                .await?
                .ok_or_else(|| {
                    AppError::Validation("custom field related type is unavailable".into())
                })?;
            columns.push(PlannedColumn {
                column: field.column(),
                value: ColumnValue::Uuid(Some(id)),
                updatable: true,
            });
            relations.push((field.name().to_string(), 1));
            continue;
        }
        let resolved = resolve_reference(binding, local_ids).ok_or_else(|| {
            AppError::Validation(format!(
                "relation {}::{} cannot be resolved to a local row",
                row.entity.source_key(),
                field.name()
            ))
        })?;
        if let Some(token) = &binding.type_token {
            let content_type = content_type_id(transaction, content_types, token)
                .await?
                .ok_or_else(|| {
                    AppError::Validation(format!(
                        "required content type for {}::{} is not seeded",
                        row.entity.source_key(),
                        field.name()
                    ))
                })?;
            if let FieldKind::GenericReference { type_column, .. } = field.kind() {
                columns.push(PlannedColumn {
                    column: type_column,
                    value: ColumnValue::Uuid(Some(content_type)),
                    updatable: true,
                });
            }
        }
        columns.push(PlannedColumn {
            column: field.column(),
            value: ColumnValue::Uuid(Some(resolved)),
            updatable: true,
        });
        relations.push((field.name().to_string(), 1));
    }
    Ok(())
}

/// A synthetic column the target must add for a `NOT NULL`-without-default
/// column that no mapped field writes.
fn internal_column(
    row: &PreparedRow,
    column: &'static str,
    kind: &str,
    argument: &'static str,
) -> Result<PlannedColumn, AppError> {
    Ok(match kind {
        "literal" => PlannedColumn {
            column,
            value: ColumnValue::Literal(argument),
            updatable: false,
        },
        "copy" => {
            let copied = row
                .entity
                .field(argument)
                .and_then(|field| row.scalars.get(&field))
                .map(text_of);
            PlannedColumn {
                column,
                updatable: row.entity != InventoryEntity::InterfaceTemplate || copied.is_some(),
                value: ColumnValue::Copy(copied.unwrap_or_default()),
            }
        }
        other => {
            return Err(AppError::Internal(format!(
                "unknown internal column kind {other}"
            )));
        }
    })
}

/// Write the merged `custom_field_data` jsonb, replacing an existing column in
/// place or appending it when none was planned.
fn apply_custom_field_data(columns: &mut Vec<PlannedColumn>, merged: Option<Value>) {
    let Some(merged) = merged else {
        return;
    };
    let mut replaced = false;
    for column in columns.iter_mut() {
        if column.column == "custom_field_data" {
            column.value = ColumnValue::Scalar(
                PgType::Jsonb,
                InventoryValue::Json {
                    value: merged.clone(),
                },
            );
            column.updatable = true;
            replaced = true;
        }
    }
    if !replaced {
        columns.push(PlannedColumn {
            column: "custom_field_data",
            value: ColumnValue::Scalar(PgType::Jsonb, InventoryValue::Json { value: merged }),
            updatable: true,
        });
    }
}

async fn upsert_binding(
    transaction: &mut Transaction<'_, Postgres>,
    source_provider_id: Uuid,
    plan: &PlannedRow,
    job_id: Uuid,
) -> Result<(), AppError> {
    write_binding(
        transaction,
        &InventoryBinding {
            identity: InventoryIdentity {
                provider_id: source_provider_id,
                entity: plan.entity,
                external_id: plan.external_id.clone(),
            },
            local_id: plan.local_id,
        },
        Some(job_id),
        Some(plan.source_timestamps),
    )
    .await
}

async fn upsert_row(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &PlannedRow,
) -> Result<(), AppError> {
    if matches!(
        plan.entity,
        InventoryEntity::Device | InventoryEntity::Module
    ) {
        // Snapshot rows own imported identities and their separately mapped
        // components. A source hardware INSERT is not a native template-create
        // request, even when a component selection is empty or unsupported.
        prepare_explicit_hardware_replay(transaction).await?;
    }
    let Some(table) = plan.entity.table() else {
        return Err(AppError::Internal(
            "a membership resource cannot be written as a row".to_string(),
        ));
    };
    if plan.entity == InventoryEntity::Module {
        // The INSERT guard validates a new installation even on ON CONFLICT.
        // A metadata update of an existing occupant in a now-disabled bay must
        // instead take the native UPDATE path. Replay intent above acquires
        // graph/catalog locks before this row lock and missing-row decision.
        let existing: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM dcim_module WHERE id=$1 FOR UPDATE")
                .bind(plan.local_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(crate::errors::classify_sqlx_error_owned)?;
        if existing.is_some() {
            return update_existing_module(transaction, plan).await;
        }
    }
    let mut builder: QueryBuilder<Postgres> = QueryBuilder::new("INSERT INTO ");
    builder.push(table);
    builder.push(" (id");
    let mut updatable: Vec<&'static str> = Vec::new();
    for column in &plan.columns {
        builder.push(", ").push(column.column);
        if column.updatable {
            updatable.push(column.column);
        }
    }
    builder.push(") VALUES (");
    builder.push_bind(plan.local_id);
    for column in &plan.columns {
        builder.push(", ");
        push_column_value(&mut builder, &column.value)?;
    }
    builder.push(")");
    if plan.entity != InventoryEntity::Module {
        if updatable.is_empty() {
            builder.push(" ON CONFLICT (id) DO NOTHING");
        } else {
            builder.push(" ON CONFLICT (id) DO UPDATE SET ");
            for (index, column) in updatable.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.push(*column).push(" = EXCLUDED.").push(*column);
            }
            builder.push(" WHERE (");
            for (index, column) in updatable.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.push(table).push(".").push(*column);
            }
            builder.push(") IS DISTINCT FROM (");
            for (index, column) in updatable.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.push("EXCLUDED.").push(*column);
            }
            builder.push(")");
        }
    }
    builder
        .build()
        .execute(&mut **transaction)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    Ok(())
}

async fn update_existing_module(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &PlannedRow,
) -> Result<(), AppError> {
    let Some(mut builder) = module_update_query(plan)? else {
        return Ok(());
    };
    builder
        .build()
        .execute(&mut **transaction)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
    Ok(())
}

/// Build synchronously so borrowed column iterators and predicates cannot enter
/// the async state that the commit and webhook owners require to be Send.
fn module_update_query<'a>(
    plan: &'a PlannedRow,
) -> Result<Option<QueryBuilder<Postgres>>, AppError> {
    let mut builder: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE dcim_module SET ");
    let mut assignments = 0;
    for column in &plan.columns {
        if !column.updatable {
            continue;
        }
        if assignments > 0 {
            builder.push(", ");
        }
        builder.push(column.column).push(" = ");
        push_column_value(&mut builder, &column.value)?;
        assignments += 1;
    }
    if assignments == 0 {
        return Ok(None);
    }
    builder.push(" WHERE id = ").push_bind(plan.local_id);
    builder.push(" AND (");
    let mut compared = 0;
    for column in &plan.columns {
        if !column.updatable {
            continue;
        }
        if compared > 0 {
            builder.push(" OR ");
        }
        builder.push(column.column).push(" IS DISTINCT FROM ");
        push_column_value(&mut builder, &column.value)?;
        compared += 1;
    }
    builder.push(")");
    Ok(Some(builder))
}

async fn replace_junctions(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &PlannedRow,
) -> Result<(), AppError> {
    for junction in &plan.junctions {
        if !junction_changed(transaction, plan.local_id, junction).await? {
            continue;
        }
        clear_junction(transaction, plan.local_id, junction).await?;
        populate_junction(transaction, plan.local_id, junction).await?;
    }
    Ok(())
}

/// Replace the ordered selector rows of every `FieldKind::SelectorRows` field.
async fn replace_selectors(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &PlannedRow,
) -> Result<(), AppError> {
    for selectors in &plan.selectors {
        dataflow_selectors::replace(transaction, plan.local_id, selectors).await?;
    }
    Ok(())
}

async fn junction_changed(
    transaction: &mut Transaction<'_, Postgres>,
    local_id: Uuid,
    junction: &PlannedJunction,
) -> Result<bool, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(junction.remote_column)
        .push(" FROM ")
        .push(junction.table)
        .push(" WHERE ")
        .push(junction.local_column)
        .push(" = ")
        .push_bind(local_id)
        .push(" ORDER BY ")
        .push(junction.remote_column);
    let current: Vec<Uuid> = query
        .build_query_scalar()
        .fetch_all(&mut **transaction)
        .await?;
    let mut desired = junction.refs.clone();
    desired.sort_unstable();
    desired.dedup();
    Ok(current != desired)
}

async fn clear_junction(
    transaction: &mut Transaction<'_, Postgres>,
    local_id: Uuid,
    junction: &PlannedJunction,
) -> Result<(), AppError> {
    let mut query = QueryBuilder::<Postgres>::new("DELETE FROM ");
    query
        .push(junction.table)
        .push(" WHERE ")
        .push(junction.local_column)
        .push(" = ")
        .push_bind(local_id)
        .push(" AND NOT (")
        .push(junction.remote_column)
        .push(" = ANY(")
        .push_bind(junction.refs.as_slice())
        .push("::uuid[]))");
    query
        .build()
        .execute(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to clear a relation: {error}")))?;
    Ok(())
}

async fn populate_junction(
    transaction: &mut Transaction<'_, Postgres>,
    local_id: Uuid,
    junction: &PlannedJunction,
) -> Result<(), AppError> {
    if junction.refs.is_empty() {
        return Ok(());
    }
    // One complete set per statement: native AFTER ROW validators must observe
    // a shared later member as well as a nonmatching first member. The array
    // bind also avoids a parameter-per-member limit for large source sets.
    let mut insert = QueryBuilder::<Postgres>::new("INSERT INTO ");
    insert
        .push(junction.table)
        .push(" (")
        .push(junction.local_column)
        .push(", ")
        .push(junction.remote_column)
        .push(") SELECT ")
        .push_bind(local_id)
        .push(", member FROM unnest(")
        .push_bind(junction.refs.as_slice())
        .push("::uuid[]) AS members(member) WHERE NOT EXISTS (SELECT 1 FROM ")
        .push(junction.table)
        .push(" existing WHERE existing.")
        .push(junction.local_column)
        .push(" = ")
        .push_bind(local_id)
        .push(" AND existing.")
        .push(junction.remote_column)
        .push(" = members.member)");
    insert
        .build()
        .execute(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to write a relation: {error}")))?;
    Ok(())
}

/// Recompute `tree_id`/`level`/`lft`/`rght` for one nested-set table from the
/// whole parent graph. A row whose parent is absent (dangling or root) becomes
/// a root; a cycle is rejected. Existing rows are preserved — only the
/// bookkeeping columns change.
async fn recompute_tree(
    transaction: &mut Transaction<'_, Postgres>,
    entity: InventoryEntity,
) -> Result<(), AppError> {
    let table = entity
        .table()
        .ok_or_else(|| AppError::Internal("entity has no table".to_string()))?;
    let parent_of = load_parent_graph(transaction, table).await?;
    let (children, roots) = build_forest_hierarchy(&parent_of);
    let bookkeeping = walk_forest(&roots, &children, parent_of.len())?;
    write_tree_bookkeeping(transaction, table, &bookkeeping).await
}

/// Read a nested-set table's `(id, parent_id)` graph.
async fn load_parent_graph(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
) -> Result<HashMap<Uuid, Option<Uuid>>, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT id, parent_id FROM ");
    query.push(table);
    let rows = query
        .build()
        .fetch_all(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to read a hierarchy: {error}")))?;
    let mut parent_of: HashMap<Uuid, Option<Uuid>> = HashMap::with_capacity(rows.len());
    for row in &rows {
        let id: Uuid = row.try_get("id")?;
        let parent: Option<Uuid> = row.try_get("parent_id")?;
        parent_of.insert(id, parent);
    }
    Ok(parent_of)
}

/// Build the child adjacency and the sorted root list from a parent graph. A
/// parent absent from the graph (dangling or null) makes its row a root.
fn build_forest_hierarchy(
    parent_of: &HashMap<Uuid, Option<Uuid>>,
) -> (HashMap<Uuid, Vec<Uuid>>, Vec<Uuid>) {
    let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut roots: Vec<Uuid> = Vec::new();
    for (&id, parent) in parent_of {
        match (*parent).filter(|value| parent_of.contains_key(value)) {
            Some(parent) => children.entry(parent).or_default().push(id),
            None => roots.push(id),
        }
    }
    roots.sort_unstable();
    for list in children.values_mut() {
        list.sort_unstable();
    }
    (children, roots)
}

/// The nested-set bookkeeping assigned to every row of one table.
struct TreeBookkeeping {
    tree_of: HashMap<Uuid, i32>,
    level_of: HashMap<Uuid, i32>,
    lft_of: HashMap<Uuid, i32>,
    rght_of: HashMap<Uuid, i32>,
}

/// Assign `tree_id`/`level`/`lft`/`rght` by walking each root's subtree. A
/// revisit proves a cycle, as does a walk that does not reach every row.
fn walk_forest(
    roots: &[Uuid],
    children: &HashMap<Uuid, Vec<Uuid>>,
    row_count: usize,
) -> Result<TreeBookkeeping, AppError> {
    let mut bookkeeping = TreeBookkeeping {
        tree_of: HashMap::with_capacity(row_count),
        level_of: HashMap::new(),
        lft_of: HashMap::new(),
        rght_of: HashMap::new(),
    };
    let mut counter: i32 = 1;
    for (index, root) in roots.iter().enumerate() {
        let tree_id = i32::try_from(index + 1)
            .map_err(|_| AppError::Validation("hierarchy is too large".to_string()))?;
        walk_subtree(*root, tree_id, children, &mut bookkeeping, &mut counter)?;
    }
    if bookkeeping.tree_of.len() != row_count {
        return Err(AppError::Validation(
            "hierarchy contains a cycle".to_string(),
        ));
    }
    Ok(bookkeeping)
}

/// Walk one root's subtree depth-first, numbering the nested-set columns and
/// rejecting a row reached twice (a cycle).
fn walk_subtree(
    root: Uuid,
    tree_id: i32,
    children: &HashMap<Uuid, Vec<Uuid>>,
    bookkeeping: &mut TreeBookkeeping,
    counter: &mut i32,
) -> Result<(), AppError> {
    let mut stack: Vec<(Uuid, i32, bool)> = vec![(root, 0, false)];
    while let Some((id, depth, exiting)) = stack.pop() {
        if exiting {
            bookkeeping.rght_of.insert(id, *counter);
            *counter += 1;
            continue;
        }
        if bookkeeping.tree_of.contains_key(&id) {
            return Err(AppError::Validation(
                "hierarchy contains a cycle".to_string(),
            ));
        }
        bookkeeping.tree_of.insert(id, tree_id);
        bookkeeping.level_of.insert(id, depth);
        bookkeeping.lft_of.insert(id, *counter);
        *counter += 1;
        stack.push((id, depth, true));
        if let Some(kids) = children.get(&id) {
            for child in kids.iter().rev() {
                stack.push((*child, depth + 1, false));
            }
        }
    }
    Ok(())
}

/// Write the computed bookkeeping columns back for every visited row.
async fn write_tree_bookkeeping(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
    bookkeeping: &TreeBookkeeping,
) -> Result<(), AppError> {
    for (id, tree_id) in &bookkeeping.tree_of {
        let mut query = QueryBuilder::<Postgres>::new("UPDATE ");
        query.push(table).push(" SET tree_id = $2, level = $3, lft = $4, rght = $5 WHERE id = $1 AND (tree_id, level, lft, rght) IS DISTINCT FROM ($2, $3, $4, $5)");
        query
            .build()
            .bind(id)
            .bind(*tree_id)
            .bind(bookkeeping.level_of.get(id).copied().unwrap_or(0))
            .bind(bookkeeping.lft_of.get(id).copied().unwrap_or(0))
            .bind(bookkeeping.rght_of.get(id).copied().unwrap_or(0))
            .execute(&mut **transaction)
            .await
            .map_err(|error| AppError::Database(format!("Failed to write a hierarchy: {error}")))?;
    }
    Ok(())
}

async fn update_job(
    transaction: &mut Transaction<'_, Postgres>,
    job: &SyncJob,
) -> Result<SyncJob, AppError> {
    let row = sqlx::query(
        "UPDATE sync_job SET status = $2, started_at = COALESCE(started_at, $3), \
         finished_at = $4, stats = $5 WHERE id = $1 \
         RETURNING id, provider_id, mapping_version, status, started_at, finished_at, stats",
    )
    .bind(job.id)
    .bind(status_to_string(&job.status))
    .bind(job.started_at)
    .bind(job.finished_at)
    .bind(&job.stats)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| AppError::Database(format!("Failed to complete the sync job: {error}")))?;
    let status: String = row.try_get("status")?;
    Ok(SyncJob {
        id: row.try_get("id")?,
        provider_id: row.try_get("provider_id")?,
        mapping_version: row.try_get("mapping_version")?,
        status: parse_status(&status)?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        stats: row.try_get("stats")?,
    })
}

fn status_to_string(status: &SyncStatus) -> String {
    match status {
        SyncStatus::Queued => "queued",
        SyncStatus::Running => "running",
        SyncStatus::Completed => "completed",
        SyncStatus::Failed => "failed",
    }
    .to_string()
}

fn parse_status(status: &str) -> Result<SyncStatus, AppError> {
    match status {
        "queued" => Ok(SyncStatus::Queued),
        "running" => Ok(SyncStatus::Running),
        "completed" => Ok(SyncStatus::Completed),
        "failed" => Ok(SyncStatus::Failed),
        other => Err(AppError::Validation(format!(
            "Unknown sync status '{other}'"
        ))),
    }
}

fn text_of(value: &InventoryValue) -> String {
    match value {
        InventoryValue::Text { value } => value.clone(),
        InventoryValue::Integer { value } => value.to_string(),
        InventoryValue::Number { value } => value.to_string(),
        InventoryValue::Boolean { value } => value.to_string(),
        InventoryValue::Null | InventoryValue::Json { .. } => String::new(),
    }
}

fn push_column_value<'a>(
    builder: &mut QueryBuilder<Postgres>,
    value: &'a ColumnValue,
) -> Result<(), AppError> {
    match value {
        ColumnValue::Literal(sql) => {
            builder.push(*sql);
        }
        ColumnValue::Copy(text) => {
            builder.push_bind(text.as_str());
        }
        ColumnValue::Uuid(Some(uuid)) => {
            builder.push_bind(*uuid);
        }
        ColumnValue::Uuid(None) => {
            builder.push("NULL");
        }
        ColumnValue::Tags(tags) => {
            builder.push_bind(tags.clone());
        }
        ColumnValue::Scalar(sql_type, scalar) => push_scalar(builder, *sql_type, scalar)?,
    }
    Ok(())
}

fn json_string_array(value: &Value) -> Result<Vec<String>, AppError> {
    value
        .as_array()
        .ok_or_else(|| AppError::Validation("expected a text array value".to_string()))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| AppError::Validation("array entry is not text".to_string()))
        })
        .collect()
}

fn json_int_array(value: &Value) -> Result<Vec<i32>, AppError> {
    value
        .as_array()
        .ok_or_else(|| AppError::Validation("expected an integer array value".to_string()))?
        .iter()
        .map(|item| {
            let number = item
                .as_i64()
                .ok_or_else(|| AppError::Validation("array entry is not an integer".to_string()))?;
            i32::try_from(number).map_err(|_| {
                AppError::Validation("array entry exceeds the integer range".to_string())
            })
        })
        .collect()
}

fn push_scalar<'a>(
    builder: &mut QueryBuilder<Postgres>,
    sql_type: PgType,
    value: &'a InventoryValue,
) -> Result<(), AppError> {
    match (sql_type, value) {
        (_, InventoryValue::Null) => {
            builder.push("NULL");
        }
        (PgType::Bool, InventoryValue::Boolean { value }) => {
            builder.push_bind(*value);
        }
        (PgType::Integer | PgType::SmallInt, InventoryValue::Integer { value }) => {
            let narrowed = i32::try_from(*value)
                .map_err(|_| AppError::Validation("integer value is out of range".to_string()))?;
            builder.push_bind(narrowed);
        }
        (PgType::BigInt, InventoryValue::Integer { value }) => {
            builder.push_bind(*value);
        }
        (PgType::Numeric, InventoryValue::Number { value }) => {
            // Bind the decimal representation directly, not a float8 parameter
            // whose server-side numeric conversion may change the value.
            if !value.is_finite() {
                return Err(AppError::Validation("numeric value must be finite".into()));
            }
            builder.push_bind(value.to_string()).push("::numeric");
        }
        (
            PgType::Numeric,
            InventoryValue::Json {
                value: Value::Number(value),
            },
        ) => {
            builder.push_bind(value.to_string()).push("::numeric");
        }
        (PgType::Double | PgType::Real, InventoryValue::Number { value }) => {
            builder.push_bind(*value);
        }
        (PgType::Text, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
        }
        (PgType::Date, InventoryValue::Text { value }) => {
            let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| AppError::Validation("invalid calendar date".into()))?;
            builder.push_bind(date);
        }
        (PgType::Uuid, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::uuid");
        }
        (PgType::Inet, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::inet");
        }
        (PgType::Cidr, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::cidr");
        }
        (PgType::MacAddr, InventoryValue::Text { value }) => {
            builder.push_bind(value.as_str());
            builder.push("::macaddr");
        }
        (PgType::Jsonb, InventoryValue::Json { value }) => {
            builder.push_bind(value);
        }
        (PgType::Json, InventoryValue::Json { value }) => {
            builder.push_bind(value);
            builder.push("::json");
        }
        (PgType::TextArray, InventoryValue::Json { value }) => {
            builder.push_bind(json_string_array(value)?);
        }
        (PgType::IntArray, InventoryValue::Json { value }) => {
            builder.push_bind(json_int_array(value)?);
        }
        (PgType::SmallIntArray, InventoryValue::Json { value }) => {
            let values = json_int_array(value)?
                .into_iter()
                .map(|value| {
                    i16::try_from(value).map_err(|_| {
                        AppError::Validation("array entry exceeds the smallint range".into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            builder.push_bind(values);
        }
        (other, _) => {
            return Err(AppError::Validation(format!(
                "unsupported column type {other:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "local_inventory_target/hardware_tests.rs"]
mod hardware_tests;

#[cfg(test)]
mod captured_hardware_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    #[ignore = "requires owned PostgreSQL fixture with integrated migrations"]
    async fn natural_key_nullable_scopes_and_conflicts()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = std::env::var("PIPING_DATABASE__URL")?;
        let pool = PgPool::connect(&url).await?;
        let mut tx = pool.begin().await?;
        let token = Uuid::now_v7().to_string();
        let make = |entity: InventoryEntity,
                    id: Uuid,
                    scope: Option<(&'static str, Option<Uuid>)>,
                    name: String| {
            let mut columns = vec![PlannedColumn {
                column: "name",
                value: ColumnValue::Scalar(
                    PgType::Text,
                    InventoryValue::Text {
                        value: name.clone(),
                    },
                ),
                updatable: true,
            }];
            if matches!(
                entity,
                InventoryEntity::Platform | InventoryEntity::Tenant | InventoryEntity::Site
            ) {
                columns.push(PlannedColumn {
                    column: "slug",
                    value: ColumnValue::Scalar(PgType::Text, InventoryValue::Text { value: name }),
                    updatable: true,
                });
            }
            if let Some((column, value)) = scope {
                columns.push(PlannedColumn {
                    column,
                    value: ColumnValue::Uuid(value),
                    updatable: true,
                });
            }
            PlannedRow {
                entity,
                source_entity: entity.source_key().into(),
                external_id: id.to_string(),
                local_id: id,
                source_timestamps: SourceTimestamps::default(),
                columns,
                reference_targets: HashMap::new(),
                relations: vec![],
                junctions: vec![],
                selectors: Vec::new(),
            }
        };
        let mut findings = Vec::new();
        let mut complete = true;
        let rows = vec![
            make(
                InventoryEntity::Platform,
                Uuid::now_v7(),
                None,
                token.clone(),
            ),
            make(
                InventoryEntity::Tenant,
                Uuid::now_v7(),
                Some(("group_id", None)),
                token.clone(),
            ),
            make(
                InventoryEntity::Cluster,
                Uuid::now_v7(),
                None,
                token.clone(),
            ),
            make(
                InventoryEntity::VirtualMachine,
                Uuid::now_v7(),
                None,
                token.clone(),
            ),
        ];
        assess_natural_key_conflicts(&mut tx, &rows, &mut findings, &mut complete).await?;
        assert!(complete && findings.is_empty(), "{findings:?}");
        let sites: Vec<_> = (0..1000)
            .map(|n| {
                make(
                    InventoryEntity::Site,
                    Uuid::now_v7(),
                    None,
                    format!("{token}-{n}"),
                )
            })
            .collect();
        assess_natural_key_conflicts(&mut tx, &sites, &mut findings, &mut complete).await?;
        assert!(complete && findings.is_empty(), "{findings:?}");
        let repeated_id = Uuid::now_v7();
        let other_id = Uuid::now_v7();
        let platforms = vec![
            make(InventoryEntity::Platform, repeated_id, None, token.clone()),
            make(InventoryEntity::Platform, repeated_id, None, token.clone()),
            make(InventoryEntity::Platform, other_id, None, token.clone()),
            make(InventoryEntity::Platform, repeated_id, None, token.clone()),
            make(
                InventoryEntity::Platform,
                Uuid::now_v7(),
                Some(("manufacturer_id", Some(Uuid::now_v7()))),
                token.clone(),
            ),
        ];
        assess_natural_key_conflicts(&mut tx, &platforms, &mut findings, &mut complete).await?;
        assert!(complete);
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.code == FindingCode::TargetDefinitionConflict)
                .count(),
            4
        );
        findings.clear();
        let scope = Uuid::now_v7();
        let duplicate = vec![
            make(
                InventoryEntity::VirtualMachine,
                Uuid::now_v7(),
                Some(("cluster_id", Some(scope))),
                token.to_uppercase(),
            ),
            make(
                InventoryEntity::VirtualMachine,
                Uuid::now_v7(),
                Some(("cluster_id", Some(scope))),
                token.clone(),
            ),
            make(
                InventoryEntity::VirtualMachine,
                Uuid::now_v7(),
                Some(("cluster_id", Some(Uuid::now_v7()))),
                token.clone(),
            ),
        ];
        findings.clear();
        assess_natural_key_conflicts(&mut tx, &duplicate, &mut findings, &mut complete).await?;
        assert!(complete);
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.code == FindingCode::TargetDefinitionConflict)
                .count(),
            1
        );
        let missing = make(InventoryEntity::Platform, Uuid::now_v7(), None, token);
        let mut missing = missing;
        missing.columns.retain(|column| column.column != "name");
        findings.clear();
        assess_natural_key_conflicts(&mut tx, &[missing], &mut findings, &mut complete).await?;
        assert!(
            !complete
                && findings
                    .iter()
                    .any(|f| f.code == FindingCode::AnalysisDependencyUnavailable)
        );
        tx.rollback().await?;
        Ok(())
    }

    #[test]
    fn device_index_values_preserve_omitted_fields_and_explicit_nulls() {
        let site = Uuid::from_u128(7);
        let current = json!({"name": "MiXeD", "site_id": site, "tenant_id": null});
        let mut row = PlannedRow {
            entity: InventoryEntity::Device,
            source_entity: "dcim.devices".into(),
            external_id: "one".into(),
            local_id: Uuid::from_u128(1),
            source_timestamps: SourceTimestamps::default(),
            columns: Vec::new(),
            reference_targets: HashMap::new(),
            relations: Vec::new(),
            junctions: Vec::new(),
            selectors: Vec::new(),
        };
        assert_eq!(
            effective_device_index_value(&row, Some(&current), "name", true),
            Some(json!("MiXeD"))
        );
        assert_eq!(
            effective_device_index_value(&row, Some(&current), "site_id", false),
            Some(json!(site))
        );
        assert_eq!(
            effective_device_index_value(&row, None, "tenant_id", true),
            Some(Value::Null)
        );
        assert_eq!(
            effective_device_index_value(&row, None, "site_id", false),
            None
        );
        row.columns.push(PlannedColumn {
            column: "name",
            value: ColumnValue::Scalar(PgType::Text, InventoryValue::Null),
            updatable: true,
        });
        assert_eq!(
            effective_device_index_value(&row, Some(&current), "name", true),
            Some(Value::Null)
        );
        row.columns[0].updatable = false;
        assert_eq!(
            effective_device_index_value(&row, Some(&current), "name", true),
            Some(json!("MiXeD"))
        );
    }

    #[test]
    fn prepared_intent_includes_null_empty_and_relations_without_exposing_values() {
        let entity = InventoryEntity::Site;
        let name = entity.field("name").expect("name field");
        let owner = entity.field("owner").expect("owner field");
        let region = entity.field("region").expect("region field");
        let tags = entity.field("tags").expect("tag field");
        let other = InventoryEntity::Owner
            .field("name")
            .expect("other name field");
        let mut row = PreparedRow {
            entity,
            source_entity: entity.source_key().into(),
            external_id: "1".into(),
            local_id: Uuid::from_u128(1),
            source_timestamps: SourceTimestamps::default(),
            scalars: BTreeMap::from([(name, InventoryValue::Null)]),
            references: vec![(
                owner,
                ReferenceBinding {
                    target: Some(InventoryEntity::Owner),
                    type_token: None,
                    external_id: "2".into(),
                },
            )],
            cleared: vec![region],
            junctions: Vec::new(),
            selectors: Vec::new(),
            custom_fields: Some(serde_json::Map::new()),
            prepared_definition: None,
            tags: Some(Vec::new()),
        };
        for field in [name, owner, region, tags] {
            assert!(row.has_field(field));
        }
        assert!(!row.has_field(other));
        assert!(row.has_custom_fields());
        row.tags = None;
        row.custom_fields = None;
        assert!(!row.has_field(tags));
        assert!(!row.has_custom_fields());

        let junction = InventoryEntity::CustomField
            .field("object_types")
            .expect("junction field");
        row.entity = InventoryEntity::CustomField;
        row.junctions.push((junction, Vec::new()));
        assert!(row.has_field(junction));
        assert!(!row.has_field(name));
    }

    #[test]
    fn equal_uuids_in_distinct_entities_do_not_satisfy_dependencies() {
        let shared = Uuid::from_u128(1);
        let make =
            |entity: InventoryEntity, local_id: Uuid, columns: Vec<PlannedColumn>| PlannedRow {
                entity,
                source_entity: entity.source_key().into(),
                external_id: "synthetic".into(),
                local_id,
                source_timestamps: SourceTimestamps::default(),
                columns,
                reference_targets: HashMap::new(),
                relations: Vec::new(),
                junctions: Vec::new(),
                selectors: Vec::new(),
            };
        let tag = make(InventoryEntity::Tag, shared, Vec::new());
        let group = make(InventoryEntity::OwnerGroup, shared, Vec::new());
        let owner = make(
            InventoryEntity::Owner,
            Uuid::from_u128(2),
            vec![PlannedColumn {
                column: "group_id",
                value: ColumnValue::Uuid(Some(shared)),
                updatable: true,
            }],
        );
        let only_tag = HashSet::from([(InventoryEntity::Tag, shared)]);
        assert!(!owner.reference_is_incoming("group_id", shared, &only_tag));
        let with_group = HashSet::from([
            (InventoryEntity::Tag, shared),
            (InventoryEntity::OwnerGroup, shared),
        ]);
        assert!(owner.reference_is_incoming("group_id", shared, &with_group));
        let mut generic = make(InventoryEntity::IpAddress, Uuid::from_u128(3), Vec::new());
        generic
            .reference_targets
            .insert("assigned_object_id", InventoryEntity::VmInterface);
        assert!(!generic.reference_is_incoming(
            "assigned_object_id",
            shared,
            &HashSet::from([(InventoryEntity::Interface, shared)])
        ));
        assert!(generic.reference_is_incoming(
            "assigned_object_id",
            shared,
            &HashSet::from([(InventoryEntity::VmInterface, shared)])
        ));
        // The group already exists outside this batch. The owner's own UUID is
        // equal to that group's UUID, but this is not a self-dependency.
        let existing_group_reference = make(
            InventoryEntity::Owner,
            shared,
            vec![PlannedColumn {
                column: "group_id",
                value: ColumnValue::Uuid(Some(shared)),
                updatable: true,
            }],
        );
        assert!(hardware_write_order(&[existing_group_reference]).is_ok());
        let rows = vec![tag, owner, group];
        let order: Vec<_> = hardware_write_order(&rows)
            .expect("acyclic graph")
            .iter()
            .map(|row| row.entity)
            .collect();
        assert_eq!(
            order,
            vec![
                InventoryEntity::Tag,
                InventoryEntity::OwnerGroup,
                InventoryEntity::Owner
            ]
        );
    }

    #[test]
    fn transfer_plan_pin_covers_values_and_timestamp_absence() {
        let batch = LocalInventoryBatch::default();
        let mut row = PlannedRow {
            entity: InventoryEntity::OwnerGroup,
            source_entity: "users.owner-groups".into(),
            external_id: "1".into(),
            local_id: Uuid::from_u128(1),
            source_timestamps: SourceTimestamps::default(),
            columns: vec![PlannedColumn {
                column: "name",
                value: ColumnValue::Scalar(
                    PgType::Text,
                    InventoryValue::Text {
                        value: "first".into(),
                    },
                ),
                updatable: true,
            }],
            reference_targets: HashMap::new(),
            relations: Vec::new(),
            junctions: Vec::new(),
            selectors: Vec::new(),
        };
        let initial = resolved_plan_digest(&batch, &[row.clone()]).expect("digest");
        row.source_timestamps.created = Some(None);
        let cleared = resolved_plan_digest(&batch, &[row.clone()]).expect("digest");
        assert_ne!(initial, cleared);
        row.columns[0].value = ColumnValue::Scalar(
            PgType::Text,
            InventoryValue::Text {
                value: "second".into(),
            },
        );
        assert_ne!(
            cleared,
            resolved_plan_digest(&batch, &[row]).expect("digest")
        );
    }

    #[tokio::test]
    #[ignore = "requires owned PostgreSQL fixture with integrated migrations; actual snapshot Device upsert consumer"]
    async fn snapshot_device_insert_does_not_materialize_unbound_template_components()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = std::env::var("PIPING_DATABASE__URL")
            .map_err(|_| "select an owned PostgreSQL fixture through PIPING_DATABASE__URL")?;
        let pool = PgPool::connect(&url).await?;
        crate::storage::database::migrations::run_migrations(&pool).await?;
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *tx)
            .await?;
        let site = Uuid::now_v7();
        let device_type = Uuid::now_v7();
        let device = Uuid::now_v7();
        let name = format!("snapshot-materialization-{device}");
        sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
            .bind(site).bind(&name).execute(&mut *tx).await?;
        let cloned=sqlx::query("INSERT INTO dcim_devicetype SELECT (jsonb_populate_record(NULL::dcim_devicetype,to_jsonb(t)||jsonb_build_object('id',$1::uuid,'model',$2::text,'slug',$2::text,'external_id',NULL))).* FROM dcim_devicetype t ORDER BY t.id LIMIT 1")
            .bind(device_type).bind(&name).execute(&mut *tx).await?;
        if cloned.rows_affected() != 1 {
            return Err("fixture DeviceType missing".into());
        }
        sqlx::query("INSERT INTO dcim_modulebaytemplate (name,label,position,description,device_type_id) VALUES ('slot','','','',$1)").bind(device_type).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO dcim_coolingintaketemplate (name,label,description,device_type_id) VALUES ('in','','',$1)").bind(device_type).execute(&mut *tx).await?;
        let role: Uuid = sqlx::query_scalar("SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1")
            .fetch_one(&mut *tx)
            .await?;
        let mut columns = vec![
            PlannedColumn {
                column: "name",
                value: ColumnValue::Scalar(PgType::Text, InventoryValue::Text { value: name }),
                updatable: true,
            },
            PlannedColumn {
                column: "site_id",
                value: ColumnValue::Uuid(Some(site)),
                updatable: true,
            },
            PlannedColumn {
                column: "device_type_id",
                value: ColumnValue::Uuid(Some(device_type)),
                updatable: true,
            },
            PlannedColumn {
                column: "role_id",
                value: ColumnValue::Uuid(Some(role)),
                updatable: true,
            },
        ];
        for (column, value) in [
            ("serial", "''"),
            ("status", "'active'"),
            ("description", "''"),
            ("comments", "''"),
            ("custom_field_data", "'{}'::jsonb"),
            ("console_port_count", "0"),
            ("console_server_port_count", "0"),
            ("power_port_count", "0"),
            ("power_outlet_count", "0"),
            ("interface_count", "0"),
            ("front_port_count", "0"),
            ("rear_port_count", "0"),
            ("device_bay_count", "0"),
            ("module_bay_count", "0"),
            ("inventory_item_count", "0"),
        ] {
            columns.push(PlannedColumn {
                column,
                value: ColumnValue::Literal(value),
                updatable: false,
            });
        }
        let plan = PlannedRow {
            entity: InventoryEntity::Device,
            source_entity: InventoryEntity::Device.source_key().into(),
            external_id: "123".into(),
            local_id: device,
            source_timestamps: SourceTimestamps::default(),
            columns,
            reference_targets: HashMap::new(),
            relations: vec![],
            junctions: vec![],
            selectors: Vec::new(),
        };
        upsert_row(&mut tx, &plan).await?;
        upsert_row(&mut tx, &plan).await?;
        let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT COUNT(*) FROM dcim_device WHERE id=$1),(SELECT COUNT(*) FROM dcim_modulebay WHERE device_id=$1),(SELECT COUNT(*) FROM dcim_coolingintake WHERE device_id=$1)").bind(device).fetch_one(&mut *tx).await?;
        tx.rollback().await?;
        assert_eq!(
            counts,
            (1, 0, 0),
            "snapshot upsert must not generate component UUIDs outside the explicit imported graph"
        );
        Ok(())
    }

    use crate::contexts::sync::domain::inventory_sync::{SourceEntity, SourceIdentity};

    fn record(entity: &str, external_id: &str, fields: Value) -> SourceRecord {
        let identity = SourceIdentity::new(
            Uuid::now_v7(),
            SourceEntity::new(entity).expect("valid source entity"),
            external_id,
        )
        .expect("valid identity");
        let Value::Object(fields) = fields else {
            panic!("fields must be an object");
        };
        SourceRecord::new(identity, fields)
    }

    fn literal(target: &str, value: Value) -> FieldMapping {
        FieldMapping {
            id: Uuid::now_v7(),
            entity_mapping_id: Uuid::now_v7(),
            source_field: target.to_string(),
            target_field: target.to_string(),
            forward_transform_json: json!({ "op": "literal", "args": [value] }),
            reverse_transform_json: None,
            round_trip_critical: false,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn positioned_template_mapping_preserves_both_position_values() {
        let source = record(
            "dcim.port-template-mappings",
            "1",
            json!({
                "front_port_position": 2, "rear_port_position": 7
            }),
        );
        let mappings = [
            literal("front_port_position", json!(2)),
            literal("rear_port_position", json!(7)),
        ];
        let entity = InventoryEntity::PortTemplateMapping;
        let row = prepare_row(
            entity,
            source.identity.source_provider_id,
            &source,
            &borrow(&mappings),
        )
        .expect("valid positions");
        for (name, expected) in [("front_port_position", 2), ("rear_port_position", 7)] {
            let field = entity.field(name).expect("position field");
            assert_eq!(
                row.scalars.get(&field),
                Some(&InventoryValue::Integer { value: expected })
            );
        }
    }

    #[test]
    fn positioned_template_mapping_requires_both_endpoint_mappings() {
        let provider = Uuid::from_u128(1);
        let entity = InventoryEntity::PortTemplateMapping;
        let config = assessment_config(
            entity,
            provider,
            &["front_port_position", "rear_port_position"],
        );
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![provider_record(
                provider,
                entity,
                "1",
                json!({
                    "front_port_position": 1, "rear_port_position": 1
                }),
            )],
        };
        let result = assess_preparation(&config, &snapshot);
        assert!(result.batch.is_none());
        assert!(
            result
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::FieldValueInvalid
                    && finding.field_path.as_deref() == Some("front_port"))
        );
    }

    #[test]
    fn positioned_mapping_requires_complete_unmodified_orm_values() {
        let provider = Uuid::from_u128(1);
        let entity = InventoryEntity::PortTemplateMapping;
        let fields = [
            "front_port_position",
            "rear_port_position",
            "front_port",
            "rear_port",
            "device_type",
            "module_type",
        ];
        let mut config = assessment_config(entity, provider, &fields);
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![provider_record(
                provider,
                entity,
                "1",
                json!({
                    "front_port_position": 1, "rear_port_position": 2,
                    "front_port": 10, "rear_port": 11, "device_type": 12, "module_type": null
                }),
            )],
        };
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_some(), "{:#?}", assessment.findings);
        for field in fields {
            let mut incomplete = snapshot.clone();
            incomplete.records[0].fields.remove(field);
            let result = assess_preparation(&config, &incomplete);
            assert!(result.batch.is_none(), "missing {field} must block");
            assert!(
                result
                    .findings
                    .iter()
                    .any(|f| f.code == FindingCode::FieldValueInvalid
                        && f.field_path.as_deref() == Some(field))
            );
        }
        let mapping = config
            .field_mappings
            .iter_mut()
            .find(|field| field.target_field == "rear_port_position")
            .expect("position mapping");
        mapping.forward_transform_json = json!({"op":"literal", "args":[3]});
        let transformed = assess_preparation(&config, &snapshot);
        assert!(transformed.batch.is_none());
        assert!(
            transformed
                .findings
                .iter()
                .any(|f| f.code == FindingCode::MappingInvalid
                    && f.field_path.as_deref() == Some("rear_port_position"))
        );
    }

    #[test]
    fn template_reverse_port_relations_are_consumed_by_the_mapping_resource() {
        // `rear_ports` is the canonical embedded relationship the source
        // adapter projects into `dcim.port-template-mappings` records, and
        // `front_ports` is its reverse view. Both are consumed, so a populated
        // value must not become an unsupported-field blocker. The legacy scalar
        // pair has no local target field and still blocks.
        let provider = Uuid::from_u128(1);
        for (entity, fields, field) in [
            (
                InventoryEntity::FrontPortTemplate,
                json!({"name": "port", "rear_ports": [
                    {"position": 1, "rear_port": 2, "rear_port_position": 1}
                ]}),
                "rear_ports",
            ),
            (
                InventoryEntity::RearPortTemplate,
                json!({"name": "port", "front_ports": [
                    {"position": 1, "front_port": 2, "front_port_position": 1}
                ]}),
                "front_ports",
            ),
        ] {
            let config = assessment_config(entity, provider, &["name"]);
            let snapshot = SourceSnapshot {
                coverage: None,
                records: vec![provider_record(provider, entity, "1", fields)],
            };
            let result = assess_preparation(&config, &snapshot);
            assert!(
                !result.findings.iter().any(|finding| {
                    finding.code == FindingCode::FieldUnsupported
                        && finding.field_path.as_deref() == Some(field)
                }),
                "{entity:?}::{field} is consumed by the mapping projection"
            );
        }

        for field in ["rear_port", "rear_port_position"] {
            let config = assessment_config(InventoryEntity::FrontPortTemplate, provider, &["name"]);
            let snapshot = SourceSnapshot {
                coverage: None,
                records: vec![provider_record(
                    provider,
                    InventoryEntity::FrontPortTemplate,
                    "1",
                    json!({"name": "port", "rear_port": 2, "rear_port_position": 1}),
                )],
            };
            let result = assess_preparation(&config, &snapshot);
            assert!(
                result.findings.iter().any(|finding| {
                    finding.code == FindingCode::FieldUnsupported
                        && finding.field_path.as_deref() == Some(field)
                }),
                "legacy scalar {field} must still block"
            );
        }
    }

    #[test]
    fn source_timestamps_preserve_missing_and_explicit_null() {
        let missing = record("dcim.sites", "1", json!({}));
        assert_eq!(
            SourceTimestamps::from_record(&missing).expect("missing observations are valid"),
            SourceTimestamps::default(),
        );
        let cleared = record("dcim.sites", "1", json!({"created": null}));
        assert_eq!(
            SourceTimestamps::from_record(&cleared).expect("explicit unknown is valid"),
            SourceTimestamps {
                created: Some(None),
                updated: None
            },
        );
    }

    #[test]
    fn source_timestamps_normalize_explicit_instants_without_losing_precision() {
        let source = record(
            "dcim.sites",
            "1",
            json!({
                "created": "2020-02-29T01:02:03.123456000000000+03:00",
                "last_updated": "2020-03-01T00:00:00.000000000000Z"
            }),
        );
        let dates = SourceTimestamps::from_record(&source).expect("valid source instants");
        assert_eq!(
            serde_json::to_value(dates.created).expect("date serialization"),
            json!("2020-02-28T22:02:03.123456Z"),
        );
        assert_eq!(
            serde_json::to_value(dates.updated).expect("date serialization"),
            json!("2020-03-01T00:00:00Z"),
        );
    }

    #[test]
    fn source_timestamps_reject_invalid_shapes_and_lossy_instants() {
        for value in [
            json!(false),
            json!(0),
            json!([]),
            json!({}),
            json!(""),
            json!("not-a-date"),
            json!("2020-02-29"),
            json!("2020-02-29T00:00:00"),
            json!("2020-02-29T00:00:00.1234567Z"),
            json!("2020-02-29T00:00:00.1234560001Z"),
            json!("2020-02-29T00:00:00.123456000000001Z"),
            json!("2016-12-31T23:59:60Z"),
        ] {
            let source = record(
                "dcim.sites",
                "1",
                json!({
                    "created": value.clone(), "last_updated": value
                }),
            );
            assert_eq!(
                SourceTimestamps::from_record(&source),
                Err(vec!["created", "last_updated"]),
            );
        }
    }

    #[test]
    fn source_timestamps_reach_prepared_rows_without_local_date_mappings() {
        let source = record(
            "dcim.sites",
            "1",
            json!({
                "name": "site", "slug": "site",
                "created": "2020-02-29T01:02:03.123456+03:00",
                "last_updated": null
            }),
        );
        let config = assessment_config(
            InventoryEntity::Site,
            source.identity.source_provider_id,
            &["name", "slug"],
        );
        let assessment = assess_preparation(
            &config,
            &SourceSnapshot {
                coverage: None,
                records: vec![source],
            },
        );
        assert!(
            assessment.findings.is_empty(),
            "valid dates must not block the row"
        );
        let batch = assessment.batch.expect("prepared batch");
        let dates = batch.rows[0].source_timestamps;
        assert_eq!(
            serde_json::to_value(dates.created).expect("date serialization"),
            json!("2020-02-28T22:02:03.123456Z"),
        );
        assert_eq!(dates.updated, Some(None));
    }

    #[test]
    fn source_timestamps_report_each_invalid_field_before_any_write() {
        let source = record(
            "dcim.sites",
            "1",
            json!({
                "name": "site", "slug": "site", "created": "invalid", "last_updated": false
            }),
        );
        let config = assessment_config(
            InventoryEntity::Site,
            source.identity.source_provider_id,
            &["name", "slug"],
        );
        let assessment = assess_preparation(
            &config,
            &SourceSnapshot {
                coverage: None,
                records: vec![source],
            },
        );
        assert!(assessment.batch.is_none());
        assert!(assessment.analysis_complete);
        let fields: HashSet<_> = assessment
            .findings
            .iter()
            .filter(|finding| finding.code == FindingCode::FieldValueInvalid)
            .filter_map(|finding| finding.field_path.as_deref())
            .collect();
        assert_eq!(fields, HashSet::from(["created", "last_updated"]));
    }

    pub(super) fn identity(target: &str, source: &str) -> FieldMapping {
        FieldMapping {
            id: Uuid::now_v7(),
            entity_mapping_id: Uuid::now_v7(),
            source_field: source.to_string(),
            target_field: target.to_string(),
            forward_transform_json: json!({ "op": "identity", "args": [{ "field": source }] }),
            reverse_transform_json: None,
            round_trip_critical: false,
            created_at: chrono::Utc::now(),
        }
    }

    fn borrow(mappings: &[FieldMapping]) -> Vec<&FieldMapping> {
        mappings.iter().collect()
    }

    pub(super) fn assessment_config(
        entity: InventoryEntity,
        provider: Uuid,
        fields: &[&str],
    ) -> MappingConfigDocument {
        use crate::contexts::sync::domain::{
            ConflictMode, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
        };
        let logical = Uuid::from_u128(22);
        let mapping_id = Uuid::new_v5(&logical, entity.source_key().as_bytes());
        let mut field_mappings: Vec<_> = fields
            .iter()
            .map(|name| {
                let mut field = identity(name, name);
                field.entity_mapping_id = mapping_id;
                field
            })
            .collect();
        for field in entity
            .field_iter()
            .filter(|field| field.absent() == Absent::Reject && !fields.contains(&field.name()))
        {
            let mut mapping = literal(
                field.name(),
                json!(if field.name() == "status" {
                    "active"
                } else {
                    ""
                }),
            );
            mapping.entity_mapping_id = mapping_id;
            field_mappings.push(mapping);
        }
        MappingConfigDocument {
            absence_policy: Default::default(),
            reference: MappingRef::new(logical, 1),
            name: "synthetic".into(),
            description: None,
            source_provider_id: provider,
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: chrono::Utc::now(),
            retired_at: None,
            entity_mappings: vec![EntityMapping {
                id: mapping_id,
                mapping_config_logical_id: logical,
                mapping_config_version_id: 1,
                source_entity: entity.source_key().into(),
                target_entity: entity.target_key(),
                enabled: true,
                created_at: chrono::Utc::now(),
            }],
            field_mappings,
        }
    }

    pub(super) fn provider_record(
        provider: Uuid,
        entity: InventoryEntity,
        id: &str,
        fields: Value,
    ) -> SourceRecord {
        let mut record = record(entity.source_key(), id, fields);
        record.identity.source_provider_id = provider;
        record
    }

    fn business_owner_fixture(provider: Uuid) -> (MappingConfigDocument, SourceSnapshot) {
        let mut config =
            assessment_config(InventoryEntity::Site, provider, &["name", "slug", "owner"]);
        for (entity, fields) in [
            (InventoryEntity::OwnerGroup, &["name", "description"][..]),
            (
                InventoryEntity::Owner,
                &["name", "description", "group"][..],
            ),
        ] {
            let mapping = assessment_config(entity, provider, fields);
            config.entity_mappings.extend(mapping.entity_mappings);
            config.field_mappings.extend(mapping.field_mappings);
        }
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![
                provider_record(
                    provider,
                    InventoryEntity::OwnerGroup,
                    "g1",
                    json!({"name": format!("owners-group-{provider}"), "description": "group"}),
                ),
                provider_record(
                    provider,
                    InventoryEntity::Owner,
                    "o1",
                    json!({"name": format!("owners-person-{provider}"), "description": "owner", "group": {"id": "g1"}}),
                ),
                provider_record(
                    provider,
                    InventoryEntity::Site,
                    "s1",
                    json!({"name": format!("owners-site-{provider}"), "slug": format!("owners-site-{provider}"), "owner": {"id": "o1"}}),
                ),
            ],
        };
        (config, snapshot)
    }

    #[test]
    fn business_owner_preparation_is_qualified_and_rejects_memberships() {
        let provider = Uuid::from_u128(51);
        let (config, snapshot) = business_owner_fixture(provider);
        let batch = assess_preparation(&config, &snapshot)
            .batch
            .expect("business owner graph prepares");
        assert_eq!(batch.rows.len(), 3);
        let owner = batch
            .rows
            .iter()
            .find(|row| row.entity == InventoryEntity::Owner)
            .expect("owner");
        let site = batch
            .rows
            .iter()
            .find(|row| row.entity == InventoryEntity::Site)
            .expect("site");
        assert!(
            owner
                .references
                .iter()
                .any(|(field, binding)| field.name() == "group"
                    && binding.target == Some(InventoryEntity::OwnerGroup)
                    && binding.external_id == "g1")
        );
        assert!(
            site.references
                .iter()
                .any(|(field, binding)| field.name() == "owner"
                    && binding.target == Some(InventoryEntity::Owner)
                    && binding.external_id == "o1")
        );
        let other = derive_local_id(
            Uuid::from_u128(52),
            "users.owners",
            "o1",
            &InventoryEntity::Owner.target_key(),
        );
        assert_ne!(owner.local_id, other);
        for field in ["users", "user_groups"] {
            let mut invalid = snapshot.clone();
            invalid.records[1]
                .fields
                .insert(field.into(), json!([{"id": 7}]));
            let result = assess_preparation(&config, &invalid);
            assert!(result.batch.is_none());
            assert!(
                result
                    .findings
                    .iter()
                    .any(|finding| finding.code == FindingCode::FieldUnsupported)
            );
        }
    }

    #[test]
    fn transformed_custom_field_definition_cannot_diverge_from_validation() {
        let source = record(
            "extras.custom-fields",
            "1",
            json!({"name": "source_name", "type": "text"}),
        );
        let mappings = [
            literal("name", json!("different_name")),
            literal("type", json!("text")),
        ];
        let result = prepare_row(
            InventoryEntity::CustomField,
            source.identity.source_provider_id,
            &source,
            &borrow(&mappings),
        );
        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    fn owner_tag_fixture() -> (MappingConfigDocument, SourceSnapshot) {
        let provider = Uuid::from_u128(1);
        let mut config =
            assessment_config(InventoryEntity::Site, provider, &["name", "slug", "tags"]);
        let tags = assessment_config(InventoryEntity::Tag, provider, &["name", "slug"]);
        config.entity_mappings.extend(tags.entity_mappings);
        config.field_mappings.extend(tags.field_mappings);
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![
                provider_record(
                    provider,
                    InventoryEntity::Tag,
                    "100",
                    json!({"name": "one", "slug": "tag-one"}),
                ),
                provider_record(
                    provider,
                    InventoryEntity::Site,
                    "1",
                    json!({"name": "A", "slug": "a", "tags": [{"id": 100, "slug": "tag-one"}]}),
                ),
                provider_record(
                    provider,
                    InventoryEntity::Site,
                    "2",
                    json!({"name": "B", "slug": "b", "tags": []}),
                ),
            ],
        };
        (config, snapshot)
    }

    #[test]
    fn tag_memberships_use_provider_ids_not_mutable_labels() {
        let (config, mut snapshot) = owner_tag_fixture();
        snapshot.records[1].fields.insert(
            "tags".into(),
            json!([{"id": 100, "slug": "stale", "name": "stale"}]),
        );
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "1",
            json!({"object_type": "dcim.site", "object_id": 1, "tag": {"id": 100}}),
        ));
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_some(), "{:?}", assessment.findings);
        snapshot.records[0]
            .fields
            .insert("slug".into(), json!("renamed"));
        snapshot.records[0]
            .fields
            .insert("name".into(), json!("Renamed"));
        assert!(assess_preparation(&config, &snapshot).batch.is_some());
        snapshot.records[1].fields.remove("tags");
        snapshot.records.retain(|record| {
            record.identity.entity.as_str() != InventoryEntity::TaggedObject.source_key()
        });
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_some(), "{:?}", assessment.findings);
        let batch = assessment.batch.unwrap();
        assert!(
            batch
                .rows
                .iter()
                .find(|row| row.entity == InventoryEntity::Site && row.external_id == "1")
                .unwrap()
                .tags
                .is_none()
        );
    }

    #[test]
    fn verified_membership_counts_do_not_fabricate_source_rows() {
        use crate::contexts::sync::infrastructure::providers::netbox_snapshot_capture::{
            capture_snapshot, tests::transport,
        };
        let provider = Uuid::now_v7();
        let mut value = transport(provider);
        for resource in value["resources"].as_array_mut().unwrap() {
            match resource["resource_key"].as_str().unwrap() {
                "dcim.sites" => resource["rows"] = json!([{"id":1,"tags":[7]}]),
                "extras.tags" => resource["rows"] = json!([{"id":7}]),
                _ => {}
            }
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("memberships");
        let anchor = capture.seal(&bundle).unwrap();
        let source = ImmutableInventorySource::open(&bundle, &anchor, provider, 100).unwrap();
        let snapshot = source.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(
            !snapshot
                .records
                .iter()
                .any(|record| record.identity.entity.as_str()
                    == InventoryEntity::TaggedObject.source_key())
        );
        assert_eq!(
            count_tag_memberships(&snapshot, source.verified_tag_relationships()),
            1
        );
        assert_eq!(count_tag_memberships(&snapshot, None), 0);
    }

    #[test]
    fn omitted_embedded_tags_block_positive_edges_instead_of_claiming_preserved_arrays_match() {
        let (config, mut snapshot) = owner_tag_fixture();
        snapshot.records[1].fields.remove("tags");
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::Tag,
            "101",
            json!({"name": "two", "slug": "tag-two"}),
        ));
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "201",
            json!({"object_type": "dcim.site", "object_id": 1, "tag": {"id": 101}}),
        ));
        // Preparation cannot know whether the stored array is empty, contains
        // Tag100, or already matches Tag101. None permits a commit claim here.
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        assert!(!assessment.analysis_complete);
        assert!(assessment.findings.iter().any(|finding| {
            finding.code == FindingCode::AnalysisDependencyUnavailable
                && finding.record_identity_sha256.as_deref() == Some(digest(b"1").as_str())
                && finding.field_path.as_deref() == Some("tags.graph.actual")
        }));
    }

    #[test]
    fn tag_input_rejects_null_labels_bad_ids_and_duplicates_but_accepts_empty() {
        let provider = Uuid::from_u128(1);
        for value in [
            json!(null),
            json!(["slug"]),
            json!([{"slug": "slug", "name": "name"}]),
            json!([{"id": null}]),
            json!([{"id": true}]),
            json!([{"id": ""}]),
            json!([{"id": 100}, {"id": "100"}]),
        ] {
            assert!(tag_references(&value, provider).is_err(), "{value}");
            let rec = record("dcim.sites", "1", json!({"tags": value}));
            assert!(
                prepare_row(
                    InventoryEntity::Site,
                    provider,
                    &rec,
                    &borrow(&[identity("tags", "tags")])
                )
                .is_err()
            );
        }
        assert!(tag_references(&json!([]), provider).unwrap().is_empty());
    }

    #[test]
    fn independent_tag_findings_survive_bad_entries_and_unrelated_fields() {
        let (config, mut snapshot) = owner_tag_fixture();
        snapshot.records[1]
            .fields
            .insert("name".into(), json!(true));
        snapshot.records[1].fields.insert(
            "tags".into(),
            json!([{"id": 999}, {"id": 999}, {"slug": "tag-one"}]),
        );
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        for code in [
            FindingCode::TargetReferenceMissing,
            FindingCode::TagMembershipDuplicate,
            FindingCode::TagsInvalid,
        ] {
            assert!(
                assessment
                    .findings
                    .iter()
                    .any(|finding| finding.code == code),
                "{code:?}: {:?}",
                assessment.findings
            );
        }
    }

    #[test]
    fn duplicate_tag_edges_remain_visible_when_edge_identity_is_invalid() {
        let (config, mut snapshot) = owner_tag_fixture();
        let edge = provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "201",
            json!({"object_type": "dcim.site", "object_id": 1, "tag": {"id": 100}}),
        );
        snapshot.records.extend([edge.clone(), edge]);
        snapshot.records[2]
            .fields
            .insert("name".into(), json!(false));
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::RecordIdentityInvalid)
        );
        assert!(assessment.findings.iter().any(|finding| finding.code
            == FindingCode::TagMembershipDuplicate
            && finding.affected_count == 2));
    }

    #[test]
    fn tag_graph_distinguishes_external_ids_even_when_embedded_labels_match() {
        let (config, mut snapshot) = owner_tag_fixture();
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::Tag,
            "101",
            json!({"name": "two", "slug": "tag-two"}),
        ));
        snapshot.records[1]
            .fields
            .insert("tags".into(), json!([{"id": 101, "slug": "tag-one"}]));
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "1",
            json!({"object_type": "dcim.site", "object_id": 1, "tag": {"id": 100}}),
        ));
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        for id in ["100", "101"] {
            assert!(assessment.findings.iter().any(|finding| {
                finding.code == FindingCode::TagGraphMismatch
                    && finding
                        .edge
                        .as_ref()
                        .and_then(|edge| edge.target_identity_sha256.as_deref())
                        == Some(digest(id.as_bytes()).as_str())
            }));
        }
        snapshot.records[3].identity.source_provider_id = Uuid::from_u128(2);
        snapshot.records[1]
            .fields
            .insert("name".into(), json!(true));
        let assessment = assess_preparation(&config, &snapshot);
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::RecordIdentityInvalid)
        );
        assert!(assessment.findings.iter().any(|finding| finding.code
            == FindingCode::TargetReferenceMissing
            && finding.field_path.as_deref() == Some("tags[0]")));
    }

    #[test]
    fn ordinary_pull_validation_error_retains_typed_context_without_private_values() {
        let provider = Uuid::from_u128(1);
        let config = assessment_config(InventoryEntity::Site, provider, &["name", "slug"]);
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![provider_record(
                provider,
                InventoryEntity::Site,
                "1",
                serde_json::json!({"name": "site", "slug": "site", "CANARY-private-key": "CANARY-private-value"}),
            )],
        };
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::FieldUnsupported)
        );
        for code in [
            "inventory_preflight:preparation_blocked",
            "inventory_preflight:target_graph_blocked",
        ] {
            let error =
                blocked_assessment_error(code, &assessment.findings, assessment.analysis_complete);
            let AppError::Validation(message) = error else {
                panic!("expected validation error");
            };
            assert!(!message.contains("CANARY"));
            let report: Value = serde_json::from_str(&message).expect("safe structured error");
            assert_eq!(report["code"].as_str(), Some(code));
            assert_eq!(
                report["analysis_complete"].as_bool(),
                Some(assessment.analysis_complete)
            );
            let findings: Vec<Finding> =
                serde_json::from_value(report["findings"].clone()).expect("typed findings");
            assert_eq!(findings, assessment.findings);
        }
    }

    #[test]
    fn raw_custom_fields_without_mapping_rejects_present_malformed_values() {
        let provider = Uuid::from_u128(1);
        for mapped in [false, true] {
            let fields = if mapped {
                vec!["name", "slug", "custom_fields"]
            } else {
                vec!["name", "slug"]
            };
            let config = assessment_config(InventoryEntity::Site, provider, &fields);
            for value in [
                json!("CANARY-private-value"),
                json!(""),
                json!([]),
                json!([{"private": "CANARY-private-value"}]),
                json!(false),
                json!(true),
                json!(42),
            ] {
                let mut record = provider_record(
                    provider,
                    InventoryEntity::Site,
                    "1",
                    json!({"name": "site", "slug": "site"}),
                );
                record.fields.insert("custom_fields".into(), value);
                let assessment = assess_preparation(
                    &config,
                    &SourceSnapshot {
                        coverage: None,
                        records: vec![record],
                    },
                );
                assert!(assessment.batch.is_none());
                assert!(
                    assessment
                        .findings
                        .iter()
                        .any(|finding| finding.code == FindingCode::FieldValueInvalid
                            && finding.field_path.as_deref() == Some("custom_fields"))
                );
                assert!(
                    !serde_json::to_string(&assessment.findings)
                        .expect("safe shape findings")
                        .contains("CANARY")
                );
            }
        }
    }

    #[test]
    fn raw_and_mapped_custom_fields_preserve_absent_null_and_object_semantics() {
        let provider = Uuid::from_u128(1);
        for mapped in [false, true] {
            let fields = if mapped {
                vec!["name", "slug", "custom_fields"]
            } else {
                vec!["name", "slug"]
            };
            let config = assessment_config(InventoryEntity::Site, provider, &fields);
            for value in [
                None,
                Some(Value::Null),
                Some(json!({"private": "CANARY-private-value"})),
            ] {
                let expected = value.as_ref().map(|value| {
                    if value.is_null() {
                        serde_json::Map::new()
                    } else {
                        value.as_object().expect("test object").clone()
                    }
                });
                let mut record = provider_record(
                    provider,
                    InventoryEntity::Site,
                    "1",
                    json!({"name": "site", "slug": "site"}),
                );
                if let Some(value) = value {
                    record.fields.insert("custom_fields".into(), value);
                }
                let batch = assess_preparation(
                    &config,
                    &SourceSnapshot {
                        coverage: None,
                        records: vec![record],
                    },
                )
                .batch
                .expect("valid custom-field shape");
                assert!(
                    batch.rows[0].custom_fields == expected,
                    "absent/null/object semantics differ"
                );
            }
        }
    }

    #[test]
    fn duplicate_mapped_tag_slugs_survive_an_unrelated_invalid_color() {
        let provider = Uuid::from_u128(1);
        let config = assessment_config(InventoryEntity::Tag, provider, &["name", "slug", "color"]);
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![
                provider_record(
                    provider,
                    InventoryEntity::Tag,
                    "1",
                    json!({"name": "one", "slug": "duplicate", "color": true}),
                ),
                provider_record(
                    provider,
                    InventoryEntity::Tag,
                    "2",
                    json!({"name": "two", "slug": "duplicate", "color": "112233"}),
                ),
            ],
        };
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::FieldValueInvalid
                    && finding.field_path.as_deref() == Some("color"))
        );
        let duplicates: Vec<_> = assessment
            .findings
            .iter()
            .filter(|finding| {
                finding.code == FindingCode::DefinitionInvalid
                    && finding.field_path.as_deref() == Some("slug")
            })
            .collect();
        assert_eq!(duplicates.len(), 2);
        assert!(duplicates.iter().all(|finding| finding.affected_count == 2));
        for id in ["1", "2"] {
            assert!(
                duplicates
                    .iter()
                    .any(|finding| finding.record_identity_sha256.as_deref()
                        == Some(digest(id.as_bytes()).as_str()))
            );
        }
        let mut reordered = snapshot.clone();
        reordered.records.reverse();
        assert_eq!(
            assessment.findings,
            assess_preparation(&config, &reordered).findings
        );
    }

    #[test]
    fn malformed_membership_for_one_owner_does_not_hide_another_owner_mismatch() {
        let (config, mut snapshot) = owner_tag_fixture();
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "201",
            json!({"object_type": "dcim.site", "object_id": 2, "tag": null}),
        ));
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none() && !assessment.analysis_complete);
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::TagMembershipInvalid
                    && finding.field_path.as_deref() == Some("tag"))
        );
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::TagGraphMismatch
                    && finding.record_identity_sha256.as_deref() == Some(digest(b"1").as_str()))
        );
        assert!(
            !assessment
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::TagGraphMismatch
                    && finding.record_identity_sha256.as_deref() == Some(digest(b"2").as_str()))
        );
        assert!(assessment.findings.iter().any(|finding| finding.code
            == FindingCode::AnalysisDependencyUnavailable
            && finding.record_identity_sha256.as_deref() == Some(digest(b"2").as_str())));
    }

    #[test]
    fn unknown_membership_owner_keeps_graph_incomplete_without_inventing_mismatch() {
        let (config, mut snapshot) = owner_tag_fixture();
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "201",
            json!({"object_type": null, "object_id": 1, "tag": {"id": 100}}),
        ));
        let unknown = assess_preparation(&config, &snapshot);
        assert!(unknown.batch.is_none() && !unknown.analysis_complete);
        assert!(
            !unknown
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::TagGraphMismatch)
        );
        assert!(unknown.findings.iter().any(|finding| finding.code
            == FindingCode::AnalysisDependencyUnavailable
            && finding.field_path.as_deref() == Some("tags.graph.owners")));
        snapshot.records[1].fields.insert("tags".into(), json!([]));
        snapshot.records.push(provider_record(
            config.source_provider_id,
            InventoryEntity::TaggedObject,
            "202",
            json!({"object_type": "dcim.site", "object_id": 1, "tag": {"id": 100}}),
        ));
        let positive_edge = assess_preparation(&config, &snapshot);
        assert!(positive_edge.batch.is_none() && !positive_edge.analysis_complete);
        assert!(
            positive_edge
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::TagGraphMismatch
                    && finding.record_identity_sha256.as_deref() == Some(digest(b"1").as_str()))
        );
    }

    #[test]
    fn independent_mapping_definition_expression_and_membership_defects_are_all_retained() {
        use crate::contexts::sync::domain::{
            ConflictMode, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
        };
        let provider = Uuid::from_u128(1);
        let logical = Uuid::from_u128(2);
        let mapping_id = Uuid::from_u128(3);
        let mut fields: Vec<_> = ["name", "type", "validation_regex"]
            .into_iter()
            .map(|name| {
                let mut field = identity(name, name);
                field.entity_mapping_id = mapping_id;
                field
            })
            .collect();
        let mut expression = literal("label", json!("CANARY-private"));
        expression.entity_mapping_id = mapping_id;
        expression.forward_transform_json =
            json!({"op": "concat", "args": [{"op": "CANARY-private", "args": []}, {"sep": ""}]});
        fields.push(expression);
        fields.push(literal("CANARY-private-field", json!("CANARY-private")));
        let config = MappingConfigDocument {
            absence_policy: Default::default(),
            reference: MappingRef::new(logical, 1),
            name: "synthetic".into(),
            description: None,
            source_provider_id: provider,
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: chrono::Utc::now(),
            retired_at: None,
            entity_mappings: vec![
                EntityMapping {
                    id: mapping_id,
                    mapping_config_logical_id: logical,
                    mapping_config_version_id: 1,
                    source_entity: "extras.custom-fields".into(),
                    target_entity: InventoryEntity::CustomField.target_key(),
                    enabled: true,
                    created_at: chrono::Utc::now(),
                },
                EntityMapping {
                    id: Uuid::from_u128(9),
                    mapping_config_logical_id: logical,
                    mapping_config_version_id: 2,
                    source_entity: "CANARY-private-model".into(),
                    target_entity: "CANARY-private-target".into(),
                    enabled: true,
                    created_at: chrono::Utc::now(),
                },
            ],
            field_mappings: fields,
        };
        let empty = assess_preparation(&config, &SourceSnapshot::default());
        assert!(empty.batch.is_none());
        assert!(
            empty
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::ExpressionInvalid)
        );
        assert!(
            empty
                .findings
                .iter()
                .filter(|finding| finding.code == FindingCode::MappingInvalid)
                .count()
                >= 3
        );
        let mut records = vec![
            record(
                "extras.custom-fields",
                "1",
                json!({"name": "alpha", "type": "CANARY-private"}),
            ),
            record(
                "extras.custom-fields",
                "2",
                json!({"name": "beta", "type": "text", "validation_regex": "["}),
            ),
            record(
                "extras.custom-fields",
                "3",
                json!({"name": "gamma", "type": "text"}),
            ),
            record(
                "extras.tagged-objects",
                "101",
                json!({"object_type": null, "object_id": {"private": "CANARY-private"}, "tag": true}),
            ),
            record(
                "extras.tagged-objects",
                "102",
                json!({"object_type": "dcim.device", "object_id": 1, "tag": {"id": 999}}),
            ),
            record(
                "extras.tagged-objects",
                "103",
                json!({"object_type": "dcim.device", "object_id": 1, "tag": {"id": 999}}),
            ),
        ];
        for record in &mut records {
            record.identity.source_provider_id = provider;
        }
        let mut snapshot = SourceSnapshot {
            coverage: None,
            records,
        };
        let assessment = assess_preparation(&config, &snapshot);
        assert!(assessment.batch.is_none());
        assert!(!assessment.analysis_complete);
        for (code, minimum) in [
            (FindingCode::DefinitionInvalid, 2),
            (FindingCode::ExpressionInvalid, 3),
            (FindingCode::MappingInvalid, 3),
            (FindingCode::TagMembershipInvalid, 3),
            (FindingCode::TagMembershipDuplicate, 2),
        ] {
            assert!(
                assessment
                    .findings
                    .iter()
                    .filter(|finding| finding.code == code)
                    .count()
                    >= minimum
            );
        }
        assert!(
            assessment
                .findings
                .iter()
                .filter(|finding| matches!(
                    finding.code,
                    FindingCode::DefinitionInvalid
                        | FindingCode::ExpressionInvalid
                        | FindingCode::TagMembershipInvalid
                        | FindingCode::TagMembershipDuplicate
                ))
                .all(|finding| finding.record_identity_sha256.is_some()
                    && finding.field_path.is_some())
        );
        assert!(
            !serde_json::to_string(&assessment.findings)
                .expect("safe findings")
                .contains("CANARY")
        );
        snapshot.records.reverse();
        assert_eq!(
            assessment.findings,
            assess_preparation(&config, &snapshot).findings
        );
    }

    #[test]
    fn independent_custom_values_reuse_canonical_validation_with_safe_definition_edges() {
        let source = record("dcim.sites", "1", json!({}));
        let row = prepare_row(
            InventoryEntity::Site,
            source.identity.source_provider_id,
            &source,
            &borrow(&[
                literal("name", json!("synthetic")),
                literal("slug", json!("synthetic")),
            ]),
        )
        .expect("prepared row");
        let definition_source = record(
            "extras.custom-fields",
            "1",
            json!({"name": "number", "type": "integer"}),
        );
        let mut integer =
            validate_source_custom_field(&definition_source).expect("canonical definition");
        integer.id = Uuid::from_u128(1);
        integer.is_secret = false;
        let mut text = integer.clone();
        text.id = Uuid::from_u128(2);
        text.is_secret = true;
        text.field_type = CustomFieldType::Text;
        assert!(validate_definition(&integer).is_ok() && validate_definition(&text).is_ok());
        let first = format!("custom_fields.sha256:{}", digest(b"CANARY-private-key"));
        let second = format!("custom_fields.sha256:{}", digest(b"other-key"));
        let mut findings =
            assess_custom_value(&row, &first, &integer, &json!("CANARY-private-value"));
        findings.extend(assess_custom_value(
            &row,
            &second,
            &text,
            &json!({"private": "CANARY-private-value"}),
        ));
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.code == FindingCode::CustomFieldValueInvalid)
                .count(),
            2
        );
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.code == FindingCode::CustomFieldSecret)
                .count(),
            1
        );
        assert!(findings.iter().all(|finding| {
            finding.record_identity_sha256.is_some()
                && finding
                    .edge
                    .as_ref()
                    .is_some_and(|edge| edge.target_local_id.is_some())
        }));
        assert!(
            !serde_json::to_string(&findings)
                .expect("safe custom-field findings")
                .contains("CANARY")
        );
    }

    #[test]
    fn pure_assessment_aggregates_owner_enum_expression_and_model_blockers() {
        use crate::contexts::sync::domain::{
            ConflictMode, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
        };
        let provider = Uuid::from_u128(1);
        let logical = Uuid::from_u128(2);
        let mapping_id = Uuid::from_u128(3);
        let mut expression = literal("name", json!("CANARY-private-expression"));
        expression.entity_mapping_id = mapping_id;
        expression.forward_transform_json = json!({"op": "CANARY-private-expression", "args": []});
        let config = MappingConfigDocument {
            absence_policy: Default::default(),
            reference: MappingRef::new(logical, 1),
            name: "synthetic".into(),
            description: None,
            source_provider_id: provider,
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: chrono::Utc::now(),
            retired_at: None,
            entity_mappings: vec![EntityMapping {
                id: mapping_id,
                mapping_config_logical_id: logical,
                mapping_config_version_id: 1,
                source_entity: "dcim.devices".into(),
                target_entity: InventoryEntity::Device.target_key(),
                enabled: true,
                created_at: chrono::Utc::now(),
            }],
            field_mappings: vec![expression],
        };
        let mut device = record(
            "dcim.devices",
            "1",
            json!({"name": "CANARY-private-value", "owner": {"id": 1}, "cooling_method": "liquid"}),
        );
        device.identity.source_provider_id = provider;
        let mut unknown = record(
            "dcim.unknown-things",
            "2",
            json!({"name": "CANARY-private-value"}),
        );
        unknown.identity.source_provider_id = provider;
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![device, unknown],
        };
        let first = assess_preparation(&config, &snapshot);
        assert!(first.batch.is_none());
        assert!(
            first
                .findings
                .iter()
                .any(|f| f.code == FindingCode::ModelUnsupported)
        );
        assert_eq!(
            first
                .findings
                .iter()
                .filter(|f| f.code == FindingCode::FieldUnsupported)
                .count(),
            2
        );
        assert!(
            first
                .findings
                .iter()
                .any(|f| f.code == FindingCode::ExpressionInvalid)
        );
        let bytes = serde_json::to_vec(&first.findings).expect("findings");
        assert!(!String::from_utf8_lossy(&bytes).contains("CANARY"));
        assert_eq!(
            first.findings,
            assess_preparation(&config, &snapshot).findings
        );
    }

    #[test]
    fn local_id_is_deterministic_and_tuple_sensitive() {
        let provider = Uuid::now_v7();
        let a = derive_local_id(provider, "dcim.devices", "5", "piping.inventory.device");
        assert_eq!(
            a,
            derive_local_id(provider, "dcim.devices", "5", "piping.inventory.device")
        );
        assert_ne!(
            a,
            derive_local_id(provider, "dcim.devices", "6", "piping.inventory.device")
        );
        assert_ne!(
            a,
            derive_local_id(provider, "dcim.sites", "5", "piping.inventory.site")
        );
        assert_ne!(
            a,
            derive_local_id(
                Uuid::now_v7(),
                "dcim.devices",
                "5",
                "piping.inventory.device"
            )
        );
    }

    #[test]
    fn nullable_definition_metadata_is_absent_in_the_domain() {
        let source = record(
            "extras.custom-fields",
            "1",
            json!({
                "name": "owner_team", "type": "text", "default": null, "validation_schema": null,
            }),
        );
        let definition =
            validate_source_custom_field(&source).expect("null optional metadata is valid");
        assert!(definition.default_value.is_none());
        assert!(definition.validation_schema.is_none());
    }

    #[test]
    fn device_row_converts_scalars_choices_and_references() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({ "name": "r1" }));
        let mappings = vec![
            identity("name", "name"),
            literal("status", json!({ "value": "active", "label": "Active" })),
            literal("role", json!({ "id": 7 })),
            literal("site", json!({ "id": 3 })),
            literal("tags", json!([{ "id": 17, "slug": "core" }])),
        ];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        assert_eq!(row.entity, InventoryEntity::Device);
        assert_eq!(row.external_id, "5");
        let tag = &row.tags.as_ref().expect("tags")[0];
        assert_eq!(tag.external_id, "17");
        assert_eq!(tag.entity.as_str(), InventoryEntity::Tag.source_key());
        assert_eq!(tag.source_provider_id, rec.identity.source_provider_id);
        let name = InventoryEntity::Device.field("name").expect("name");
        assert_eq!(
            row.scalars.get(&name),
            Some(&InventoryValue::Text {
                value: "r1".to_string()
            })
        );
        let status = InventoryEntity::Device.field("status").expect("status");
        assert_eq!(
            row.scalars.get(&status),
            Some(&InventoryValue::Text {
                value: "active".to_string()
            })
        );
        let role = row
            .references
            .iter()
            .find(|(field, _)| field.name() == "role")
            .expect("role reference");
        assert_eq!(role.1.target, Some(InventoryEntity::DeviceRole));
        assert_eq!(role.1.external_id, "7");
    }

    #[test]
    fn generic_reference_captures_the_type_token() {
        let provider = Uuid::now_v7();
        let rec = record(
            "dcim.mac-addresses",
            "9",
            json!({ "assigned_object_type": "dcim.interface" }),
        );
        let mappings = vec![
            literal("mac_address", json!("aa:bb:cc:dd:ee:ff")),
            literal("assigned_object", json!({ "id": 42 })),
        ];
        let row = prepare_row(
            InventoryEntity::MacAddress,
            provider,
            &rec,
            &borrow(&mappings),
        )
        .expect("prepared");
        let (field, binding) = row
            .references
            .iter()
            .find(|(field, _)| field.name() == "assigned_object")
            .expect("gfk reference");
        assert_eq!(binding.target, Some(InventoryEntity::Interface));
        assert_eq!(binding.type_token.as_deref(), Some("dcim.interface"));
        assert_eq!(binding.external_id, "42");
        assert!(matches!(field.kind(), FieldKind::GenericReference { .. }));
    }

    #[test]
    fn generic_reference_rejects_unknown_or_disallowed_type() {
        let provider = Uuid::now_v7();
        for token in ["dcim.cable", "dcim.device"] {
            let rec = record(
                "dcim.mac-addresses",
                "9",
                json!({ "assigned_object_type": token }),
            );
            let mappings = vec![literal("assigned_object", json!({ "id": 42 }))];
            assert!(
                prepare_row(
                    InventoryEntity::MacAddress,
                    provider,
                    &rec,
                    &borrow(&mappings)
                )
                .is_err(),
                "token {token} must be rejected"
            );
        }
    }

    #[test]
    fn integer_overflow_is_blocked() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.interfaces", "1", json!({}));
        let mappings = vec![literal("mtu", json!(5_000_000_000_i64))];
        assert!(
            prepare_row(
                InventoryEntity::Interface,
                provider,
                &rec,
                &borrow(&mappings)
            )
            .is_err()
        );
    }

    #[test]
    fn orm_decimal_strings_and_json_numbers_preserve_exact_numeric_values() {
        for (entity, name, raw) in [
            (InventoryEntity::Cable, "length", "12.50"),
            (InventoryEntity::DeviceType, "u_height", "1.5"),
            (InventoryEntity::DeviceType, "weight", "12.34"),
            (InventoryEntity::Device, "position", "2.5"),
            (InventoryEntity::VirtualMachine, "vcpus", "0.25"),
            (InventoryEntity::Site, "latitude", "0.100000000000000001"),
        ] {
            let field = entity.field(name).expect("native numeric field");
            let number: Value = serde_json::from_str(raw).expect("exact decimal token");
            for value in [json!(raw), number.clone()] {
                assert_eq!(
                    convert_scalar(field, &value).expect("canonical numeric source value"),
                    InventoryValue::Json {
                        value: number.clone()
                    },
                );
            }
        }
        let field = InventoryEntity::DeviceType.field("u_height").unwrap();
        for invalid in [
            json!("NaN"),
            json!("Infinity"),
            json!(" 1.5"),
            json!("1.5 "),
            json!(""),
            json!(true),
            json!({"value":"1.5"}),
            json!([]),
        ] {
            assert!(convert_scalar(field, &invalid).is_err());
        }
    }

    #[test]
    fn unknown_mapping_target_blocks_and_derived_target_blocks() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({}));
        for target in ["mystery_business_field", "display"] {
            let mappings = vec![literal(target, json!("x"))];
            assert!(
                prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings)).is_err(),
                "target {target} must be rejected"
            );
        }
    }

    #[test]
    fn missing_reference_id_blocks() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({}));
        let mappings = vec![literal("role", json!({ "display": "no id" }))];
        assert!(prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings)).is_err());
    }

    #[test]
    fn service_port_mappings_bind_ordered_text_array_not_tags() {
        let provider = Uuid::now_v7();
        let rec = record("ipam.services", "1", json!({}));
        let mappings = vec![
            literal("port_mappings", json!(["TCP/080", "udp/53", "sctp/65535"])),
            literal("tags", json!([{"id": 18, "name": "service"}])),
        ];
        let row = prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        let field = InventoryEntity::Service.field("port_mappings").unwrap();
        assert_eq!(
            row.value(field),
            Some(&InventoryValue::Json {
                value: json!(["tcp/80", "udp/53", "sctp/65535"])
            })
        );
        assert_eq!(row.tags.as_ref().expect("tags")[0].external_id, "18");
    }

    #[test]
    fn malformed_port_mapping_blocks() {
        let provider = Uuid::now_v7();
        for bad in [
            json!(["not-a-mapping"]),
            json!(["tcp/70000"]),
            json!([7]),
            json!([]),
            json!(["tcp/0"]),
            json!(["icmp/80"]),
            json!(["TCP/80", "tcp/080"]),
            json!({"value": ["tcp/80"]}),
            json!("tcp/80"),
            json!(null),
        ] {
            let rec = record("ipam.services", "1", json!({}));
            let mappings = vec![literal("port_mappings", bad.clone())];
            assert!(
                prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings)).is_err(),
                "value {bad} must be rejected"
            );
        }
    }

    #[test]
    fn missing_service_port_mappings_and_legacy_targets_block() {
        let provider = Uuid::now_v7();
        let rec = record("ipam.services", "1", json!({}));
        let mappings = vec![identity("port_mappings", "port_mappings")];
        assert!(prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings)).is_err());
        for name in ["protocol", "ports"] {
            let mappings = vec![literal(name, json!("tcp"))];
            assert!(
                prepare_row(InventoryEntity::Service, provider, &rec, &borrow(&mappings)).is_err()
            );
        }
    }

    #[test]
    fn custom_fields_are_captured_from_raw_record_or_mapping() {
        let provider = Uuid::now_v7();
        // Read straight from the raw source record when the mapping omits it.
        let rec = record(
            "dcim.devices",
            "5",
            json!({ "custom_fields": { "owner_team": "ops" }, "name": "r1" }),
        );
        let mappings = vec![identity("name", "name")];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        assert_eq!(row.custom_fields.as_ref().map(|map| map.len()), Some(1));

        // An explicit `custom_fields` mapping target is honoured too.
        let rec = record("dcim.devices", "5", json!({ "name": "r1" }));
        let mappings = vec![
            identity("name", "name"),
            literal("custom_fields", json!({ "owner_team": "ops" })),
        ];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        assert_eq!(row.custom_fields.as_ref().map(|map| map.len()), Some(1));

        // An explicitly empty object plans a clear, not a preserve.
        let rec = record("dcim.devices", "5", json!({ "custom_fields": {} }));
        let row = prepare_row(
            InventoryEntity::Device,
            provider,
            &rec,
            &borrow(&Vec::<FieldMapping>::new()),
        )
        .expect("prepared");
        assert_eq!(row.custom_fields, Some(serde_json::Map::new()));
    }

    #[test]
    fn tree_entities_are_the_nested_set_tables() {
        for entity in [
            InventoryEntity::Region,
            InventoryEntity::SiteGroup,
            InventoryEntity::Platform,
            InventoryEntity::DeviceRole,
            InventoryEntity::InventoryItem,
        ] {
            assert!(entity.is_tree(), "{entity:?} must be a tree entity");
        }
        for entity in [
            InventoryEntity::Device,
            InventoryEntity::Site,
            InventoryEntity::IpAddress,
        ] {
            assert!(!entity.is_tree(), "{entity:?} must not be a tree entity");
        }
    }

    #[test]
    fn required_tag_identity_fields_reject_absence() {
        let provider = Uuid::now_v7();
        let rec = record("extras.tags", "1", json!({}));
        for field in ["name", "slug"] {
            let mappings = vec![identity(field, field)];
            assert!(
                prepare_row(InventoryEntity::Tag, provider, &rec, &borrow(&mappings)).is_err(),
                "absent tag {field} must be rejected"
            );
        }
    }

    #[test]
    fn inventory_component_references_are_typed_and_reject_out_of_slice_targets() {
        let provider = Uuid::now_v7();
        let rec = record(
            "dcim.inventory-items",
            "1",
            json!({"name":"component item","component_type":"dcim.interface","component":{"id":1}}),
        );
        let mappings = vec![identity("name", "name"), identity("component", "component")];
        let row = prepare_row(
            InventoryEntity::InventoryItem,
            provider,
            &rec,
            &borrow(&mappings),
        )
        .expect("canonical Interface component prepares");
        let (_, binding) = row
            .references
            .iter()
            .find(|(field, _)| field.name() == "component")
            .expect("typed component binding");
        assert_eq!(binding.target, Some(InventoryEntity::Interface));
        assert_eq!(binding.type_token.as_deref(), Some("dcim.interface"));
        assert_eq!(binding.external_id, "1");
        for token in ["dcim.consoleport", "dcim.device", "dcim.interfacetemplate"] {
            let mut unsupported = rec.clone();
            unsupported
                .fields
                .insert("component_type".into(), json!(token));
            assert!(
                prepare_row(
                    InventoryEntity::InventoryItem,
                    provider,
                    &unsupported,
                    &borrow(&mappings),
                )
                .is_err(),
                "component type {token} must remain unsupported"
            );
        }
    }

    #[test]
    fn null_optional_columns_use_native_empty_encoding() {
        let provider = Uuid::now_v7();
        // DeviceType.front_image/rear_image are NOT NULL with no DB default and
        // are null in the source: native empty string, never a missing column.
        let rec = record("dcim.device-types", "1", json!({}));
        let mappings = vec![
            literal("front_image", Value::Null),
            literal("rear_image", Value::Null),
        ];
        let row = prepare_row(
            InventoryEntity::DeviceType,
            provider,
            &rec,
            &borrow(&mappings),
        )
        .expect("prepared");
        let front = InventoryEntity::DeviceType
            .field("front_image")
            .expect("front_image");
        assert_eq!(
            row.scalars.get(&front),
            Some(&InventoryValue::Text {
                value: String::new()
            })
        );
    }

    #[test]
    fn null_required_field_is_rejected_and_nullable_is_preserved() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.sites", "5", json!({}));

        // Site.name is NOT NULL; Device.name is nullable in the native schema.
        let reject = vec![literal("name", Value::Null)];
        assert!(prepare_row(InventoryEntity::Site, provider, &rec, &borrow(&reject)).is_err());
        let rec = record("dcim.devices", "5", json!({}));

        // Explicit null on a nullable scalar clears it to SQL NULL.
        let clear = vec![literal("latitude", Value::Null)];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&clear))
            .expect("prepared");
        let latitude = InventoryEntity::Device.field("latitude").expect("latitude");
        assert_eq!(row.scalars.get(&latitude), Some(&InventoryValue::Null));

        // A missing property preserves the local column (nothing planned).
        let missing = vec![identity("latitude", "latitude")];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&missing))
            .expect("prepared");
        assert!(!row.scalars.contains_key(&latitude));
    }

    #[test]
    fn explicit_null_clears_relations_and_missing_reference_preserves() {
        let provider = Uuid::now_v7();
        let rec = record("dcim.devices", "5", json!({}));

        // Explicit null on a nullable FK records a clear of the column.
        let clear = vec![literal("rack", Value::Null)];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&clear))
            .expect("prepared");
        let rack = InventoryEntity::Device.field("rack").expect("rack");
        assert_eq!(row.cleared, vec![rack]);
        assert!(row.references.is_empty());

        // Explicit null on a GFK clears both columns (type + id).
        let rec = record("dcim.mac-addresses", "9", json!({}));
        let clear = vec![literal("assigned_object", Value::Null)];
        let row = prepare_row(InventoryEntity::MacAddress, provider, &rec, &borrow(&clear))
            .expect("prepared");
        let gfk = InventoryEntity::MacAddress
            .field("assigned_object")
            .expect("assigned_object");
        assert!(row.cleared.contains(&gfk));

        // Missing FK property preserves the local column.
        let rec = record("dcim.devices", "5", json!({}));
        let missing = vec![identity("rack", "rack")];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&missing))
            .expect("prepared");
        assert!(row.cleared.is_empty());
        assert!(row.references.is_empty());

        // A NOT NULL relation cannot be cleared.
        let rec = record("virtualization.clusters", "1", json!({}));
        let clear_type = vec![literal("type", Value::Null)];
        assert!(
            prepare_row(
                InventoryEntity::Cluster,
                provider,
                &rec,
                &borrow(&clear_type)
            )
            .is_err()
        );
    }
    #[test]
    fn imported_custom_field_definitions_are_validated_canonically() {
        let good = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "owner_team", "type": { "value": "text" }, "unique": false, "required": false }),
        );
        assert!(validate_source_custom_field(&good).is_ok());

        let bad_name = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "Bad Name", "type": { "value": "text" } }),
        );
        assert!(validate_source_custom_field(&bad_name).is_err());

        let unknown_type = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "ok", "type": { "value": "nope" } }),
        );
        assert!(validate_source_custom_field(&unknown_type).is_err());
    }
    #[test]
    fn populated_out_of_scope_reference_is_recorded_and_unresolvable() {
        let provider = Uuid::now_v7();
        // `config_template` targets an out-of-scope table: the reference is
        // recorded (never silently dropped) and cannot be resolved, so the
        // commit planner fails rather than leaving the column unset.
        let rec = record("dcim.devices", "5", json!({}));
        let mappings = vec![literal("config_template", json!({ "id": 7 }))];
        let row = prepare_row(InventoryEntity::Device, provider, &rec, &borrow(&mappings))
            .expect("prepared");
        let (_, binding) = row.references.first().expect("reference");
        assert_eq!(binding.target, None);
        assert_eq!(binding.external_id, "7");
        assert!(resolve_reference(binding, &HashMap::new()).is_none());
    }
    #[test]
    fn source_custom_field_definition_is_returned_for_the_incoming_cache() {
        let rec = record(
            "extras.custom-fields",
            "1",
            json!({ "name": "meta", "type": { "value": "json" }, "required": false }),
        );
        let definition = validate_source_custom_field(&rec).expect("valid definition");
        assert_eq!(definition.name, "meta");
        assert_eq!(definition.field_type, CustomFieldType::Json);
        // The definition is the cache payload keyed by (name, scope), so a
        // value validated against it needs no DB row.
        assert_eq!(
            validate_source_custom_field(&rec).expect("again").name,
            "meta"
        );
    }
}
