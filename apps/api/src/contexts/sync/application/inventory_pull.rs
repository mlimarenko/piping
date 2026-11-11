//! Canonical local-inventory pull orchestration (PIP-86).
//!
//! One path: resolve the immutable mapping configuration, reject unsupported
//! job shapes before any read, take the provider lease, read the full enabled
//! source manifest page by page, hand the snapshot to the typed local target,
//! and let the target commit rows, bindings, the report and the job's terminal
//! state in one transaction.
//!
//! The pull is read-only towards the remote provider: it never issues a
//! POST/PATCH/DELETE, never bootstraps a webhook, and never auto-adopts a row by
//! matching a mutable name.

use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use super::commit_authorisation::{RequestCommitAuthorisation, RequestCommitAuthority};
use super::lock::{ProviderLease, ProviderLeaseError};
use crate::contexts::access_control::domain::Action;
use crate::contexts::shared::request_context::RequestContext;
use crate::contexts::sync::domain::{
    CommitPull, FieldMapping, InventoryCommitAuthorisation, InventoryEntity, InventorySource,
    LocalInventoryTarget, MappingConfigDocument, MappingRef, SourceEntity, SourceSnapshot, SyncJob,
    SyncStatus, TransformDirection, evaluate_expression,
};
use crate::contexts::sync::infrastructure::persistence::{
    MappingConfigurationRepository, SqlxSyncJobRepository, SyncJobRepository,
};
use crate::errors::AppError;
use crate::observability::{instrument_span, operation_span};

pub use crate::contexts::sync::domain::inventory_sync::collect_snapshot;

/// The enabled source entities of a local pull, after checking the
/// configuration maps every entity the source exposes.
///
/// A local pull ingests the whole enabled manifest; a mapping that skips an
/// entity is rejected before any write so a later withdrawal pass cannot delete
/// rows the operator never mapped. The optional `data-flows` namespace is the
/// one exception: when the provider does not expose the plugin, a mapping that
/// still names those entities is withheld rather than rejected, and the
/// snapshot records the capability so no absence can be confirmed for them.
pub fn require_full_manifest_coverage(
    document: &MappingConfigDocument,
    supported: &[SourceEntity],
) -> Result<Vec<SourceEntity>, AppError> {
    let mut enabled = Vec::new();
    for entity in &document.entity_mappings {
        if !entity.enabled {
            continue;
        }
        let parsed = SourceEntity::new(&entity.source_entity).map_err(|error| {
            AppError::Validation(format!(
                "entity mapping has an invalid source entity: {error}"
            ))
        })?;
        if !supported.contains(&parsed) {
            if is_optional_source_entity(&parsed) {
                continue;
            }
            return Err(AppError::Validation(format!(
                "Unsupported source entity '{}'",
                parsed.as_str()
            )));
        }
        if !enabled.contains(&parsed) {
            enabled.push(parsed);
        }
    }

    for supported_entity in supported {
        if !enabled.contains(supported_entity) {
            return Err(AppError::Validation(format!(
                "local inventory pull requires a mapping for every source entity; '{}' is missing",
                supported_entity.as_str()
            )));
        }
    }

    Ok(enabled)
}

/// Whether a mapping entity belongs to the optional plugin namespace.
fn is_optional_source_entity(entity: &SourceEntity) -> bool {
    InventoryEntity::from_source_key(entity.as_str())
        .is_some_and(InventoryEntity::is_optional_source)
}

/// Evaluate every forward transform against the fetched snapshot before the
/// target writes. A configuration with an unmapped entity, an entity with no
/// field mappings, or a forward expression that cannot be evaluated on a real
/// record is rejected here.
pub fn validate_forward_expressions(
    document: &MappingConfigDocument,
    snapshot: &SourceSnapshot,
) -> Result<(), AppError> {
    for entity in &document.entity_mappings {
        if !entity.enabled {
            continue;
        }
        let source_entity = SourceEntity::new(&entity.source_entity)?;
        if InventoryEntity::from_source_key(source_entity.as_str())
            .is_some_and(InventoryEntity::is_membership)
        {
            continue;
        }
        let fields: Vec<&FieldMapping> = document
            .field_mappings
            .iter()
            .filter(|field| field.entity_mapping_id == entity.id)
            .collect();
        if fields.is_empty() {
            return Err(AppError::Validation(format!(
                "entity '{}' has no field mappings",
                source_entity.as_str()
            )));
        }

        for record in snapshot
            .records
            .iter()
            .filter(|record| record.entity() == &source_entity)
        {
            let sample = Value::Object(record.fields.clone());
            for field in &fields {
                evaluate_expression(
                    &field.forward_transform_json,
                    &sample,
                    TransformDirection::Forward,
                )
                .map_err(|error| {
                    AppError::Validation(format!(
                        "forward transform for '{}.{}' failed: {}",
                        source_entity.as_str(),
                        field.target_field,
                        error
                    ))
                })?;
            }
        }
    }
    Ok(())
}

/// A busy provider was rejected before a sync job was created.
#[derive(Debug, thiserror::Error)]
pub enum InventoryPullError {
    #[error("Sync already running for provider {0}")]
    Busy(Uuid),
    #[error(transparent)]
    Failed(#[from] AppError),
}

/// The occurrence is recorded only after the shared provider lease is held.
pub trait ScheduledPullAdmission: Send + Sync {
    fn record_job(&self, job_id: Uuid) -> BoxFuture<'_, Result<(), AppError>>;
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>>;
}

struct ScheduledLease {
    provider: Arc<dyn InventoryCommitAuthorisation>,
    occurrence: Arc<dyn ScheduledPullAdmission>,
}

impl InventoryCommitAuthorisation for ScheduledLease {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            self.provider.ensure_owned().await?;
            self.occurrence.ensure_owned().await
        })
    }

    fn ensure_reference_access(
        &self,
        id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        self.provider.ensure_reference_access(id, attributes)
    }

    fn ensure_mutation_access(
        &self,
        action: Action,
        id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        self.provider.ensure_mutation_access(action, id, attributes)
    }
}

impl From<ProviderLeaseError> for InventoryPullError {
    fn from(error: ProviderLeaseError) -> Self {
        match error {
            ProviderLeaseError::Busy(provider) => Self::Busy(provider),
            ProviderLeaseError::Unavailable(error) => Self::Failed(error),
        }
    }
}

impl From<InventoryPullError> for AppError {
    fn from(error: InventoryPullError) -> Self {
        match error {
            InventoryPullError::Busy(provider) => ProviderLeaseError::Busy(provider).into(),
            InventoryPullError::Failed(error) => error,
        }
    }
}

/// Orchestrates one local-inventory pull for an immutable mapping reference.
pub struct InventoryPullService<CRepo, Source, Target> {
    configs: CRepo,
    jobs: SqlxSyncJobRepository,
    source: Source,
    target: Target,
    redis: ConnectionManager,
    lock_ttl_seconds: usize,
    request_authority: Arc<dyn RequestCommitAuthority>,
}

impl<CRepo, Source, Target> InventoryPullService<CRepo, Source, Target>
where
    CRepo: MappingConfigurationRepository,
    Source: InventorySource,
    Target: LocalInventoryTarget,
{
    pub fn new(
        configs: CRepo,
        jobs: SqlxSyncJobRepository,
        source: Source,
        target: Target,
        redis: ConnectionManager,
        lock_ttl_seconds: usize,
        request_authority: Arc<dyn RequestCommitAuthority>,
    ) -> Self {
        Self {
            configs,
            jobs,
            source,
            target,
            redis,
            lock_ttl_seconds,
            request_authority,
        }
    }

    /// Run one pull on behalf of `request`. `mapping_version` is the immutable
    /// `"<logical_uuid>:<version>"` reference; there is no free-form legacy
    /// version.
    ///
    /// The caller's request travels with the job: the lease is wrapped in a
    /// [`RequestCommitAuthorisation`] so the writer re-checks the caller's
    /// current ceilings instead of trusting the lease alone.
    pub async fn run(
        &self,
        provider_id: Uuid,
        mapping_version: &str,
        request_context: RequestContext,
    ) -> Result<SyncJob, AppError> {
        self.run_with_admission(provider_id, mapping_version, request_context)
            .await
            .map_err(Into::into)
    }

    /// Run the same pull while distinguishing provider contention from failures.
    pub async fn run_with_admission(
        &self,
        provider_id: Uuid,
        mapping_version: &str,
        request_context: RequestContext,
    ) -> Result<SyncJob, InventoryPullError> {
        self.run_internal(provider_id, mapping_version, request_context, None)
            .await
    }

    async fn run_internal(
        &self,
        provider_id: Uuid,
        mapping_version: &str,
        request_context: RequestContext,
        occurrence: Option<Arc<dyn ScheduledPullAdmission>>,
    ) -> Result<SyncJob, InventoryPullError> {
        let span = operation_span("sync.pull", &[("provider_id", provider_id.to_string())]);
        instrument_span(span, async {
            let reference = MappingRef::parse(mapping_version)?;
            let document = self
                .configs
                .get_document(reference.logical_id, reference.version_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Mapping configuration not found".to_string()))?;

            if document.source_provider_id != provider_id {
                return Err(AppError::Validation(
                    "mapping configuration does not belong to the requested provider".to_string(),
                )
                .into());
            }
            if document.status != crate::contexts::sync::domain::MappingConfigurationStatus::Active
            {
                return Err(
                    AppError::Validation("Mapping configuration must be active".into()).into(),
                );
            }

            let request = document.target_sync_request();
            // Reject push/bidirectional/provider-target/non-NetBox-wins before
            // any source read.
            request.ensure_supported()?;

            let lease =
                ProviderLease::acquire(self.redis.clone(), provider_id, self.lock_ttl_seconds)
                    .await?;
            let job = SyncJob {
                id: Uuid::now_v7(),
                provider_id,
                mapping_version: request.reference.to_wire(),
                status: SyncStatus::Running,
                started_at: Some(chrono::Utc::now()),
                finished_at: None,
                stats: serde_json::json!({}),
            };
            let result = lease
                .run(|lease_authorisation| async {
                    let lease_authorisation: Arc<dyn InventoryCommitAuthorisation> =
                        if let Some(occurrence) = occurrence {
                            occurrence.record_job(job.id).await?;
                            Arc::new(ScheduledLease {
                                provider: lease_authorisation,
                                occurrence,
                            })
                        } else {
                            lease_authorisation
                        };
                    let authorisation: Arc<dyn InventoryCommitAuthorisation> =
                        Arc::new(RequestCommitAuthorisation::new(
                            lease_authorisation,
                            self.request_authority.clone(),
                            request_context.clone(),
                        ));
                    let job = self.jobs.create(job.clone()).await?;
                    self.run_locked(job, &document, authorisation).await
                })
                .await;
            self.finish_run(job, result).await.map_err(Into::into)
        })
        .await
    }

    async fn run_locked(
        &self,
        job: SyncJob,
        document: &MappingConfigDocument,
        authorisation: Arc<dyn InventoryCommitAuthorisation>,
    ) -> Result<SyncJob, AppError> {
        let supported = self.source.supported_entities();
        let enabled = require_full_manifest_coverage(document, &supported)?;
        let snapshot = collect_snapshot(&self.source, &enabled).await?;
        validate_forward_expressions(document, &snapshot)?;

        let batch = self.target.prepare(document, &snapshot)?;
        authorisation.ensure_owned().await?;
        self.target
            .commit_pull(CommitPull {
                job,
                batch,
                authorisation,
            })
            .await
    }

    async fn finish_run(
        &self,
        mut job: SyncJob,
        result: Result<SyncJob, AppError>,
    ) -> Result<SyncJob, AppError> {
        match result {
            Ok(completed) => Ok(completed),
            Err(error) => {
                // A lost commit response must not turn a durable completed job into a failure.
                if let Some(persisted) = self.jobs.get_by_id(job.id).await?
                    && persisted.status == SyncStatus::Completed
                {
                    return Ok(persisted);
                }
                job.status = SyncStatus::Failed;
                job.finished_at = Some(chrono::Utc::now());
                job.stats = serde_json::json!({"failed_count": 1, "error_code": "PULL_FAILED"});
                if self.jobs.fail_if_running(&job).await?.is_none()
                    && let Some(persisted) = self.jobs.get_by_id(job.id).await?
                    && persisted.status == SyncStatus::Completed
                {
                    return Ok(persisted);
                }
                Err(error)
            }
        }
    }
}

/// Object-safe runner used by the REST wiring. The generic service stays
/// unboxed for direct callers; the route holds this trait object so it never
/// names the concrete target type.
pub trait InventoryPullRunner: Send + Sync {
    fn run_scheduled<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
        request: RequestContext,
        occurrence: Arc<dyn ScheduledPullAdmission>,
    ) -> BoxFuture<'a, Result<SyncJob, InventoryPullError>>;

    /// Run a pull with typed lease admission for retrying callers.
    fn run_pull_with_admission<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
        request: RequestContext,
    ) -> BoxFuture<'a, Result<SyncJob, InventoryPullError>>;

    /// Adapt pull admission to the existing request error contract.
    fn run_pull<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
        request: RequestContext,
    ) -> BoxFuture<'a, Result<SyncJob, AppError>> {
        Box::pin(async move {
            self.run_pull_with_admission(provider_id, mapping_version, request)
                .await
                .map_err(Into::into)
        })
    }
}

impl<CRepo, Source, Target> InventoryPullRunner for InventoryPullService<CRepo, Source, Target>
where
    CRepo: MappingConfigurationRepository,
    Source: InventorySource,
    Target: LocalInventoryTarget,
{
    fn run_scheduled<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
        request: RequestContext,
        occurrence: Arc<dyn ScheduledPullAdmission>,
    ) -> BoxFuture<'a, Result<SyncJob, InventoryPullError>> {
        Box::pin(self.run_internal(provider_id, mapping_version, request, Some(occurrence)))
    }

    fn run_pull_with_admission<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
        request: RequestContext,
    ) -> BoxFuture<'a, Result<SyncJob, InventoryPullError>> {
        Box::pin(InventoryPullService::run_with_admission(
            self,
            provider_id,
            mapping_version,
            request,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::sync::domain::{
        MappingDirection, MappingTarget, SourceIdentity, SourcePage, SourceRecord,
    };
    use serde_json::json;

    #[test]
    fn only_lease_contention_is_a_busy_pull() {
        let provider = Uuid::now_v7();
        assert!(matches!(
            InventoryPullError::from(ProviderLeaseError::Busy(provider)),
            InventoryPullError::Busy(id) if id == provider
        ));
        assert!(matches!(
            InventoryPullError::from(ProviderLeaseError::Unavailable(AppError::Conflict(
                "other conflict".into()
            ))),
            InventoryPullError::Failed(AppError::Conflict(_))
        ));
        assert!(matches!(
            AppError::from(InventoryPullError::Busy(provider)),
            AppError::Conflict(_)
        ));
    }

    struct FakeSource {
        entities: Vec<SourceEntity>,
        pages: Vec<SourcePage>,
    }

    impl crate::contexts::sync::domain::inventory_sync::inventory_source_seal::Sealed for FakeSource {}

    impl InventorySource for FakeSource {
        fn supported_entities(&self) -> Vec<SourceEntity> {
            self.entities.clone()
        }

        async fn fetch_page(
            &self,
            entity: &SourceEntity,
            cursor: Option<&str>,
        ) -> Result<SourcePage, AppError> {
            let index = cursor
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let _ = entity;
            self.pages
                .get(index)
                .cloned()
                .ok_or_else(|| AppError::Validation("fixture page unavailable".into()))
        }
    }

    fn entity(raw: &str) -> SourceEntity {
        SourceEntity::new(raw).expect("valid entity")
    }

    #[tokio::test]
    async fn collect_snapshot_follows_cursors_until_exhausted() {
        let provider_id = Uuid::now_v7();
        let record = |id: &str| SourceRecord {
            identity: SourceIdentity::new(provider_id, entity("dcim.sites"), id).expect("identity"),
            fields: serde_json::Map::new(),
        };
        let source = FakeSource {
            entities: vec![entity("dcim.sites")],
            pages: vec![
                SourcePage {
                    records: vec![record("1")],
                    next_cursor: Some("1".to_string()),
                },
                SourcePage {
                    records: vec![record("2")],
                    next_cursor: None,
                },
            ],
        };
        let snapshot = collect_snapshot(&source, &[entity("dcim.sites")])
            .await
            .expect("snapshot");
        assert_eq!(snapshot.len(), 2);
    }

    #[tokio::test]
    async fn collect_snapshot_rejects_duplicate_identity_and_stalled_cursor() {
        let provider = Uuid::now_v7();
        let record = SourceRecord::new(
            SourceIdentity::new(provider, entity("dcim.sites"), "1").expect("identity"),
            serde_json::Map::new(),
        );
        let duplicate = FakeSource {
            entities: vec![entity("dcim.sites")],
            pages: vec![
                SourcePage {
                    records: vec![record.clone()],
                    next_cursor: Some("1".into()),
                },
                SourcePage {
                    records: vec![record],
                    next_cursor: None,
                },
            ],
        };
        assert!(
            collect_snapshot(&duplicate, &duplicate.entities)
                .await
                .is_err()
        );
        let stalled = FakeSource {
            entities: vec![entity("dcim.sites")],
            pages: vec![
                SourcePage {
                    records: vec![],
                    next_cursor: Some("1".into()),
                },
                SourcePage {
                    records: vec![],
                    next_cursor: Some("1".into()),
                },
            ],
        };
        assert!(collect_snapshot(&stalled, &stalled.entities).await.is_err());
        let failed_page = FakeSource {
            entities: vec![entity("dcim.sites")],
            pages: vec![SourcePage {
                records: vec![],
                next_cursor: Some("1".into()),
            }],
        };
        assert!(
            collect_snapshot(&failed_page, &failed_page.entities)
                .await
                .is_err()
        );
    }

    fn document(entity_name: &str, forward: Value) -> MappingConfigDocument {
        use crate::contexts::sync::domain::{
            ConflictMode, EntityMapping, MappingConfigurationStatus,
        };
        let logical_id = Uuid::now_v7();
        let entity_id = Uuid::now_v7();
        MappingConfigDocument {
            absence_policy: Default::default(),
            reference: MappingRef::new(logical_id, 1),
            name: "preset".to_string(),
            description: None,
            source_provider_id: Uuid::now_v7(),
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: chrono::Utc::now(),
            retired_at: None,
            entity_mappings: vec![EntityMapping {
                id: entity_id,
                mapping_config_logical_id: logical_id,
                mapping_config_version_id: 1,
                source_entity: entity_name.to_string(),
                target_entity: "piping.inventory.site".to_string(),
                enabled: true,
                created_at: chrono::Utc::now(),
            }],
            field_mappings: vec![FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: entity_id,
                source_field: "name".to_string(),
                target_field: "name".to_string(),
                forward_transform_json: forward,
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: chrono::Utc::now(),
            }],
        }
    }

    struct EmptyManifestSource {
        provider: Uuid,
        entities: Vec<SourceEntity>,
    }

    impl crate::contexts::sync::domain::inventory_sync::inventory_source_seal::Sealed
        for EmptyManifestSource
    {
    }

    impl InventorySource for EmptyManifestSource {
        fn supported_entities(&self) -> Vec<SourceEntity> {
            self.entities.clone()
        }
        fn source_provider_id(&self) -> Option<Uuid> {
            Some(self.provider)
        }
        async fn fetch_page(
            &self,
            entity: &SourceEntity,
            cursor: Option<&str>,
        ) -> Result<SourcePage, AppError> {
            if !self.entities.contains(entity) || cursor.is_some() {
                return Err(AppError::Validation(
                    "fixture collection unavailable".into(),
                ));
            }
            Ok(SourcePage {
                records: vec![],
                next_cursor: None,
            })
        }
    }

    #[tokio::test]
    async fn collector_distinguishes_complete_empty_manifest_from_omission() {
        let source = EmptyManifestSource {
            provider: Uuid::now_v7(),
            entities: InventoryEntity::core()
                .chain([
                    InventoryEntity::TaggedObject,
                    InventoryEntity::CableTermination,
                ])
                .map(|entity| SourceEntity::new(entity.source_key()).expect("catalog key"))
                .collect(),
        };
        let snapshot = collect_snapshot(&source, &source.entities)
            .await
            .expect("complete manifest");
        let coverage = snapshot.coverage.as_ref().expect("collector evidence");
        assert_eq!(coverage.completed_entities().len(), source.entities.len());
        assert!(snapshot.records.is_empty());
        assert!(
            collect_snapshot(&source, &source.entities[1..])
                .await
                .is_err()
        );

        let mut config = document("dcim.sites", json!({}));
        config.source_provider_id = source.provider;
        let template = config.entity_mappings[0].clone();
        config.entity_mappings = source
            .entities
            .iter()
            .map(|entity| {
                let mut mapping = template.clone();
                mapping.id = Uuid::now_v7();
                mapping.source_entity = entity.as_str().to_owned();
                mapping.target_entity = InventoryEntity::from_source_key(entity.as_str())
                    .expect("catalog entity")
                    .target_key();
                mapping
            })
            .collect();
        config.field_mappings.clear();
        assert!(coverage.matches(&snapshot, &config));
        let batch = crate::contexts::sync::infrastructure::persistence::local_inventory_target::assess_preparation(&config, &snapshot)
            .batch.expect("empty manifest prepares");
        assert_eq!(
            batch
                .source_coverage()
                .expect("batch coverage")
                .completed_entities()
                .len(),
            source.entities.len()
        );
        assert_eq!(batch.mapping_digest(), config.content_digest());
        let mut wrong_provider = config.clone();
        wrong_provider.source_provider_id = Uuid::now_v7();
        assert!(!coverage.matches(&snapshot, &wrong_provider));
        let mut modified_snapshot = snapshot.clone();
        modified_snapshot.records.push(SourceRecord::new(
            SourceIdentity::new(source.provider, entity("dcim.sites"), "1").expect("identity"),
            serde_json::Map::new(),
        ));
        assert!(!coverage.matches(&modified_snapshot, &config));
        config.entity_mappings.pop();
        assert!(!coverage.matches(&snapshot, &config));
    }

    #[test]
    fn unsealed_snapshot_cannot_prepare_destructive_policy() {
        use crate::contexts::sync::domain::{AbsencePolicy, inventory_preflight::FindingCode};
        use crate::contexts::sync::infrastructure::persistence::local_inventory_target::assess_preparation;

        let mut config = document("dcim.sites", json!({}));
        config.field_mappings.clear();
        let snapshot = SourceSnapshot::unsealed(vec![]);
        let retained = assess_preparation(&config, &snapshot)
            .batch
            .expect("retain import");
        assert!(retained.source_coverage().is_none());
        config.absence_policy = AbsencePolicy::DeleteSourceOwned;
        let result = assess_preparation(&config, &snapshot);
        assert!(result.batch.is_none());
        assert!(
            result
                .findings
                .iter()
                .any(|finding| finding.code == FindingCode::SourceAuthorityUnattested)
        );
    }

    #[test]
    fn full_manifest_coverage_rejects_a_missing_entity() {
        let doc = document(
            "dcim.sites",
            json!({"op": "identity", "args": [{"field": "name"}]}),
        );
        let error =
            require_full_manifest_coverage(&doc, &[entity("dcim.sites"), entity("dcim.regions")])
                .expect_err("missing entity must be rejected");
        assert!(error.to_string().contains("dcim.regions"));
    }

    #[test]
    fn forward_expression_is_evaluated_on_real_records() {
        let provider_id = Uuid::now_v7();
        let mut doc = document(
            "dcim.sites",
            json!({"op": "identity", "args": [{"field": "name"}]}),
        );
        doc.source_provider_id = provider_id;
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![SourceRecord {
                identity: SourceIdentity::new(provider_id, entity("dcim.sites"), "1")
                    .expect("identity"),
                fields: json!({ "name": "north" })
                    .as_object()
                    .cloned()
                    .expect("object"),
            }],
        };
        validate_forward_expressions(&doc, &snapshot).expect("forward expression evaluates");
    }
}
