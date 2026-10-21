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
use std::time::Duration;
use uuid::Uuid;

use super::lock::{SyncLock, SyncLockGuard};
use crate::contexts::sync::domain::{
    CommitPull, FieldMapping, InventoryCommitAuthorisation, InventoryEntity, InventorySource,
    LocalInventoryTarget, MappingConfigDocument, MappingRef, SourceEntity, SourceSnapshot, SyncJob,
    SyncStatus, TransformDirection, evaluate_expression,
};
use crate::contexts::sync::infrastructure::persistence::{
    MappingConfigurationRepository, SqlxSyncJobRepository, SyncJobRepository,
};
use crate::errors::AppError;
use crate::observability::{EventFields, instrument_span, log_error_event, operation_span};

/// Read every page of every requested source entity into one snapshot.
///
/// A `None` cursor ends an entity; the loop is bounded by the source's own
/// cursor contract, not by a fixed page count.
pub async fn collect_snapshot<Source: InventorySource>(
    source: &Source,
    entities: &[SourceEntity],
) -> Result<SourceSnapshot, AppError> {
    let mut records = Vec::new();
    for entity in entities {
        let mut cursor: Option<String> = None;
        loop {
            let page = source.fetch_page(entity, cursor.as_deref()).await?;
            records.extend(page.records);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }
    Ok(SourceSnapshot { records })
}

/// The enabled source entities of a local pull, after checking the
/// configuration maps every entity the source exposes.
///
/// A local pull ingests the whole enabled manifest; a mapping that skips an
/// entity is rejected before any write so a later withdrawal pass cannot delete
/// rows the operator never mapped.
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

/// Orchestrates one local-inventory pull for an immutable mapping reference.
pub struct InventoryPullService<CRepo, Source, Target> {
    configs: CRepo,
    jobs: SqlxSyncJobRepository,
    source: Source,
    target: Target,
    redis: ConnectionManager,
    lock_ttl_seconds: usize,
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
    ) -> Self {
        Self {
            configs,
            jobs,
            source,
            target,
            redis,
            lock_ttl_seconds,
        }
    }

    /// Run one pull. `mapping_version` is the immutable
    /// `"<logical_uuid>:<version>"` reference; there is no free-form legacy
    /// version.
    pub async fn run(&self, provider_id: Uuid, mapping_version: &str) -> Result<SyncJob, AppError> {
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
                ));
            }
            if document.status != crate::contexts::sync::domain::MappingConfigurationStatus::Active
            {
                return Err(AppError::Validation(
                    "Mapping configuration must be active".into(),
                ));
            }

            let request = document.target_sync_request();
            // Reject push/bidirectional/provider-target/non-NetBox-wins before
            // any source read.
            request.ensure_supported()?;

            let mut redis = self.redis.clone();
            let lock = SyncLock::new(
                format!("sync:provider:{provider_id}"),
                self.lock_ttl_seconds,
            );
            let Some(guard) = lock.acquire(&mut redis).await? else {
                return Err(AppError::Conflict(format!(
                    "Sync already running for provider {provider_id}"
                )));
            };

            let guard = Arc::new(guard);
            let job = SyncJob {
                id: Uuid::now_v7(),
                provider_id,
                mapping_version: request.reference.to_wire(),
                status: SyncStatus::Running,
                started_at: Some(chrono::Utc::now()),
                finished_at: None,
                stats: serde_json::json!({}),
            };
            let job = match self.jobs.create(job).await {
                Ok(job) => job,
                Err(error) => {
                    let _ = lock.release(&mut redis, &guard).await;
                    return Err(error);
                }
            };
            let authorisation: Arc<dyn InventoryCommitAuthorisation> =
                Arc::new(LeaseAuthorisation {
                    lock: lock.clone(),
                    guard: guard.clone(),
                    redis: self.redis.clone(),
                });
            let result = {
                let work = self.run_locked(job.clone(), &document, authorisation.clone());
                tokio::pin!(work);
                let period = Duration::from_millis(
                    ((self.lock_ttl_seconds as u64).saturating_mul(1000) / 3).max(1),
                );
                let mut renewals = tokio::time::interval(period);
                renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        result = &mut work => break result,
                        _ = renewals.tick() => {
                            if let Err(error) = authorisation.ensure_owned().await {
                                break Err(error);
                            }
                        }
                    }
                }
            };
            let result = self.finish_run(job, result).await;

            if let Err(error) = lock.release(&mut redis, &guard).await {
                // The target's transaction at commit is the authoritative
                // guard; a failed lease release is logged, not fatal, and the
                // TTL bounds the window.
                log_error_event(
                    "sync.pull.lease.release.failed",
                    "sync_lock_release_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("sync.pull.release_lock")
                        .with_resource("provider", provider_id.to_string())
                        .with_message("Failed to release pull lease"),
                );
            }

            result
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

struct LeaseAuthorisation {
    lock: SyncLock,
    guard: Arc<SyncLockGuard>,
    redis: ConnectionManager,
}

impl InventoryCommitAuthorisation for LeaseAuthorisation {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            let mut redis = self.redis.clone();
            match self.lock.renew(&mut redis, &self.guard).await {
                Ok(true) => Ok(()),
                Ok(false) => Err(AppError::Conflict("Sync lease ownership was lost".into())),
                Err(_) => Err(AppError::Redis(
                    "Sync lease ownership cannot be verified".into(),
                )),
            }
        })
    }
}

/// Object-safe runner used by the REST wiring. The generic service stays
/// unboxed for direct callers; the route holds this trait object so it never
/// names the concrete target type.
pub trait InventoryPullRunner: Send + Sync {
    fn run_pull<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
    ) -> BoxFuture<'a, Result<SyncJob, AppError>>;
}

impl<CRepo, Source, Target> InventoryPullRunner for InventoryPullService<CRepo, Source, Target>
where
    CRepo: MappingConfigurationRepository,
    Source: InventorySource,
    Target: LocalInventoryTarget,
{
    fn run_pull<'a>(
        &'a self,
        provider_id: Uuid,
        mapping_version: &'a str,
    ) -> BoxFuture<'a, Result<SyncJob, AppError>> {
        Box::pin(InventoryPullService::run(
            self,
            provider_id,
            mapping_version,
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

    struct FakeSource {
        entities: Vec<SourceEntity>,
        pages: Vec<SourcePage>,
    }

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
            let page = self.pages.get(index).cloned().unwrap_or(SourcePage {
                records: Vec::new(),
                next_cursor: None,
            });
            let _ = entity;
            Ok(page)
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

    fn document(entity_name: &str, forward: Value) -> MappingConfigDocument {
        use crate::contexts::sync::domain::{
            ConflictMode, EntityMapping, MappingConfigurationStatus,
        };
        let logical_id = Uuid::now_v7();
        let entity_id = Uuid::now_v7();
        MappingConfigDocument {
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
