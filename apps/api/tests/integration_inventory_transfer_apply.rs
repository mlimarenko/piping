//! Canonical writer tests on an owned child database, never on the provider DB.
//! The shared child fixture and `pwa81_transfer_` prefix are supplied by Main.
#[path = "common/child_database.rs"]
mod child_database;
mod common;

use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use futures_util::{FutureExt, future::BoxFuture};
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator};
use uuid::Uuid;

use child_database::ChildDatabase;
use piping_backend::config::ApiTokenConfig;
use piping_backend::contexts::access_control::application::{
    CreateUserApiTokenInput, UserApiTokenService, user_api_token_scope_catalog_pairs,
};
use piping_backend::contexts::access_control::domain::vocabulary::{
    ATTR_DEVICE_TYPE_ID, ATTR_OBJECT_TYPE,
};
use piping_backend::contexts::access_control::domain::{Action, ResourceType, UserApiTokenScope};
use piping_backend::contexts::access_control::infrastructure::persistence::{
    SqlxUserApiTokenRepository, SqlxUserRepository,
};
use piping_backend::contexts::sync::domain::inventory_export::{
    CaptureOutcome, EdgeCoverage, SourceAttestationIdentity,
};
use piping_backend::contexts::sync::domain::inventory_preflight::{
    AssessmentOutcome, ExpectedSnapshotAuthority, FindingCode, ImportCapability, PreflightProfile,
    PreflightReport, Severity, catalog_digest, tool_digest,
};
use piping_backend::contexts::sync::domain::inventory_sync::{
    CommitPull, InventoryCommitAuthorisation, LocalInventoryTarget, TypedInventoryBatch,
    TypedInventoryRow,
};
use piping_backend::contexts::sync::domain::inventory_target_schema::{
    Absent, FieldKind, InventoryEntity,
};
use piping_backend::contexts::sync::domain::inventory_transfer::{
    PinnedTransferDescriptor, RetainAbsencePolicy, TRANSFER_FORMAT_VERSION, TransferApproval,
    TransferDescriptor, TransferOperator, TransferReceipt, TransferRunState, TransferScope,
};
use piping_backend::contexts::sync::domain::sync_job::{SyncJob, SyncStatus};
use piping_backend::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, MappingConfigDocument, MappingConfigurationStatus,
    MappingDirection, MappingRef, MappingTarget, SourceEntity, SourceIdentity, SourceRecord,
    SourceSnapshot,
};
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::{
    LocalInventoryBatch, LocalInventoryCatalog, assess_preparation,
};
use piping_backend::errors::AppError;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Authorisation {
    calls: AtomicUsize,
    deny_at: usize,
}

impl InventoryCommitAuthorisation for Authorisation {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if call == self.deny_at {
                Err(AppError::Conflict("synthetic authorisation lost".into()))
            } else {
                Ok(())
            }
        })
    }

    // Reference reads do not consume this writer fixture's fence call counter.
    // The request-authorisation controls exercise the real READ decision.
    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    // This fixture models the provider lease for the transfer writer. The
    // object-level CREATE/UPDATE decision belongs to the request authorisation
    // above it (and to `OperatorCommitAuthorisation` in production), so this
    // double grants it without consuming the fence counter.
    fn ensure_mutation_access(
        &self,
        _action: Action,
        _id: Uuid,
        _attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
}

fn authorisation(deny_at: usize) -> Arc<dyn InventoryCommitAuthorisation> {
    Arc::new(Authorisation {
        calls: AtomicUsize::new(0),
        deny_at,
    })
}

fn map_tag_scope(config: &mut MappingConfigDocument) -> TestResult {
    let mapping = config
        .entity_mappings
        .iter()
        .find(|mapping| mapping.source_entity == InventoryEntity::Tag.source_key())
        .ok_or("missing tag mapping")?;
    config.field_mappings.push(FieldMapping {
        id: Uuid::now_v7(),
        entity_mapping_id: mapping.id,
        source_field: "object_types".into(),
        target_field: "object_types".into(),
        forward_transform_json: json!({"op":"identity","args":[{"field":"object_types"}]}),
        reverse_transform_json: None,
        round_trip_critical: false,
        created_at: Utc::now(),
    });
    Ok(())
}

fn set_tag_scope(source: &mut SourceSnapshot, scope: Option<Value>) {
    let tag = source
        .records
        .iter_mut()
        .find(|row| row.identity.entity.as_str() == InventoryEntity::Tag.source_key())
        .expect("fixture tag");
    if let Some(scope) = scope {
        tag.fields.insert("object_types".into(), scope);
    } else {
        tag.fields.remove("object_types");
    }
}

async fn tag_scope_snapshot(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('inventory', (SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM inventory_tag t), 'scope', (SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY tag_id,content_type_id),'[]') FROM inventory_tag_object_type s), 'sites', (SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY id),'[]') FROM dcim_site s), 'owners', (SELECT COALESCE(jsonb_agg(to_jsonb(o) ORDER BY id),'[]') FROM users_owner o), 'groups', (SELECT COALESCE(jsonb_agg(to_jsonb(g) ORDER BY id),'[]') FROM users_ownergroup g), 'bindings', (SELECT COALESCE(jsonb_agg(to_jsonb(b) ORDER BY source_entity,external_id),'[]') FROM sync_inventory_binding b), 'jobs', (SELECT COALESCE(jsonb_agg(to_jsonb(j) ORDER BY id),'[]') FROM sync_job j))").fetch_one(pool).await?)
}

async fn running_scope_job(pool: &PgPool, config: &MappingConfigDocument) -> TestResult<SyncJob> {
    let job = SyncJob {
        id: Uuid::now_v7(),
        provider_id: config.source_provider_id,
        mapping_version: config.reference.to_string(),
        status: SyncStatus::Running,
        started_at: Some(Utc::now()),
        finished_at: None,
        stats: json!({}),
    };
    sqlx::query("INSERT INTO sync_job(id,provider_id,mapping_version,status,started_at,stats) VALUES ($1,$2,$3,'running',$4,'{}'::jsonb)")
        .bind(job.id).bind(job.provider_id).bind(&job.mapping_version).bind(job.started_at).execute(pool).await?;
    Ok(job)
}

struct FinalTagMutationDenial {
    observations: std::sync::Mutex<Vec<Value>>,
}

impl InventoryCommitAuthorisation for FinalTagMutationDenial {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn ensure_mutation_access(
        &self,
        action: Action,
        _id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        let denied = if action == Action::Update && attributes["object_type"] == "extras.tag" {
            let mut observations = self.observations.lock().expect("tag mutation observations");
            observations.push(attributes);
            observations.len() == 3
        } else {
            false
        };
        Box::pin(async move {
            if denied {
                Err(AppError::Forbidden(
                    "synthetic final tag mutation denial".into(),
                ))
            } else {
                Ok(())
            }
        })
    }
}

async fn assert_invalid_scope_graph(
    pool: &PgPool,
    target: &LocalInventoryCatalog,
    config: &MappingConfigDocument,
    batch: LocalInventoryBatch,
) -> TestResult {
    let job = running_scope_job(pool, config).await?;
    let before = tag_scope_snapshot(pool).await?;
    let plan = target.plan_read_only(&batch).await?;
    assert!(
        plan.findings
            .iter()
            .any(|finding| finding.severity == Severity::Blocker),
        "assessment accepted an incompatible scope graph"
    );
    assert!(
        target.transfer_observation(&batch).await.is_err(),
        "observation accepted an incompatible scope graph"
    );
    assert_eq!(before, tag_scope_snapshot(pool).await?);
    assert!(
        target
            .commit_pull(CommitPull {
                job,
                batch,
                authorisation: authorisation(usize::MAX)
            })
            .await
            .is_err(),
        "commit accepted an incompatible scope graph"
    );
    assert_eq!(before, tag_scope_snapshot(pool).await?);
    Ok(())
}

async fn tag_scope_tokens(pool: &PgPool) -> TestResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT c.app_label || '.' || c.model FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id JOIN inventory_tag t ON t.id=s.tag_id WHERE t.slug='synthetic-tag' ORDER BY 1").fetch_all(pool).await?)
}

fn fixture(provider: Uuid) -> TestResult<(MappingConfigDocument, SourceSnapshot)> {
    let reference = MappingRef::new(Uuid::now_v7(), 1);
    let mut config = MappingConfigDocument {
        absence_policy: Default::default(),
        reference,
        name: "synthetic transfer".into(),
        description: None,
        source_provider_id: provider,
        target: MappingTarget::LocalInventory,
        status: MappingConfigurationStatus::Active,
        direction: MappingDirection::Pull,
        conflict_mode: ConflictMode::NetboxWins,
        created_by: None,
        created_at: Utc::now(),
        retired_at: None,
        entity_mappings: Vec::new(),
        field_mappings: Vec::new(),
    };
    for (entity, fields) in [
        (InventoryEntity::Tag, &["name", "slug"][..]),
        (InventoryEntity::OwnerGroup, &["name", "description"][..]),
        (
            InventoryEntity::Owner,
            &["name", "description", "group"][..],
        ),
        (
            InventoryEntity::Site,
            &["name", "slug", "owner", "tags"][..],
        ),
    ] {
        let id = Uuid::now_v7();
        config.entity_mappings.push(EntityMapping {
            id,
            mapping_config_logical_id: reference.logical_id,
            mapping_config_version_id: reference.version_id,
            source_entity: entity.source_key().into(),
            target_entity: entity.target_key(),
            enabled: true,
            created_at: Utc::now(),
        });
        for field in entity
            .field_iter()
            .filter(|field| fields.contains(&field.name()) || field.absent() == Absent::Reject)
        {
            let mapped = fields.contains(&field.name());
            let expression = if mapped {
                json!({"op": "identity", "args": [{"field": field.name()}]})
            } else {
                json!({"op": "literal", "args": [if field.name() == "status" { "active" } else { "" }]})
            };
            config.field_mappings.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: id,
                source_field: field.name().into(),
                target_field: field.name().into(),
                forward_transform_json: expression,
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: Utc::now(),
            });
        }
    }
    let mut records = Vec::new();
    for (entity, id, value) in [
        (
            InventoryEntity::Tag,
            "tag-1",
            json!({"name":"synthetic-tag","slug":"synthetic-tag"}),
        ),
        (
            InventoryEntity::OwnerGroup,
            "group-1",
            json!({"name":"synthetic-group","description":"known group"}),
        ),
        (
            InventoryEntity::Owner,
            "owner-1",
            json!({"name":"synthetic-owner","description":"known owner","group":{"id":"group-1"}}),
        ),
        (
            InventoryEntity::Site,
            "site-1",
            json!({"name":"synthetic-site","slug":"synthetic-site","owner":{"id":"owner-1"},"tags":[{"id":"tag-1"}]}),
        ),
    ] {
        records.push(SourceRecord::new(
            SourceIdentity::new(provider, SourceEntity::new(entity.source_key())?, id)?,
            value.as_object().ok_or("fixture record")?.clone(),
        ));
    }
    // The embedded site tags and the authoritative membership edge must agree.
    records.push(SourceRecord::new(
        SourceIdentity::new(
            provider,
            SourceEntity::new(InventoryEntity::TaggedObject.source_key())?,
            "site-tag-1",
        )?,
        json!({"object_type":"dcim.site","object_id":"site-1","tag":{"id":"tag-1"}})
            .as_object()
            .ok_or("fixture membership")?
            .clone(),
    ));
    Ok((
        config,
        SourceSnapshot {
            coverage: None,
            records,
        },
    ))
}

fn prepare(
    config: &MappingConfigDocument,
    source: &SourceSnapshot,
) -> TestResult<LocalInventoryBatch> {
    let assessment = assess_preparation(config, source);
    assessment.batch.ok_or_else(|| {
        let findings: Vec<_> = assessment
            .findings
            .iter()
            .map(|finding| (finding.code, finding.field_path.as_deref()))
            .collect();
        format!("synthetic preparation blocked: {findings:?}").into()
    })
}

async fn descriptor(
    target: &LocalInventoryCatalog,
    batch: &LocalInventoryBatch,
    config: &MappingConfigDocument,
) -> TestResult<PinnedTransferDescriptor> {
    let (target, schema_digest) = target.transfer_observation(batch).await?;
    let now = Utc::now();
    let digest = "1".repeat(64);
    let operator_id = Uuid::now_v7();
    Ok(TransferDescriptor {
        format_version: TRANSFER_FORMAT_VERSION,
        run_id: Uuid::now_v7(),
        provider_id: config.source_provider_id,
        bundle_digest: digest.clone(),
        mapping_ref: config.reference,
        mapping_digest: digest.clone(),
        profile_digest: digest.clone(),
        catalog_digest: digest.clone(),
        tool_digest: digest.clone(),
        schema_digest,
        artifact_sha256: digest.clone(),
        executable_sha256: digest.clone(),
        target,
        operator: TransferOperator {
            user_id: operator_id,
        },
        authority: SourceAttestationIdentity {
            operator_id,
            provenance_sha256: digest.clone(),
            authority_id: Uuid::now_v7(),
            attestation_sha256: digest.clone(),
            issuer: "synthetic fixture".into(),
            issued_at: now,
            scope_digest: digest.clone(),
            expectations_sha256: digest,
            edge_coverage: EdgeCoverage::TypedInventoryReferences,
        },
        approval: TransferApproval {
            not_before: now - Duration::minutes(1),
            expires_at: now + Duration::hours(1),
        },
        retain_absence: RetainAbsencePolicy::RetainAndReport,
        scope: TransferScope {
            creates: true,
            updates: true,
            owner_assignments: true,
            custom_fields: true,
            object_references: true,
            secret_intents: false,
        },
    }
    .pin()?)
}

async fn values(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('tags',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM inventory_tag t), 'groups',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM users_ownergroup t), 'owners',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM users_owner t), 'sites',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM dcim_site t), 'bindings',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY source_entity,external_id),'[]') FROM sync_inventory_binding t), 'jobs',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM sync_job t))")
        .fetch_one(pool).await?)
}

async fn inventory_values(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('tags',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM inventory_tag t), 'groups',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM users_ownergroup t), 'owners',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM users_owner t), 'sites',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM dcim_site t))")
        .fetch_one(pool).await?)
}

/// Exercise the actual Device index expressions on the guarded child, with
/// source-qualified foreign keys and independently known names and scopes.
async fn device_natural_keys(
    pool: &PgPool,
    target: &LocalInventoryCatalog,
    base: &MappingConfigDocument,
    site_id: Uuid,
) -> TestResult {
    let manufacturer = Uuid::now_v7();
    let role = Uuid::now_v7();
    let device_type = Uuid::now_v7();
    let other_site = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_manufacturer(id,name,slug,description,comments,custom_field_data) VALUES ($1,'device-manufacturer','device-manufacturer','','','{}')")
        .bind(manufacturer).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_devicerole(id,name,slug,color,vm_role,description,level,lft,rght,tree_id,comments,custom_field_data) VALUES ($1,'device-role','device-role','123456',false,'',0,1,2,1,'','{}')")
        .bind(role).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_devicetype(id,model,slug,part_number,u_height,is_full_depth,front_image,rear_image,comments,manufacturer_id,description,console_port_template_count,console_server_port_template_count,power_port_template_count,power_outlet_template_count,interface_template_count,front_port_template_count,rear_port_template_count,device_bay_template_count,module_bay_template_count,inventory_item_template_count,exclude_from_utilization,device_count,custom_field_data) VALUES ($1,'device-model','device-model','',1,true,'','','',$2,'',0,0,0,0,0,0,0,0,0,0,false,0,'{}')")
        .bind(device_type).bind(manufacturer).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_site(id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,'second-site','second-site','active','','','','','','{}')")
        .bind(other_site).execute(pool).await?;
    for (entity, external, id) in [
        (InventoryEntity::DeviceRole, "role-1", role),
        (InventoryEntity::DeviceType, "type-1", device_type),
        (InventoryEntity::Site, "site-2", other_site),
    ] {
        sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,$2,$3,$4,$5)")
            .bind(base.source_provider_id).bind(entity.source_key()).bind(external).bind(entity.target_key()).bind(id).execute(pool).await?;
    }
    let mut config = base.clone();
    let mapping = Uuid::now_v7();
    config.entity_mappings.push(EntityMapping {
        id: mapping,
        mapping_config_logical_id: config.reference.logical_id,
        mapping_config_version_id: config.reference.version_id,
        source_entity: InventoryEntity::Device.source_key().into(),
        target_entity: InventoryEntity::Device.target_key(),
        enabled: true,
        created_at: Utc::now(),
    });
    for field in [
        "name",
        "site",
        "role",
        "device_type",
        "status",
        "serial",
        "comments",
        "description",
    ] {
        config.field_mappings.push(FieldMapping {
            id: Uuid::now_v7(),
            entity_mapping_id: mapping,
            source_field: field.into(),
            target_field: field.into(),
            forward_transform_json: json!({"op":"identity","args":[{"field":field}]}),
            reverse_transform_json: None,
            round_trip_critical: false,
            created_at: Utc::now(),
        });
    }
    let record = |external: &str, name: &str, site: &str| -> TestResult<SourceRecord> {
        Ok(SourceRecord::new(SourceIdentity::new(base.source_provider_id, SourceEntity::new("dcim.devices")?, external)?,
            json!({"name":name,"site":{"id":site},"role":{"id":"role-1"},"device_type":{"id":"type-1"},"status":"active","serial":"","comments":"","description":""}).as_object().ok_or("device record")?.clone()))
    };
    let initial = SourceSnapshot {
        coverage: None,
        records: vec![record("device-1", "MiXeD-Device", "site-1")?],
    };
    let batch = prepare(&config, &initial)?;
    let empty = target.plan_read_only(&batch).await?;
    assert!(
        empty.analysis_complete
            && !empty.findings.iter().any(|finding| finding.code
                == FindingCode::AnalysisDependencyUnavailable
                || finding.code == FindingCode::TargetDefinitionConflict)
    );
    let pair = SourceSnapshot {
        coverage: None,
        records: vec![
            record("device-1", "MiXeD-Device", "site-1")?,
            record("device-2", "mixed-device", "site-1")?,
        ],
    };
    let pair_plan = target.plan_read_only(&prepare(&config, &pair)?).await?;
    assert!(
        pair_plan
            .findings
            .iter()
            .any(|finding| finding.code == FindingCode::TargetDefinitionConflict)
    );
    let pinned = descriptor(target, &batch, &config).await?;
    target
        .apply_transfer(batch, &pinned, authorisation(usize::MAX))
        .await?;
    let transfer_proofs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sync_inventory_ownership WHERE source_provider_id=$1",
    )
    .bind(base.source_provider_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        transfer_proofs, 0,
        "sealed transfer is not source creation provenance"
    );
    let stored_site: Uuid =
        sqlx::query_scalar("SELECT site_id FROM dcim_device WHERE name='MiXeD-Device'")
            .fetch_one(pool)
            .await?;
    assert_eq!(stored_site, site_id);
    let mut omitted = initial.clone();
    omitted.records[0].fields.remove("name");
    let update_batch = prepare(&config, &omitted)?;
    let update = target.plan_read_only(&update_batch).await?;
    assert!(
        update.analysis_complete
            && !update.findings.iter().any(|finding| finding.code
                == FindingCode::TargetDefinitionConflict
                || finding.code == FindingCode::AnalysisDependencyUnavailable)
    );
    let update_descriptor = descriptor(target, &update_batch, &config).await?;
    target
        .apply_transfer(update_batch, &update_descriptor, authorisation(usize::MAX))
        .await?;
    let preserved: String = sqlx::query_scalar("SELECT name FROM dcim_device WHERE site_id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(preserved, "MiXeD-Device");
    let conflict = SourceSnapshot {
        coverage: None,
        records: vec![record("device-2", "mixed-device", "site-1")?],
    };
    assert!(
        target
            .plan_read_only(&prepare(&config, &conflict)?)
            .await?
            .findings
            .iter()
            .any(|finding| finding.code == FindingCode::TargetDefinitionConflict)
    );
    let distinct_scope = SourceSnapshot {
        coverage: None,
        records: vec![record("device-2", "mixed-device", "site-2")?],
    };
    let scoped = target
        .plan_read_only(&prepare(&config, &distinct_scope)?)
        .await?;
    assert!(
        scoped.analysis_complete
            && !scoped.findings.iter().any(|finding| finding.code
                == FindingCode::TargetDefinitionConflict
                || finding.code == FindingCode::AnalysisDependencyUnavailable)
    );
    Ok(())
}

async fn exercise(pool: &PgPool) -> TestResult {
    let provider = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,'pwa81 synthetic','netbox','{}',false)")
        .bind(provider).execute(pool).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let (config, source) = fixture(provider)?;
    let batch = prepare(&config, &source)?;

    // Drift after pinning is refused without an inventory/job/binding write.
    let stale = descriptor(&target, &batch, &config).await?;
    let extra = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag(id,name,slug) VALUES ($1,'drift','drift')")
        .bind(extra)
        .execute(pool)
        .await?;
    let before = values(pool).await?;
    let stale_result = target
        .apply_transfer(batch.clone(), &stale, authorisation(usize::MAX))
        .await;
    assert!(
        matches!(stale_result, Err(AppError::Conflict(message)) if message == "inventory_transfer:stale_target")
    );
    assert_eq!(before, values(pool).await?);
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(extra)
        .execute(pool)
        .await?;

    // The second ownership check occurs immediately before commit.
    let pinned = descriptor(&target, &batch, &config).await?;
    let before = values(pool).await?;
    let denial = Arc::new(Authorisation {
        calls: AtomicUsize::new(0),
        deny_at: 2,
    });
    let denied = target
        .apply_transfer(batch.clone(), &pinned, denial.clone())
        .await;
    assert!(
        matches!(denied, Err(AppError::Conflict(message)) if message == "synthetic authorisation lost")
    );
    assert_eq!(denial.calls.load(Ordering::SeqCst), 2);
    assert_eq!(before, values(pool).await?);
    assert!(
        target
            .transfer_receipt(pinned.descriptor().run_id, pinned.descriptor_digest())
            .await?
            .is_none()
    );

    let a = target.apply_transfer(batch.clone(), &pinned, authorisation(usize::MAX));
    let b = target.apply_transfer(batch.clone(), &pinned, authorisation(usize::MAX));
    let (a, b) = tokio::join!(a, b);
    assert!(a.is_ok() || b.is_ok());
    for result in [&a, &b] {
        if let Err(error) = result {
            assert!(
                matches!(error, AppError::Conflict(message) if message == "inventory_transfer:busy")
            );
        }
    }
    let receipt = target
        .transfer_receipt(pinned.descriptor().run_id, pinned.descriptor_digest())
        .await?
        .ok_or("missing receipt")?;
    let completed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sync_job WHERE id=$1 AND status='completed'")
            .bind(pinned.descriptor().run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(completed, 1);
    if let Ok(other) = a {
        assert_eq!(other, receipt);
    }
    if let Ok(other) = b {
        assert_eq!(other, receipt);
    }
    let (site_id, site_name, owner_name, group_name): (Uuid, String, String, String) = sqlx::query_as(
        "SELECT s.id,s.name,o.name,g.name FROM dcim_site s JOIN users_owner o ON o.id=s.owner_id JOIN users_ownergroup g ON g.id=o.group_id WHERE s.slug='synthetic-site'"
    ).fetch_one(pool).await?;
    assert_eq!(
        (site_name.as_str(), owner_name.as_str(), group_name.as_str()),
        ("synthetic-site", "synthetic-owner", "synthetic-group")
    );
    let tags: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    let tag_id: Uuid =
        sqlx::query_scalar("SELECT id FROM inventory_tag WHERE slug='synthetic-tag'")
            .fetch_one(pool)
            .await?;
    assert_eq!(tags, vec![tag_id.to_string()]);
    assert_eq!(receipt.sync_job_id, pinned.descriptor().run_id);

    // A distinct run with the same values may refresh bindings, but must not
    // touch any inventory row or fire its update triggers.
    let unchanged = descriptor(&target, &batch, &config).await?;
    let inventory_before = inventory_values(pool).await?;
    let no_op = target
        .apply_transfer(batch.clone(), &unchanged, authorisation(usize::MAX))
        .await?;
    assert_eq!(no_op.stats.unchanged, 4);
    assert_eq!(inventory_before, inventory_values(pool).await?);

    // Replay returns the original receipt and never overwrites a later edit.
    sqlx::query("UPDATE dcim_site SET name='later native edit' WHERE id=$1")
        .bind(site_id)
        .execute(pool)
        .await?;
    let replay = target
        .apply_transfer(batch.clone(), &pinned, authorisation(usize::MAX))
        .await?;
    assert_eq!(replay, receipt);
    let current: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(current, "later native edit");

    // A bound OwnerGroup absent from a later snapshot is reported and retained.
    let mut reduced = source.clone();
    reduced
        .records
        .retain(|record| record.identity.entity.as_str() != "users.owner-groups");
    let group_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users_ownergroup WHERE name='synthetic-group'")
            .fetch_one(pool)
            .await?;
    let reduced_batch = prepare(&config, &reduced)?;
    let next = descriptor(&target, &reduced_batch, &config).await?;
    let retained = target
        .apply_transfer(reduced_batch, &next, authorisation(usize::MAX))
        .await?;
    assert!(
        retained
            .retained_absent
            .iter()
            .any(|entry| entry.source_entity == "users.owner-groups" && entry.local_id == group_id)
    );
    let group_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users_ownergroup WHERE id=$1)")
            .bind(group_id)
            .fetch_one(pool)
            .await?;
    assert!(group_exists);

    // A second provider has different deterministic IDs; names cannot adopt rows.
    let provider2 = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,'pwa81 second','netbox','{}',false)").bind(provider2).execute(pool).await?;
    let (config2, source2) = fixture(provider2)?;
    let batch2 = prepare(&config2, &source2)?;
    let first_owner = batch
        .rows()
        .iter()
        .find(|row| row.entity() == InventoryEntity::Owner)
        .ok_or("owner")?;
    let other_owner = batch2
        .rows()
        .iter()
        .find(|row| row.entity() == InventoryEntity::Owner)
        .ok_or("owner")?;
    assert_ne!(first_owner.local_id(), other_owner.local_id());
    let conflict = target.plan_read_only(&batch2).await?;
    assert!(
        conflict
            .findings
            .iter()
            .any(|finding| finding.code == FindingCode::TargetDefinitionConflict)
    );

    device_natural_keys(pool, &target, &config, site_id).await?;

    // A dangling qualified binding is a blocker, not a recreation request.
    let mut dangling_source = source.clone();
    dangling_source.records[2].identity.external_id = "deleted-owner".into();
    sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,'users.owners','deleted-owner',$2,$3)")
        .bind(provider).bind(InventoryEntity::Owner.target_key()).bind(Uuid::now_v7()).execute(pool).await?;
    let dangling = prepare(&config, &dangling_source)?;
    let assessment = target.plan_read_only(&dangling).await?;
    assert!(
        assessment
            .findings
            .iter()
            .any(|finding| finding.code == FindingCode::TargetBindingsInvalid)
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires the guarded pwa81_transfer_ child database fixture and PostgreSQL"]
async fn canonical_writer_atomicity_cas_replay_and_qualified_graph() -> TestResult {
    let (child, url) = ChildDatabase::create("pwa81_transfer_").await?;
    let pool = match PgPool::connect(&url).await {
        Ok(pool) => pool,
        Err(error) => {
            child.cleanup().await?;
            return Err(error.into());
        }
    };
    let outcome = std::panic::AssertUnwindSafe(async {
        Migrator::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .as_path(),
        )
        .await?
        .run(&pool)
        .await?;
        exercise(&pool).await
    })
    .catch_unwind()
    .await;
    pool.close().await;
    let cleanup = child.cleanup().await;
    match outcome {
        Ok(result) => {
            result?;
            cleanup?;
            Ok(())
        }
        Err(panic) => {
            cleanup?;
            std::panic::resume_unwind(panic)
        }
    }
}

async fn exercise_imported_tag_scope(pool: &PgPool) -> TestResult {
    let provider = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,'scope synthetic','netbox','{}',false)").bind(provider).execute(pool).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let (mut config, mut source) = fixture(provider)?;
    map_tag_scope(&mut config)?;
    let mut tag_only = source.clone();
    tag_only
        .records
        .retain(|row| row.identity.entity.as_str() == InventoryEntity::Tag.source_key());
    set_tag_scope(&mut tag_only, Some(json!(["dcim.device"])));
    let batch = prepare(&config, &tag_only)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    target
        .apply_transfer(batch, &pinned, authorisation(usize::MAX))
        .await?;
    assert_eq!(tag_scope_tokens(pool).await?, vec!["dcim.device"]);

    set_tag_scope(&mut tag_only, Some(json!(["dcim.site"])));
    for denied_scope in [json!(["dcim.device"]), json!(["dcim.site"])] {
        let batch = prepare(&config, &tag_only)?;
        let pinned = descriptor(&target, &batch, &config).await?;
        let before = tag_scope_snapshot(pool).await?;
        let mut policy = CoveragePolicy::allow();
        policy.mutation_denial = Some((
            Action::Update,
            "extras.tag",
            Some(("object_types", denied_scope.clone())),
        ));
        let policy = Arc::new(policy);
        assert!(
            matches!(
                target.apply_transfer(batch, &pinned, policy.clone()).await,
                Err(AppError::Forbidden(_))
            ),
            "missing mutation denial for {denied_scope}"
        );
        assert!(
            policy
                .mutations
                .lock()
                .expect("mutation observations")
                .iter()
                .any(|(_, attributes)| attributes["object_type"] == "extras.tag"
                    && attributes["object_types"] == denied_scope)
        );
        assert_eq!(before, tag_scope_snapshot(pool).await?);
    }

    let batch = prepare(&config, &tag_only)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    let before = tag_scope_snapshot(pool).await?;
    let final_denial = Arc::new(FinalTagMutationDenial {
        observations: std::sync::Mutex::new(Vec::new()),
    });
    assert!(matches!(
        target
            .apply_transfer(batch, &pinned, final_denial.clone())
            .await,
        Err(AppError::Forbidden(_))
    ));
    let observed = final_denial
        .observations
        .lock()
        .expect("tag mutation observations");
    assert_eq!(observed.len(), 3);
    assert_eq!(observed[0]["object_types"], json!(["dcim.device"]));
    assert_eq!(observed[1]["object_types"], json!(["dcim.site"]));
    assert_eq!(observed[2]["object_types"], json!(["dcim.site"]));
    drop(observed);
    assert_eq!(before, tag_scope_snapshot(pool).await?);

    set_tag_scope(&mut tag_only, None);
    let batch = prepare(&config, &tag_only)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    target
        .apply_transfer(batch, &pinned, authorisation(usize::MAX))
        .await?;
    assert_eq!(tag_scope_tokens(pool).await?, vec!["dcim.device"]);

    for invalid in [
        json!(null),
        json!(["dcim.device", "dcim.device"]),
        json!(["dcim.no_such_model"]),
    ] {
        set_tag_scope(&mut tag_only, Some(invalid.clone()));
        let before = tag_scope_snapshot(pool).await?;
        let assessment = assess_preparation(&config, &tag_only);
        assert!(
            assessment.batch.is_none(),
            "preparation accepted invalid scope {invalid}"
        );
        assert_eq!(
            before,
            tag_scope_snapshot(pool).await?,
            "invalid scope {invalid} mutated state"
        );
    }

    // A registered template is not taggable: either admission or the seeded
    // registry must reject it, including on the direct commit path.
    set_tag_scope(&mut tag_only, Some(json!(["dcim.consoleporttemplate"])));
    let before = tag_scope_snapshot(pool).await?;
    if let Some(batch) = assess_preparation(&config, &tag_only).batch {
        if target.transfer_observation(&batch).await.is_ok() {
            let job = running_scope_job(pool, &config).await?;
            let before_commit = tag_scope_snapshot(pool).await?;
            assert!(
                target
                    .commit_pull(CommitPull {
                        job,
                        batch,
                        authorisation: authorisation(usize::MAX)
                    })
                    .await
                    .is_err(),
                "commit accepted a non-taggable scope"
            );
            assert_eq!(before_commit, tag_scope_snapshot(pool).await?);
        } else {
            assert_eq!(before, tag_scope_snapshot(pool).await?);
        }
    } else {
        assert_eq!(before, tag_scope_snapshot(pool).await?);
    }

    // A site attachment is incompatible with the device-only scope.
    set_tag_scope(&mut source, None);
    let batch = prepare(&config, &source)?;
    assert_invalid_scope_graph(pool, &target, &config, batch).await?;

    set_tag_scope(&mut tag_only, Some(json!([])));
    let batch = prepare(&config, &tag_only)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    target
        .apply_transfer(batch, &pinned, authorisation(usize::MAX))
        .await?;
    assert!(tag_scope_tokens(pool).await?.is_empty());
    set_tag_scope(&mut source, Some(json!(["dcim.site"])));
    let batch = prepare(&config, &source)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    let before = tag_scope_snapshot(pool).await?;
    let mut policy = CoveragePolicy::allow();
    policy
        .read_denials
        .push(("extras.tag", Some(("object_types", json!(["dcim.site"])))));
    let policy = Arc::new(policy);
    assert!(matches!(
        target.apply_transfer(batch, &pinned, policy.clone()).await,
        Err(AppError::Forbidden(_))
    ));
    assert!(
        policy
            .reads
            .lock()
            .expect("read observations")
            .iter()
            .any(|attributes| attributes["object_type"] == "extras.tag"
                && attributes["object_types"] == json!(["dcim.site"]))
    );
    assert_eq!(before, tag_scope_snapshot(pool).await?);
    set_tag_scope(&mut source, None);
    let batch = prepare(&config, &source)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    target
        .apply_transfer(batch, &pinned, authorisation(usize::MAX))
        .await?;

    // Original READ of the attached Tag precedes scope assessment for Site.
    set_tag_scope(&mut source, Some(json!(["dcim.device"])));
    let batch = prepare(&config, &source)?;
    let job = running_scope_job(pool, &config).await?;
    let before = tag_scope_snapshot(pool).await?;
    let mut policy = CoveragePolicy::allow();
    policy
        .read_denials
        .push(("extras.tag", Some(("object_types", json!([])))));
    let policy = Arc::new(policy);
    assert!(matches!(
        target
            .commit_pull(CommitPull {
                job,
                batch,
                authorisation: policy.clone()
            })
            .await,
        Err(AppError::Forbidden(_))
    ));
    assert!(
        policy
            .reads
            .lock()
            .expect("read observations")
            .iter()
            .any(|attributes| attributes["object_type"] == "extras.tag"
                && attributes["object_types"] == json!([]))
    );
    assert!(
        !policy
            .mutations
            .lock()
            .expect("mutation observations")
            .iter()
            .any(|(_, attributes)| attributes["object_type"] == "extras.tag")
    );
    assert_eq!(before, tag_scope_snapshot(pool).await?);

    // A rollback-only assessment must validate deferred scope constraints too.
    set_tag_scope(&mut tag_only, Some(json!(["dcim.device"])));
    let batch = prepare(&config, &tag_only)?;
    assert_invalid_scope_graph(pool, &target, &config, batch).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the guarded pwa81_transfer_ child database fixture and PostgreSQL"]
async fn imported_tag_scope_persistence_rejection_and_deferred_rollback() -> TestResult {
    let (child, url) = ChildDatabase::create("pwa81_transfer_").await?;
    let pool = match PgPool::connect(&url).await {
        Ok(pool) => pool,
        Err(error) => {
            child.cleanup().await?;
            return Err(error.into());
        }
    };
    let outcome = std::panic::AssertUnwindSafe(async {
        Migrator::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .as_path(),
        )
        .await?
        .run(&pool)
        .await?;
        exercise_imported_tag_scope(&pool).await
    })
    .catch_unwind()
    .await;
    pool.close().await;
    let cleanup = child.cleanup().await;
    match outcome {
        Ok(result) => {
            result?;
            cleanup?;
            Ok(())
        }
        Err(panic) => {
            cleanup?;
            std::panic::resume_unwind(panic)
        }
    }
}

struct TemplateMutationPolicy {
    denied_action: Option<Action>,
    denied_label: Option<&'static str>,
    device_type: Uuid,
    observed: std::sync::Mutex<Vec<(Action, Value)>>,
}

impl InventoryCommitAuthorisation for TemplateMutationPolicy {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_mutation_access(
        &self,
        action: Action,
        _id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        self.observed
            .lock()
            .expect("policy observation")
            .push((action.clone(), attributes.clone()));
        let denied = self.denied_action.as_ref() == Some(&action)
            && attributes["object_type"] == "dcim.consoleporttemplate"
            && attributes["device_type_id"] == self.device_type.to_string()
            && self
                .denied_label
                .is_none_or(|label| attributes["label"] == label);
        Box::pin(async move {
            if denied {
                Err(AppError::Forbidden(
                    "synthetic conditional mutation denial".into(),
                ))
            } else {
                Ok(())
            }
        })
    }
}

async fn exercise_template_mutation_policy(pool: &PgPool) -> TestResult {
    let provider = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,'mutation synthetic','netbox','{}',false)").bind(provider).execute(pool).await?;
    let manufacturer = Uuid::now_v7();
    let device = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_manufacturer(id,name,slug,description,comments,custom_field_data) VALUES ($1,'mutation-manufacturer','mutation-manufacturer','','','{}')").bind(manufacturer).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_devicetype(id,model,slug,part_number,u_height,is_full_depth,front_image,rear_image,comments,manufacturer_id,description,console_port_template_count,console_server_port_template_count,power_port_template_count,power_outlet_template_count,interface_template_count,front_port_template_count,rear_port_template_count,device_bay_template_count,module_bay_template_count,inventory_item_template_count,exclude_from_utilization,device_count,custom_field_data) VALUES ($1,'mutation-device','mutation-device','',1,true,'','','',$2,'',0,0,0,0,0,0,0,0,0,0,false,0,'{}')").bind(device).bind(manufacturer).execute(pool).await?;
    sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,$2,'1',$3,$4)").bind(provider).bind(InventoryEntity::DeviceType.source_key()).bind(InventoryEntity::DeviceType.target_key()).bind(device).execute(pool).await?;
    let (mut config, _) = fixture(provider)?;
    config.entity_mappings.clear();
    config.field_mappings.clear();
    let entity = InventoryEntity::ConsolePortTemplate;
    let mapping_id = Uuid::now_v7();
    config.entity_mappings.push(EntityMapping {
        id: mapping_id,
        mapping_config_logical_id: config.reference.logical_id,
        mapping_config_version_id: config.reference.version_id,
        source_entity: entity.source_key().into(),
        target_entity: entity.target_key(),
        enabled: true,
        created_at: Utc::now(),
    });
    for field in [
        "name",
        "label",
        "description",
        "type",
        "device_type",
        "module_type",
    ] {
        config.field_mappings.push(FieldMapping {
            id: Uuid::now_v7(),
            entity_mapping_id: mapping_id,
            source_field: field.into(),
            target_field: field.into(),
            forward_transform_json: json!({"op":"identity","args":[{"field":field}]}),
            reverse_transform_json: None,
            round_trip_critical: false,
            created_at: Utc::now(),
        });
    }
    let mut source = SourceSnapshot { coverage: None, records: vec![SourceRecord::new(SourceIdentity::new(provider, SourceEntity::new(entity.source_key())?, "1")?, json!({"name":"console", "label":"old", "description":"", "type":null, "device_type":1, "module_type":null}).as_object().ok_or("template fixture")?.clone())] };
    let target = LocalInventoryCatalog::new(pool.clone());
    let batch = prepare(&config, &source)?;
    let deny_create = Arc::new(TemplateMutationPolicy {
        denied_action: Some(Action::Create),
        denied_label: None,
        device_type: device,
        observed: std::sync::Mutex::new(Vec::new()),
    });
    let pinned = descriptor(&target, &batch, &config).await?;
    assert!(matches!(
        target
            .apply_transfer(batch.clone(), &pinned, deny_create)
            .await,
        Err(AppError::Forbidden(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_consoleporttemplate")
        .fetch_one(pool)
        .await?;
    assert_eq!(count, 0);
    let bindings: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity=$2").bind(provider).bind(entity.source_key()).fetch_one(pool).await?;
    assert_eq!(bindings, 0);
    let allowed = Arc::new(TemplateMutationPolicy {
        denied_action: None,
        denied_label: None,
        device_type: device,
        observed: std::sync::Mutex::new(Vec::new()),
    });
    let pinned = descriptor(&target, &batch, &config).await?;
    target.apply_transfer(batch, &pinned, allowed).await?;
    source.records[0]
        .fields
        .insert("label".into(), json!("ordinary"));
    let ordinary = prepare(&config, &source)?;
    let ordinary_policy = Arc::new(TemplateMutationPolicy {
        denied_action: Some(Action::Update),
        denied_label: Some("ordinary"),
        device_type: device,
        observed: std::sync::Mutex::new(Vec::new()),
    });
    let ordinary_result = target
        .commit_pull(CommitPull {
            job: SyncJob {
                id: Uuid::now_v7(),
                provider_id: provider,
                mapping_version: config.reference.to_string(),
                status: SyncStatus::Running,
                started_at: Some(Utc::now()),
                finished_at: None,
                stats: json!({}),
            },
            batch: ordinary,
            authorisation: ordinary_policy,
        })
        .await;
    assert!(matches!(ordinary_result, Err(AppError::Forbidden(_))));
    let label: String = sqlx::query_scalar("SELECT label FROM dcim_consoleporttemplate")
        .fetch_one(pool)
        .await?;
    assert_eq!(label, "old");
    source.records[0]
        .fields
        .insert("label".into(), json!("new"));
    let update = prepare(&config, &source)?;
    for denied_label in ["old", "new"] {
        let policy = Arc::new(TemplateMutationPolicy {
            denied_action: Some(Action::Update),
            denied_label: Some(denied_label),
            device_type: device,
            observed: std::sync::Mutex::new(Vec::new()),
        });
        let pinned = descriptor(&target, &update, &config).await?;
        assert!(matches!(
            target.apply_transfer(update.clone(), &pinned, policy).await,
            Err(AppError::Forbidden(_))
        ));
        let label: String = sqlx::query_scalar("SELECT label FROM dcim_consoleporttemplate")
            .fetch_one(pool)
            .await?;
        assert_eq!(label, "old");
    }
    let policy = Arc::new(TemplateMutationPolicy {
        denied_action: None,
        denied_label: None,
        device_type: device,
        observed: std::sync::Mutex::new(Vec::new()),
    });
    let pinned = descriptor(&target, &update, &config).await?;
    target
        .apply_transfer(update, &pinned, policy.clone())
        .await?;
    let observed = policy.observed.lock().expect("policy observation");
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].1["label"], "old");
    assert_eq!(observed[1].1["label"], "new");
    assert_eq!(observed[1].1["device_type_id"], device.to_string());
    assert!(
        observed
            .iter()
            .all(|(action, attrs)| *action == Action::Update
                && attrs["object_type"] == "dcim.consoleporttemplate")
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires the guarded pwa81_transfer_ child database fixture and PostgreSQL"]
async fn template_mutation_authority_guards_atomic_transfer() -> TestResult {
    let (child, url) = ChildDatabase::create("pwa81_transfer_").await?;
    let pool = match PgPool::connect(&url).await {
        Ok(pool) => pool,
        Err(error) => {
            child.cleanup().await?;
            return Err(error.into());
        }
    };
    let outcome = std::panic::AssertUnwindSafe(async {
        Migrator::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .as_path(),
        )
        .await?
        .run(&pool)
        .await?;
        exercise_template_mutation_policy(&pool).await
    })
    .catch_unwind()
    .await;
    pool.close().await;
    let cleanup = child.cleanup().await;
    match outcome {
        Ok(result) => {
            result?;
            cleanup?;
            Ok(())
        }
        Err(panic) => {
            cleanup?;
            std::panic::resume_unwind(panic)
        }
    }
}

fn vlan_contact_fixture(provider: Uuid) -> TestResult<(MappingConfigDocument, SourceSnapshot)> {
    let config = identity_mapping(provider);
    let mut records = Vec::new();
    for (entity, id, fields) in [
        (
            InventoryEntity::Tag,
            "7",
            json!({"name": "coverage-tag", "slug": "coverage-tag"}),
        ),
        (
            InventoryEntity::Vlan,
            "7",
            json!({"vid": 123, "name": "coverage-vlan", "status": "active", "description": "vlan description", "comments": "vlan comments", "group": null, "role": null, "qinq_role": null, "qinq_svlan": null, "tags": [{"id":7}]}),
        ),
        (
            InventoryEntity::ContactGroup,
            "7",
            json!({"name": "coverage-group", "slug": "coverage-group", "description": "group description", "comments": "group comments", "parent": null}),
        ),
        (
            InventoryEntity::ContactGroup,
            "8",
            json!({"name": "coverage-child", "slug": "coverage-child", "description": "", "comments": "", "parent": 7}),
        ),
        (
            InventoryEntity::ContactRole,
            "7",
            json!({"name": "coverage-role", "slug": "coverage-role", "description": "role description", "comments": "role comments"}),
        ),
        (
            InventoryEntity::ContactRole,
            "8",
            json!({"name": "coverage-secondary", "slug": "coverage-secondary", "description": ""}),
        ),
        (
            InventoryEntity::Contact,
            "7",
            json!({"name": "coverage-contact", "title": "Operator", "phone": "synthetic-phone", "email": "coverage@example.invalid", "address": "synthetic address", "link": "https://example.invalid/contact", "description": "contact description", "comments": "contact comments", "groups": [7, 8], "tags": [{"id":7}]}),
        ),
        (
            InventoryEntity::Site,
            "7",
            json!({"name": "coverage-site", "slug": "coverage-site", "status": "active", "facility": "", "description": "", "physical_address": "", "shipping_address": "", "comments": "", "latitude": "51.500001"}),
        ),
        (
            InventoryEntity::Prefix,
            "7",
            json!({"prefix": "198.51.100.0/24", "status": "active", "is_pool": false, "mark_utilized": false, "description": "prefix description", "comments": "", "vlan": 7, "role": null}),
        ),
        (
            InventoryEntity::ContactAssignment,
            "7",
            json!({"object": 7, "object_type": "dcim.site", "contact": 7, "role": 7, "priority": "primary"}),
        ),
    ] {
        let mut fields = fields.as_object().ok_or("coverage fields")?.clone();
        if entity.has_custom_field_data() {
            fields.insert("custom_fields".into(), json!({}));
        }
        fields.insert("created".into(), json!("2026-10-01T02:03:04Z"));
        fields.insert("last_updated".into(), json!("2026-10-02T03:04:05Z"));
        records.push(SourceRecord::new(
            SourceIdentity::new(provider, SourceEntity::new(entity.source_key())?, id)?,
            fields,
        ));
    }
    for (id, object_type) in [
        ("vlan-tag", "ipam.vlan"),
        ("contact-tag", "tenancy.contact"),
    ] {
        records.push(SourceRecord::new(
            SourceIdentity::new(
                provider,
                SourceEntity::new(InventoryEntity::TaggedObject.source_key())?,
                id,
            )?,
            json!({"tag":{"id":7},"object_id":7,"object_type":object_type})
                .as_object()
                .ok_or("tag membership")?
                .clone(),
        ));
    }
    Ok((
        config,
        SourceSnapshot {
            coverage: None,
            records,
        },
    ))
}

#[test]
fn vlan_contact_source_preparation_preserves_qualified_graph() -> TestResult {
    let provider = Uuid::now_v7();
    let (config, source) = vlan_contact_fixture(provider)?;
    let batch = prepare(&config, &source)?;
    assert_eq!(batch.rows().len(), 10);
    let ids: std::collections::HashSet<_> = batch.rows().iter().map(|row| row.local_id()).collect();
    assert_eq!(ids.len(), 10, "equal remote IDs remain entity-qualified");
    let (other_config, other_source) = vlan_contact_fixture(Uuid::now_v7())?;
    let other = prepare(&other_config, &other_source)?;
    assert!(
        other
            .rows()
            .iter()
            .all(|row| !ids.contains(&row.local_id()))
    );
    let mut invalid = source.clone();
    invalid
        .records
        .iter_mut()
        .find(|row| row.identity.entity.as_str() == InventoryEntity::ContactAssignment.source_key())
        .ok_or("assignment")?
        .fields
        .insert("object_type".into(), json!("unknown.model"));
    assert!(assess_preparation(&config, &invalid).batch.is_none());
    Ok(())
}

struct CoveragePolicy {
    read_denials: Vec<(&'static str, Option<(&'static str, Value)>)>,
    mutation_denial: Option<(Action, &'static str, Option<(&'static str, Value)>)>,
    reads: std::sync::Mutex<Vec<Value>>,
    mutations: std::sync::Mutex<Vec<(Action, Value)>>,
}

impl CoveragePolicy {
    fn allow() -> Self {
        Self {
            read_denials: Vec::new(),
            mutation_denial: None,
            reads: std::sync::Mutex::new(Vec::new()),
            mutations: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn matches(attributes: &Value, object_type: &str, condition: &Option<(&str, Value)>) -> bool {
        attributes["object_type"] == object_type
            && condition
                .as_ref()
                .is_none_or(|(field, value)| attributes[*field] == *value)
    }
}

impl InventoryCommitAuthorisation for CoveragePolicy {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn ensure_reference_access(
        &self,
        _id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        let denied = self
            .read_denials
            .iter()
            .any(|(kind, condition)| Self::matches(&attributes, kind, condition));
        self.reads
            .lock()
            .expect("read observations")
            .push(attributes);
        Box::pin(async move {
            if denied {
                Err(AppError::Forbidden(
                    "synthetic native reference denial".into(),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn ensure_mutation_access(
        &self,
        action: Action,
        _id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        let denied =
            self.mutation_denial
                .as_ref()
                .is_some_and(|(denied_action, kind, condition)| {
                    *denied_action == action && Self::matches(&attributes, kind, condition)
                });
        self.mutations
            .lock()
            .expect("mutation observations")
            .push((action, attributes));
        Box::pin(async move {
            if denied {
                Err(AppError::Forbidden(
                    "synthetic native mutation denial".into(),
                ))
            } else {
                Ok(())
            }
        })
    }
}

async fn coverage_snapshot(pool: &PgPool) -> TestResult<Value> {
    let memberships: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM tenancy_contact_groups t",
    )
    .fetch_one(pool)
    .await?;
    Ok(json!({"inventory": target_snapshot(pool).await?, "memberships": memberships}))
}

async fn coverage_refusal(
    pool: &PgPool,
    target: &LocalInventoryCatalog,
    config: &MappingConfigDocument,
    source: &SourceSnapshot,
    policy: CoveragePolicy,
) -> TestResult {
    let batch = prepare(config, source)?;
    let pinned = descriptor(target, &batch, config).await?;
    let before = coverage_snapshot(pool).await?;
    assert!(
        matches!(
            target
                .apply_transfer(batch, &pinned, Arc::new(policy))
                .await,
            Err(AppError::Forbidden(_))
        ),
        "native authority must refuse"
    );
    assert_snapshot(
        &before,
        &coverage_snapshot(pool).await?,
        "native denial must roll back rows, relations, bindings and jobs",
    );
    Ok(())
}

async fn exercise_vlan_contact_coverage(pool: &PgPool) -> TestResult {
    let provider = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,'coverage synthetic','netbox','{}',false)").bind(provider).execute(pool).await?;
    let (config, source) = vlan_contact_fixture(provider)?;
    let target = LocalInventoryCatalog::new(pool.clone());
    for token in [
        "ipam.vlan",
        "tenancy.contact",
        "tenancy.contactgroup",
        "tenancy.contactrole",
        "tenancy.contactassignment",
    ] {
        let mut policy = CoveragePolicy::allow();
        policy.mutation_denial = Some((Action::Create, token, None));
        coverage_refusal(pool, &target, &config, &source, policy).await?;
    }
    // The native nested-set result differs from its insert-only zero columns.
    // This denial can only be observed after materialization, before commit.
    let mut final_policy = CoveragePolicy::allow();
    final_policy.mutation_denial = Some((
        Action::Create,
        "tenancy.contactgroup",
        Some(("lft", json!(1))),
    ));
    coverage_refusal(pool, &target, &config, &source, final_policy).await?;
    let batch = prepare(&config, &source)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    let policy = Arc::new(CoveragePolicy::allow());
    target
        .apply_transfer(batch, &pinned, policy.clone())
        .await?;
    for (entity, expected) in [
        (InventoryEntity::Vlan, 1),
        (InventoryEntity::Contact, 1),
        (InventoryEntity::ContactGroup, 2),
        (InventoryEntity::ContactRole, 2),
        (InventoryEntity::ContactAssignment, 1),
    ] {
        assert_eq!(binding_count(pool, provider, entity).await?, expected);
    }
    let native: Value = sqlx::query_scalar("SELECT jsonb_build_object('vid',v.vid,'vlan',p.vlan_id=v.id,'contact_name',c.name,'email',c.email,'priority',a.priority,'target',a.object_id=s.id AND ct.app_label='dcim' AND ct.model='site','contact',a.contact_id=c.id,'role',a.role_id=r.id,'custom_fields',c.custom_field_data,'groups',(SELECT count(*) FROM tenancy_contact_groups WHERE contact_id=c.id),'hierarchy',(SELECT bool_and(child.parent_id=parent.id AND child.level=parent.level+1 AND child.lft>parent.lft AND child.rght<parent.rght AND child.tree_id=parent.tree_id) FROM tenancy_contactgroup child JOIN tenancy_contactgroup parent ON child.parent_id=parent.id)) FROM ipam_vlan v CROSS JOIN ipam_prefix p CROSS JOIN tenancy_contact c CROSS JOIN tenancy_contactassignment a JOIN inventory_content_type ct ON ct.id=a.object_type_id CROSS JOIN dcim_site s JOIN tenancy_contactrole r ON r.slug='coverage-role'")
        .fetch_one(pool).await?;
    assert_eq!(
        native,
        json!({"vid":123,"vlan":true,"contact_name":"coverage-contact","email":"coverage@example.invalid","priority":"primary","target":true,"contact":true,"role":true,"custom_fields":{},"groups":2,"hierarchy":true})
    );
    let timestamps: bool = sqlx::query_scalar("SELECT bool_and(source_created_at='2026-10-01T02:03:04Z'::timestamptz AND source_updated_at='2026-10-02T03:04:05Z'::timestamptz) FROM sync_inventory_binding WHERE source_provider_id=$1").bind(provider).fetch_one(pool).await?;
    assert!(timestamps);
    let latitude: String = sqlx::query_scalar("SELECT latitude::text FROM dcim_site")
        .fetch_one(pool)
        .await?;
    assert_eq!(
        latitude, "51.500001",
        "ORM decimal source reaches native numeric without a float conversion"
    );
    let mut assignment = source
        .records
        .iter()
        .find(|row| row.identity.entity.as_str() == InventoryEntity::ContactAssignment.source_key())
        .ok_or("assignment uniqueness control")?
        .clone();
    assignment.identity.external_id = "different-assignment".into();
    let mut unique_source = SourceSnapshot {
        coverage: None,
        records: vec![assignment],
    };
    let before = coverage_snapshot(pool).await?;
    let duplicate = target
        .plan_read_only(&prepare(&config, &unique_source)?)
        .await?;
    assert!(duplicate.analysis_complete);
    assert!(
        duplicate.findings.iter().any(|finding| {
            finding.code == FindingCode::TargetDefinitionConflict
                && finding.entity.as_deref()
                    == Some(InventoryEntity::ContactAssignment.source_key())
                && finding.field_path.as_deref() == Some("natural_key")
        }),
        "the native object-type/object/contact/role key still rejects a duplicate"
    );
    unique_source.records[0]
        .fields
        .insert("role".into(), json!(8));
    let distinct = target
        .plan_read_only(&prepare(&config, &unique_source)?)
        .await?;
    assert!(distinct.analysis_complete);
    assert!(
        !distinct
            .findings
            .iter()
            .any(|finding| finding.severity == Severity::Blocker)
    );
    assert_snapshot(
        &before,
        &coverage_snapshot(pool).await?,
        "natural-key analysis is read-only",
    );
    // Replayed/unchanged rows still reference persisted targets. Source UPDATE
    // authority must not turn their stored READ denials into an implicit grant.
    for token in [
        "ipam.vlan",
        "tenancy.contact",
        "tenancy.contactgroup",
        "tenancy.contactrole",
        "dcim.site",
        "extras.tag",
    ] {
        let mut policy = CoveragePolicy::allow();
        policy.read_denials.push((token, None));
        coverage_refusal(pool, &target, &config, &source, policy).await?;
    }
    let first_group: Uuid = sqlx::query_scalar(
        "SELECT contactgroup_id FROM tenancy_contact_groups ORDER BY id LIMIT 1",
    )
    .fetch_one(pool)
    .await?;
    let assignment_only = SourceSnapshot {
        coverage: None,
        records: source
            .records
            .iter()
            .filter(|row| {
                row.identity.entity.as_str() == InventoryEntity::ContactAssignment.source_key()
            })
            .cloned()
            .collect(),
    };
    let mut contact_projection_policy = CoveragePolicy::allow();
    contact_projection_policy
        .read_denials
        .push(("tenancy.contact", Some(("group_id", json!(first_group)))));
    coverage_refusal(
        pool,
        &target,
        &config,
        &assignment_only,
        contact_projection_policy,
    )
    .await?;
    for (entity, token, field, old, new) in [
        (
            InventoryEntity::Vlan,
            "ipam.vlan",
            "name",
            "coverage-vlan",
            "renamed-vlan",
        ),
        (
            InventoryEntity::Contact,
            "tenancy.contact",
            "name",
            "coverage-contact",
            "renamed-contact",
        ),
        (
            InventoryEntity::ContactGroup,
            "tenancy.contactgroup",
            "name",
            "coverage-group",
            "renamed-group",
        ),
        (
            InventoryEntity::ContactRole,
            "tenancy.contactrole",
            "name",
            "coverage-role",
            "renamed-role",
        ),
        (
            InventoryEntity::ContactAssignment,
            "tenancy.contactassignment",
            "priority",
            "primary",
            "secondary",
        ),
    ] {
        let mut update = source.clone();
        update
            .records
            .iter_mut()
            .find(|row| row.identity.entity.as_str() == entity.source_key())
            .ok_or("coverage update")?
            .fields
            .insert(field.into(), json!(new));
        for denied in [old, new] {
            let mut policy = CoveragePolicy::allow();
            policy.mutation_denial = Some((Action::Update, token, Some((field, json!(denied)))));
            coverage_refusal(pool, &target, &config, &update, policy).await?;
        }
        if entity == InventoryEntity::Vlan {
            for denied in [old, new] {
                let mut policy = CoveragePolicy::allow();
                policy
                    .read_denials
                    .push((token, Some((field, json!(denied)))));
                coverage_refusal(pool, &target, &config, &update, policy).await?;
            }
            let before = coverage_snapshot(pool).await?;
            let mut policy = CoveragePolicy::allow();
            policy.mutation_denial = Some((Action::Update, token, None));
            let result = target
                .commit_pull(CommitPull {
                    job: SyncJob {
                        id: Uuid::now_v7(),
                        provider_id: provider,
                        mapping_version: config.reference.to_string(),
                        status: SyncStatus::Running,
                        started_at: Some(Utc::now()),
                        finished_at: None,
                        stats: json!({}),
                    },
                    batch: prepare(&config, &update)?,
                    authorisation: Arc::new(policy),
                })
                .await;
            assert!(matches!(result, Err(AppError::Forbidden(_))));
            assert_snapshot(
                &before,
                &coverage_snapshot(pool).await?,
                "ordinary pull obeys native mutation denial",
            );
        }
    }
    let mut omitted = source.clone();
    for row in &mut omitted.records {
        match row.identity.entity.as_str() {
            "ipam.prefixes" => {
                row.fields.remove("vlan");
            }
            "tenancy.contacts" => {
                row.fields.remove("groups");
                row.fields.remove("title");
                row.fields
                    .insert("phone".into(), json!("updated synthetic phone"));
            }
            "tenancy.contact-groups" => {
                row.fields.remove("parent");
            }
            _ => {}
        }
    }
    for token in ["ipam.vlan", "tenancy.contactgroup"] {
        let mut policy = CoveragePolicy::allow();
        policy.read_denials.push((token, None));
        coverage_refusal(pool, &target, &config, &omitted, policy).await?;
    }
    let omitted_batch = prepare(&config, &omitted)?;
    let omitted_pin = descriptor(&target, &omitted_batch, &config).await?;
    target
        .apply_transfer(
            omitted_batch,
            &omitted_pin,
            Arc::new(CoveragePolicy::allow()),
        )
        .await?;
    let retained: bool = sqlx::query_scalar("SELECT title='Operator' AND (SELECT count(*) FROM tenancy_contact_groups)=2 AND (SELECT bool_and(vlan_id IS NOT NULL) FROM ipam_prefix) FROM tenancy_contact").fetch_one(pool).await?;
    assert!(
        retained,
        "omission preserves scalar and relation attributes"
    );
    let mut cleared = source.clone();
    cleared
        .records
        .retain(|row| row.identity.entity.as_str() != InventoryEntity::TaggedObject.source_key());
    for row in &mut cleared.records {
        match row.identity.entity.as_str() {
            "ipam.prefixes" => {
                row.fields.insert("vlan".into(), Value::Null);
            }
            "tenancy.contacts" => {
                row.fields.insert("groups".into(), json!([]));
                row.fields.insert("title".into(), Value::Null);
                row.fields.insert("tags".into(), json!([]));
            }
            "tenancy.contact-groups" => {
                row.fields.insert("parent".into(), Value::Null);
            }
            "ipam.vlans" => {
                row.fields.insert("tags".into(), json!([]));
            }
            _ => {}
        }
    }
    let batch = prepare(&config, &cleared)?;
    let pinned = descriptor(&target, &batch, &config).await?;
    let mut policy = CoveragePolicy::allow();
    policy.read_denials = vec![
        ("ipam.vlan", None),
        ("tenancy.contactgroup", None),
        ("extras.tag", None),
    ];
    target
        .apply_transfer(batch, &pinned, Arc::new(policy))
        .await?;
    let cleared: bool = sqlx::query_scalar("SELECT (SELECT bool_and(vlan_id IS NULL) FROM ipam_prefix) AND NOT EXISTS(SELECT 1 FROM tenancy_contact_groups) AND (SELECT bool_and(parent_id IS NULL AND level=0 AND rght=lft+1) FROM tenancy_contactgroup) AND (SELECT title='' AND cardinality(tags)=0 FROM tenancy_contact) AND (SELECT cardinality(tags)=0 FROM ipam_vlan)").fetch_one(pool).await?;
    assert!(
        cleared,
        "explicit clears remove only the selected final edges"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires the guarded pwa81_transfer_ child database fixture and PostgreSQL"]
async fn vlan_contact_writer_graph_and_authority_are_atomic() -> TestResult {
    let (child, url) = ChildDatabase::create("pwa81_transfer_").await?;
    let pool = match PgPool::connect(&url).await {
        Ok(pool) => pool,
        Err(error) => {
            child.cleanup().await?;
            return Err(error.into());
        }
    };
    let outcome = std::panic::AssertUnwindSafe(async {
        Migrator::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .as_path(),
        )
        .await?
        .run(&pool)
        .await?;
        exercise_vlan_contact_coverage(&pool).await
    })
    .catch_unwind()
    .await;
    pool.close().await;
    let cleanup = child.cleanup().await;
    match outcome {
        Ok(result) => {
            result?;
            cleanup?;
            Ok(())
        }
        Err(panic) => {
            cleanup?;
            std::panic::resume_unwind(panic)
        }
    }
}

// ---------------------------------------------------------------------------
// PWA-81 operator CLI round trip on one populated snapshot
//
// This control drives the shipped `inventory-transfer` executable, not the
// library in process: export, preflight, dry-run, descriptor, apply, resume.
// It supplies no decision of its own — the descriptor digests, the target
// observation, the scope requirements and the CREATE/UPDATE decisions stay with
// the product — and it observes the result through the catalogue, the
// qualified bindings and the durable receipt.
//
// Invocation contract (all absolute, all read from the environment, no
// defaults; nothing here reads or prints a credential):
//   PWA81_TRANSFER_SNAPSHOT_TRANSPORT  private snapshot transport (0600)
//   PWA81_TRANSFER_CLI_PATH            the `inventory-transfer` executable
//   PWA81_TRANSFER_PROOF_DIR           optional private directory for child
//                                      output; a private temporary directory
//                                      is used when absent
// The parent fixture database and Redis URL come from the shared test fixture.
// Child stdout and stderr are kept privately in the proof directory; failures
// report only a step name, an exit status and output digests.
// ---------------------------------------------------------------------------

const PWA81_TRANSFER_PROVIDER_ID: &str = "2027e474-1878-4f0b-9da2-17a97f1c7237";
/// Independently verified SHA256 of the exact transport bytes. The operator
/// endorses this digest and a substituted artifact fails closed.
const PWA81_TRANSFER_TRANSPORT_SHA256: &str =
    "c9dcb374a1b4d31b204e2da5d662436689a7478424fbb6e910810554a5b147fd";
/// The bootstrap administrator owns the endorsed snapshot and the run.
const PWA81_TRANSFER_OPERATOR_ID: &str = "00000000-0000-7000-8000-000000000001";
const PWA81_TRANSFER_PASSWORD_PEPPER: &str = "pwa81-transfer-cli";
/// Fixture-only label written by explicit SQL to make the identity mapping
/// restore the source value on the next run.
const PWA81_TRANSFER_DRIFT_LABEL: &str = "pwa81-transfer-drifted";
/// The two custom fields the producer excluded before serialization. Neither a
/// definition nor a value may appear in the target.
const PWA81_TRANSFER_EXCLUDED_CUSTOM_FIELDS: [&str; 2] = ["api_token", "pwa81_proof_untyped"];

/// Row counts of the nine populated template kinds in the verified transport.
const PWA81_TRANSFER_TEMPLATE_ROWS: [(InventoryEntity, usize); 9] = [
    (InventoryEntity::ConsolePortTemplate, 1),
    (InventoryEntity::ConsoleServerPortTemplate, 1),
    (InventoryEntity::PowerPortTemplate, 1),
    (InventoryEntity::PowerOutletTemplate, 1),
    (InventoryEntity::InterfaceTemplate, 1),
    (InventoryEntity::FrontPortTemplate, 1),
    (InventoryEntity::RearPortTemplate, 1),
    (InventoryEntity::DeviceBayTemplate, 1),
    (InventoryEntity::InventoryItemTemplate, 2),
];

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    openssl::sha::sha256(bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            write!(text, "{byte:02x}").expect("hex formatting into a String");
            text
        })
}

fn private_dir(path: &Path) -> TestResult {
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn private_file(path: &Path) -> TestResult<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

fn write_private(path: &Path, bytes: &[u8]) -> TestResult {
    let mut file = private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// A sanitized view of one finished child invocation. The raw streams stay in
/// the proof directory; only this shape travels through assertions.
struct CliRun {
    exit: i32,
    json: Value,
    stdout_sha256: String,
}

fn closed(
    step: &str,
    exit: i32,
    stdout: &[u8],
    stderr: &[u8],
) -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "cli {step} did not produce a decodable report: exit {exit} stdout_sha256 {} stderr_sha256 {}",
        sha256_hex(stdout),
        sha256_hex(stderr)
    )
    .into()
}

fn expect_exit(run: &CliRun, step: &str, expected: i32) -> TestResult {
    if run.exit == expected {
        return Ok(());
    }
    // The product's routing state and code are static strings; the child's
    // streams stay private and are represented by their digest only.
    let state = run
        .json
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let code = run
        .json
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("none");
    Err(format!(
        "cli {step}: exit {} where {expected} was required (state `{state}` code `{code}` stdout_sha256 {})",
        run.exit, run.stdout_sha256
    )
    .into())
}

fn decode<T: serde::de::DeserializeOwned>(
    run: &CliRun,
    step: &str,
    pointer: &str,
) -> TestResult<T> {
    let value = run
        .json
        .pointer(pointer)
        .ok_or_else(|| format!("cli {step}: exit {} has no `{pointer}`", run.exit))?;
    Ok(serde_json::from_value(value.clone())
        .map_err(|_| format!("cli {step}: `{pointer}` does not decode"))?)
}

fn text<'a>(run: &'a CliRun, step: &str, pointer: &str) -> TestResult<&'a str> {
    match run.json.pointer(pointer).and_then(Value::as_str) {
        Some(value) => Ok(value),
        None => Err(format!("cli {step}: the report has no `{pointer}`").into()),
    }
}

/// The executable under test plus the private paths of one control run.
/// `run` never prints its arguments or environment, so a credential-bearing
/// child invocation cannot leak through a failed assertion.
struct OperatorCli {
    program: PathBuf,
    proof: PathBuf,
    steps: Cell<usize>,
    bundle: PathBuf,
    bundle_digest: String,
    mapping: PathBuf,
    profile: PathBuf,
    profile_digest: String,
    approval: PathBuf,
    scope: PathBuf,
    database: String,
    redis: String,
    token: String,
    operator: String,
}

impl OperatorCli {
    fn new(
        program: &str,
        proof: &Path,
        database: &str,
        redis: &str,
        operator: Uuid,
        token: String,
    ) -> Self {
        Self {
            program: PathBuf::from(program),
            proof: proof.to_path_buf(),
            steps: Cell::new(0),
            bundle: proof.join("bundle"),
            bundle_digest: String::new(),
            mapping: proof.join("mapping.private.json"),
            profile: proof.join("profile.private.json"),
            profile_digest: String::new(),
            approval: proof.join("approval.private.json"),
            scope: proof.join("scope.private.json"),
            database: database.to_string(),
            redis: redis.to_string(),
            token,
            operator: operator.to_string(),
        }
    }

    fn path(&self, path: &Path, what: &str) -> TestResult<String> {
        match path.to_str() {
            Some(value) => Ok(value.to_string()),
            None => Err(format!("the {what} path is not UTF-8").into()),
        }
    }

    fn run(&self, step: &str, args: &[&str], env: &[(&str, &str)]) -> TestResult<CliRun> {
        let index = self.steps.get();
        self.steps.set(index + 1);
        let stdout_path = self
            .proof
            .join(format!("{index:02}-{step}.stdout.private.json"));
        let stderr_path = self
            .proof
            .join(format!("{index:02}-{step}.stderr.private.log"));
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .args(args)
            .stdout(private_file(&stdout_path)?)
            .stderr(private_file(&stderr_path)?);
        for (name, value) in env {
            command.env(name, value);
        }
        let status = command.status()?;
        let stdout = std::fs::read(&stdout_path)?;
        let stderr = std::fs::read(&stderr_path)?;
        let exit = status
            .code()
            .ok_or_else(|| format!("cli {step}: terminated without an exit status"))?;
        let json = if stdout.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&stdout).map_err(|_| closed(step, exit, &stdout, &stderr))?
        };
        Ok(CliRun {
            exit,
            json,
            stdout_sha256: sha256_hex(&stdout),
        })
    }

    fn assessment(&self, step: &str, env: &[(&str, &str)]) -> TestResult<CliRun> {
        let bundle = self.path(&self.bundle, "bundle")?;
        let mapping = self.path(&self.mapping, "mapping")?;
        let profile = self.path(&self.profile, "profile")?;
        self.run(
            step,
            &[
                step,
                "--bundle",
                bundle.as_str(),
                "--bundle-digest",
                self.bundle_digest.as_str(),
                "--provider-id",
                PWA81_TRANSFER_PROVIDER_ID,
                "--mapping",
                mapping.as_str(),
                "--profile",
                profile.as_str(),
                "--profile-digest",
                self.profile_digest.as_str(),
            ],
            env,
        )
    }

    fn prepare(&self, step: &str, run_id: Uuid) -> TestResult<PinnedDescriptor> {
        let output = self.proof.join(format!("{step}.descriptor.private.json"));
        let bundle = self.path(&self.bundle, "bundle")?;
        let mapping = self.path(&self.mapping, "mapping")?;
        let profile = self.path(&self.profile, "profile")?;
        let approval = self.path(&self.approval, "approval")?;
        let scope = self.path(&self.scope, "scope")?;
        let destination = self.path(&output, "descriptor")?;
        let run_id_text = run_id.to_string();
        let run = self.run(
            step,
            &[
                "descriptor",
                "--bundle",
                bundle.as_str(),
                "--bundle-digest",
                self.bundle_digest.as_str(),
                "--provider-id",
                PWA81_TRANSFER_PROVIDER_ID,
                "--mapping",
                mapping.as_str(),
                "--profile",
                profile.as_str(),
                "--profile-digest",
                self.profile_digest.as_str(),
                "--run-id",
                run_id_text.as_str(),
                "--operator-id",
                self.operator.as_str(),
                "--approval",
                approval.as_str(),
                "--scope",
                scope.as_str(),
                "--endorse-provenance-digest",
                PWA81_TRANSFER_TRANSPORT_SHA256,
                "--output",
                destination.as_str(),
            ],
            &[
                ("INVENTORY_TRANSFER_DATABASE_URL", self.database.as_str()),
                ("INVENTORY_TRANSFER_OPERATOR_TOKEN", self.token.as_str()),
            ],
        )?;
        expect_exit(&run, step, 0)?;
        if text(&run, step, "/state")? != "prepared" {
            return Err(format!("cli {step}: the descriptor was not prepared").into());
        }
        Ok(PinnedDescriptor {
            path: output,
            digest: text(&run, step, "/descriptor_digest")?.to_string(),
            run_id,
        })
    }

    fn transfer(
        &self,
        step: &str,
        command: &str,
        descriptor: &PinnedDescriptor,
    ) -> TestResult<CliRun> {
        let bundle = self.path(&self.bundle, "bundle")?;
        let mapping = self.path(&self.mapping, "mapping")?;
        let profile = self.path(&self.profile, "profile")?;
        let pinned = self.path(&descriptor.path, "descriptor")?;
        self.run(
            step,
            &[
                command,
                "--descriptor",
                pinned.as_str(),
                "--descriptor-digest",
                descriptor.digest.as_str(),
                "--bundle",
                bundle.as_str(),
                "--mapping",
                mapping.as_str(),
                "--profile",
                profile.as_str(),
                "--endorse-provenance-digest",
                PWA81_TRANSFER_TRANSPORT_SHA256,
            ],
            &[
                ("INVENTORY_TRANSFER_DATABASE_URL", self.database.as_str()),
                ("INVENTORY_TRANSFER_REDIS_URL", self.redis.as_str()),
                ("INVENTORY_TRANSFER_OPERATOR_TOKEN", self.token.as_str()),
            ],
        )
    }
}

struct PinnedDescriptor {
    path: PathBuf,
    digest: String,
    run_id: Uuid,
}

// Owns only the process spawned by this control, including on assertion failure.
struct ApplyChild(std::process::Child);

impl Drop for ApplyChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

async fn interrupted_apply(
    pool: &PgPool,
    cli: &OperatorCli,
    descriptor: &PinnedDescriptor,
) -> TestResult {
    // The job completion update follows inventory writes but precedes the
    // receipt and commit. The trigger exists only in this synthetic child DB.
    sqlx::query("CREATE FUNCTION pwa81_transfer_pause() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(810081); RETURN NEW; END $$")
        .execute(pool).await?;
    sqlx::query("CREATE TRIGGER pwa81_transfer_pause BEFORE UPDATE ON sync_job FOR EACH ROW EXECUTE FUNCTION pwa81_transfer_pause()")
        .execute(pool).await?;
    let mut barrier = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(810081)")
        .execute(&mut *barrier)
        .await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    let before = target_snapshot(pool).await?;
    let mut child = ApplyChild(
        Command::new(&cli.program)
            .env_clear()
            .args([
                "apply",
                "--descriptor",
                cli.path(&descriptor.path, "descriptor")?.as_str(),
                "--descriptor-digest",
                descriptor.digest.as_str(),
                "--bundle",
                cli.path(&cli.bundle, "bundle")?.as_str(),
                "--mapping",
                cli.path(&cli.mapping, "mapping")?.as_str(),
                "--profile",
                cli.path(&cli.profile, "profile")?.as_str(),
                "--endorse-provenance-digest",
                PWA81_TRANSFER_TRANSPORT_SHA256,
            ])
            .env("INVENTORY_TRANSFER_DATABASE_URL", &cli.database)
            .env("INVENTORY_TRANSFER_REDIS_URL", &cli.redis)
            .env("INVENTORY_TRANSFER_OPERATOR_TOKEN", &cli.token)
            .stdout(private_file(
                &cli.proof.join("interrupted.stdout.private.json"),
            )?)
            .stderr(private_file(
                &cli.proof.join("interrupted.stderr.private.log"),
            )?)
            .spawn()?,
    );
    let writer: i32 = tokio::time::timeout(StdDuration::from_secs(120), async {
        loop {
            if child.0.try_wait()?.is_some() {
                return Err::<i32, Box<dyn std::error::Error + Send + Sync>>("apply exited before the writer barrier".into());
            }
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname = current_database() AND wait_event = 'advisory' AND $1 = ANY(pg_blocking_pids(pid))")
                .bind(blocker).fetch_optional(pool).await?;
            if let Some(pid) = pid { return Ok(pid); }
            tokio::time::sleep(StdDuration::from_millis(50)).await;
        }
    }).await??;
    let registered: (String, Vec<u8>, Option<Value>) = sqlx::query_as("SELECT descriptor_digest, descriptor, receipt FROM sync_inventory_transfer_run WHERE run_id = $1")
        .bind(descriptor.run_id).fetch_one(pool).await?;
    assert_eq!(registered.0, descriptor.digest);
    assert_eq!(registered.1, std::fs::read(&descriptor.path)?);
    assert!(
        registered.2.is_none(),
        "receipt is not committed at the writer barrier"
    );
    child.0.kill()?;
    let status = child.0.wait()?;
    assert!(!status.success(), "the actual apply process was terminated");
    // A backend waiting on a lock may not notice the closed socket until the
    // wait ends. Release only after reaping our child, then observe rollback.
    sqlx::query("SELECT pg_advisory_unlock(810081)")
        .execute(&mut *barrier)
        .await?;
    tokio::time::timeout(StdDuration::from_secs(30), async {
        loop {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND datname = current_database())")
                .bind(writer).fetch_one(pool).await?;
            if !exists { return Ok::<(), sqlx::Error>(()); }
            tokio::time::sleep(StdDuration::from_millis(50)).await;
        }
    }).await??;
    assert_snapshot(
        &before,
        &target_snapshot(pool).await?,
        "process loss rolls back inventory and bindings",
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_job WHERE id = $1")
        .bind(descriptor.run_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(jobs, 0, "the uncommitted job rolled back");
    let after: (String, Vec<u8>, Option<Value>) = sqlx::query_as("SELECT descriptor_digest, descriptor, receipt FROM sync_inventory_transfer_run WHERE run_id = $1")
        .bind(descriptor.run_id).fetch_one(pool).await?;
    assert_eq!(
        after, registered,
        "durable run identity survives without a receipt"
    );
    sqlx::query("DROP TRIGGER pwa81_transfer_pause ON sync_job")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION pwa81_transfer_pause()")
        .execute(pool)
        .await?;
    // SIGKILL cannot run the owner-checked lease release. Observe expiry; do
    // not delete a potentially foreign Redis lock. The product TTL is 300 s.
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(cli.redis.as_str())?).await?;
    let lease_key = format!("sync:provider:{PWA81_TRANSFER_PROVIDER_ID}");
    tokio::time::timeout(StdDuration::from_secs(330), async {
        loop {
            let exists: bool = redis::cmd("EXISTS")
                .arg(&lease_key)
                .query_async(&mut redis)
                .await?;
            if !exists {
                return Ok::<(), redis::RedisError>(());
            }
            tokio::time::sleep(StdDuration::from_secs(1)).await;
        }
    })
    .await??;
    Ok(())
}

fn content_type_token(entity: InventoryEntity) -> TestResult<String> {
    let (app_label, model) = entity
        .content_type()
        .ok_or("a row-producing catalogue entity")?;
    Ok(format!("{app_label}.{model}"))
}

async fn content_type_id(pool: &PgPool, entity: InventoryEntity) -> TestResult<Uuid> {
    let (app_label, model) = entity
        .content_type()
        .ok_or("a row-producing catalogue entity")?;
    Ok(sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2",
    )
    .bind(app_label)
    .bind(model)
    .fetch_one(pool)
    .await?)
}

/// One conditional deny for the console-port-template class. `object_type` is
/// the discriminator the policy write path requires, so the rule can never fire
/// on an inventory row whose enforcement site supplies no attributes.
fn console_template_deny_constraint(device_type: Option<Uuid>) -> TestResult<Value> {
    let mut attributes = serde_json::Map::new();
    attributes.insert(
        ATTR_OBJECT_TYPE.to_string(),
        Value::String(content_type_token(InventoryEntity::ConsolePortTemplate)?),
    );
    if let Some(device_type) = device_type {
        attributes.insert(
            ATTR_DEVICE_TYPE_ID.to_string(),
            Value::String(device_type.to_string()),
        );
    }
    Ok(json!({ "attributes": attributes }))
}

async fn set_deny_policy(
    pool: &PgPool,
    name: &str,
    action: Action,
    operator: Uuid,
    constraints: &Value,
) -> TestResult {
    sqlx::query(
        "INSERT INTO rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled) \
         VALUES ($1, $2, 'deny', $3, $4, $5, $6, '[]'::jsonb, true)",
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(serde_json::to_value(vec![action])?)
    .bind(serde_json::to_value(vec![ResourceType::Inventory])?)
    .bind(constraints.clone())
    .bind(json!([format!("user:{operator}")]))
    .execute(pool)
    .await?;
    Ok(())
}

async fn clear_policy(pool: &PgPool, name: &str) -> TestResult {
    let removed = sqlx::query("DELETE FROM rbac_policy WHERE name = $1")
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected();
    if removed != 1 {
        return Err(format!("policy `{name}` was not exactly one row").into());
    }
    Ok(())
}

/// Issue the operator's personal token through the canonical service. The raw
/// value stays in memory and reaches the child only through its environment.
async fn issue_operator_token(pool: &PgPool, operator: Uuid) -> TestResult<String> {
    // This child database has no inherited seats. Use the same synthetic seat
    // fixture as the license tests; the real token service still checks it.
    sqlx::query("INSERT INTO license_seat_assignment (user_id) VALUES ($1)")
        .bind(operator)
        .execute(pool)
        .await?;
    let service = UserApiTokenService::new(
        Arc::new(SqlxUserApiTokenRepository::new(pool.clone())),
        Arc::new(SqlxUserRepository::new(pool.clone())),
        ApiTokenConfig::default(),
        None,
    );
    let scopes = user_api_token_scope_catalog_pairs()
        .iter()
        .filter(|(action, resource)| {
            *resource == ResourceType::Inventory
                && [Action::Create, Action::Read, Action::Update].contains(action)
        })
        .map(|(action, resource)| UserApiTokenScope {
            action: action.clone(),
            resource_type: resource.clone(),
            resource_pattern: "*".to_string(),
        })
        .collect();
    let issued = service
        .create_for_user(
            operator,
            CreateUserApiTokenInput {
                name: "pwa81 transfer operator".to_string(),
                scopes,
                expires_in_seconds: Some(3_600),
            },
            Utc::now(),
        )
        .await?;
    Ok(issued.raw_token)
}

/// One enabled entity mapping per row-producing catalogue entity, with an
/// identity transform per approved field. The catalogue owns which fields exist
/// and how an absent value is encoded; this builder guesses neither.
fn identity_mapping(provider: Uuid) -> MappingConfigDocument {
    let reference = MappingRef::new(Uuid::now_v7(), 1);
    let mut config = MappingConfigDocument {
        absence_policy: Default::default(),
        reference,
        name: "pwa81 populated identity".into(),
        description: None,
        source_provider_id: provider,
        target: MappingTarget::LocalInventory,
        status: MappingConfigurationStatus::Active,
        direction: MappingDirection::Pull,
        conflict_mode: ConflictMode::NetboxWins,
        created_by: None,
        created_at: Utc::now(),
        retired_at: None,
        entity_mappings: Vec::new(),
        field_mappings: Vec::new(),
    };
    for entity in InventoryEntity::all() {
        let mapping_id = Uuid::now_v7();
        config.entity_mappings.push(EntityMapping {
            id: mapping_id,
            mapping_config_logical_id: reference.logical_id,
            mapping_config_version_id: reference.version_id,
            source_entity: entity.source_key().into(),
            target_entity: entity.target_key(),
            enabled: true,
            created_at: Utc::now(),
        });
        for field in entity.field_iter() {
            config.field_mappings.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: mapping_id,
                source_field: field.name().into(),
                target_field: field.name().into(),
                forward_transform_json: json!({"op": "identity", "args": [{"field": field.name()}]}),
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: Utc::now(),
            });
        }
    }
    config
}

async fn binding_count(pool: &PgPool, provider: Uuid, entity: InventoryEntity) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM sync_inventory_binding WHERE source_provider_id = $1 AND source_entity = $2 AND target_entity = $3",
    )
    .bind(provider)
    .bind(entity.source_key())
    .bind(entity.target_key())
    .fetch_one(pool)
    .await?)
}

async fn binding_ids(
    pool: &PgPool,
    provider: Uuid,
    entity: InventoryEntity,
) -> TestResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT local_id FROM sync_inventory_binding WHERE source_provider_id = $1 AND source_entity = $2 AND target_entity = $3 ORDER BY local_id",
    )
    .bind(provider)
    .bind(entity.source_key())
    .bind(entity.target_key())
    .fetch_all(pool)
    .await?)
}

/// Every inventory table the closed catalogue owns, plus the qualified bindings
/// and the terminal jobs. Deliberately excludes the transfer bookkeeping tables,
/// which are not inventory: the assertions that use this compare inventory and
/// bindings only.
async fn target_snapshot(pool: &PgPool) -> TestResult<Value> {
    let mut query =
        sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT jsonb_object_agg(name, rows) FROM (");
    for table in InventoryEntity::all().filter_map(InventoryEntity::table) {
        query.push("SELECT '").push(table).push("' AS name, (SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id), '[]'::jsonb) FROM ")
            .push(table).push(" t) AS rows UNION ALL ");
    }
    query.push("SELECT 'sync_inventory_binding' AS name, (SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.source_entity, t.external_id), '[]'::jsonb) FROM sync_inventory_binding t) AS rows UNION ALL SELECT 'sync_job' AS name, (SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.id), '[]'::jsonb) FROM sync_job t) AS rows) snapshot");
    Ok(query.build_query_scalar().fetch_one(pool).await?)
}

/// Inventory equality without echoing row values: a failure reports the digest
/// of each snapshot, so a refusal test never prints fixture payload.
fn assert_snapshot(before: &Value, after: &Value, message: &str) {
    assert!(
        before == after,
        "{message} (before_sha256 {} after_sha256 {})",
        sha256_hex(before.to_string().as_bytes()),
        sha256_hex(after.to_string().as_bytes())
    );
}

fn assert_offline_assessment(report: &PreflightReport) {
    assert!(matches!(report.outcome, AssessmentOutcome::Completed));
    assert!(
        report.source_coverage_complete,
        "preflight: source coverage"
    );
    assert!(
        report.offline_analysis_complete,
        "preflight: offline analysis"
    );
    assert_eq!(report.observed_rows, 100, "preflight: every captured row");
    assert!(
        report.target.is_none(),
        "preflight must not connect to the target"
    );
    assert!(
        report.plan_digest.is_none(),
        "preflight cannot pin a target plan"
    );
    assert!(matches!(
        report.import_capability,
        ImportCapability::Unavailable
    ));
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|finding| finding.severity == Severity::Blocker)
            .map(|finding| finding.code)
            .collect::<Vec<_>>(),
        vec![FindingCode::TargetGraphNotAssessed],
        "offline preflight must retain exactly the unassessed-target blocker"
    );
}

fn assert_assessment(report: &PreflightReport, step: &str) {
    assert!(
        matches!(report.outcome, AssessmentOutcome::Completed),
        "{step}: outcome"
    );
    assert!(report.offline_analysis_complete, "{step}: offline analysis");
    if let Some(blocker) = report
        .findings
        .iter()
        .find(|finding| finding.severity == Severity::Blocker)
    {
        panic!("{step}: offline blocker {:?}", blocker.code);
    }
    if let Some(target) = &report.target {
        assert!(target.analysis_complete, "{step}: target analysis");
        if let Some(blocker) = target
            .findings
            .iter()
            .find(|finding| finding.severity == Severity::Blocker)
        {
            panic!("{step}: target blocker {:?}", blocker.code);
        }
    }
}

/// The populated values, typed edges and qualified bindings the verified
/// transport carries, read back through the catalogue's own tables.
async fn assert_populated_target(pool: &PgPool, provider: Uuid) -> TestResult {
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sync_inventory_binding WHERE source_provider_id = $1",
    )
    .bind(provider)
    .fetch_one(pool)
    .await?;
    let mut owned = 0i64;
    for entity in InventoryEntity::all() {
        let bound = binding_count(pool, provider, entity).await?;
        if bound == 0 {
            continue;
        }
        let table = entity.table().ok_or("a row-producing catalogue entity")?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT count(*) FROM sync_inventory_binding b WHERE b.source_provider_id = $1 AND b.source_entity = $2 AND NOT EXISTS (SELECT 1 FROM ",
        );
        query.push(table).push(" t WHERE t.id = b.local_id)");
        let dangling: i64 = query
            .build_query_scalar()
            .bind(provider)
            .bind(entity.source_key())
            .fetch_one(pool)
            .await?;
        assert_eq!(
            dangling,
            0,
            "{} binds a row its own table does not hold",
            entity.source_key()
        );
        owned += bound;
    }
    assert_eq!(
        total, owned,
        "every binding names a catalogue entity and that entity's canonical target key"
    );

    // Nine populated template kinds, with the verified row count of each.
    for (entity, expected) in PWA81_TRANSFER_TEMPLATE_ROWS {
        assert_eq!(
            binding_count(pool, provider, entity).await?,
            i64::try_from(expected)?,
            "{} rows",
            entity.source_key()
        );
    }

    // Positioned port templates keep their native position count.
    for entity in [
        InventoryEntity::FrontPortTemplate,
        InventoryEntity::RearPortTemplate,
    ] {
        let table = entity.table().ok_or("a template table")?;
        let positions_column = entity.field("positions").ok_or("positions field")?.column();
        let ids = binding_ids(pool, provider, entity).await?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
        query
            .push(positions_column)
            .push(" FROM ")
            .push(table)
            .push(" WHERE id = $1");
        let positions: i16 = query
            .build_query_scalar()
            .bind(ids[0])
            .fetch_one(pool)
            .await?;
        assert_eq!(positions, 2, "{} positions", entity.source_key());
    }

    // Both port-template mappings reach a bound front and rear template.
    let mapping_entity = InventoryEntity::PortTemplateMapping;
    let mapping_table = mapping_entity.table().ok_or("a mapping table")?;
    let front_column = mapping_entity
        .field("front_port")
        .ok_or("front port")?
        .column();
    let rear_column = mapping_entity
        .field("rear_port")
        .ok_or("rear port")?
        .column();
    let front_position = mapping_entity
        .field("front_port_position")
        .ok_or("front position")?
        .column();
    let rear_position = mapping_entity
        .field("rear_port_position")
        .ok_or("rear position")?
        .column();
    let front_templates = binding_ids(pool, provider, InventoryEntity::FrontPortTemplate).await?;
    let rear_templates = binding_ids(pool, provider, InventoryEntity::RearPortTemplate).await?;
    let mapping_rows = binding_ids(pool, provider, mapping_entity).await?;
    assert_eq!(mapping_rows.len(), 2, "port template mappings");
    let mut positions = Vec::new();
    for id in &mapping_rows {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
        query
            .push(front_column)
            .push(", ")
            .push(front_position)
            .push(", ")
            .push(rear_column)
            .push(", ")
            .push(rear_position)
            .push(" FROM ")
            .push(mapping_table)
            .push(" WHERE id = $1");
        let (front, front_at, rear, rear_at): (Uuid, i16, Uuid, i16) =
            query.build_query_as().bind(*id).fetch_one(pool).await?;
        assert!(
            front_templates.contains(&front),
            "a mapping names a bound front template"
        );
        assert!(
            rear_templates.contains(&rear),
            "a mapping names a bound rear template"
        );
        positions.push((front_at, rear_at));
    }
    positions.sort_unstable();
    assert_eq!(positions, vec![(1, 2), (2, 1)], "mapped template positions");

    // Inventory-item-template generic reference: root and child, typed edges.
    let template_item = InventoryEntity::InventoryItemTemplate;
    let template_item_table = template_item.table().ok_or("a template table")?;
    let template_parent = template_item.field("parent").ok_or("parent")?.column();
    let template_component = template_item.field("component").ok_or("component")?;
    let FieldKind::GenericReference {
        type_column: template_type_column,
        allowed,
        ..
    } = template_component.kind()
    else {
        return Err("inventory-item-template component is not a generic reference".into());
    };
    assert!(
        allowed.contains(&InventoryEntity::PowerPortTemplate)
            && allowed.contains(&InventoryEntity::InterfaceTemplate),
        "the catalogue permits the verified component targets"
    );
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id, ");
    query
        .push(template_parent)
        .push(", ")
        .push(template_component.column())
        .push(", ")
        .push(template_type_column)
        .push(" FROM ")
        .push(template_item_table);
    let template_rows: Vec<(Uuid, Option<Uuid>, Option<Uuid>, Option<Uuid>)> =
        query.build_query_as().fetch_all(pool).await?;
    let root = template_rows
        .iter()
        .find(|(_, parent, ..)| parent.is_none())
        .ok_or("an inventory-item-template root")?;
    let child = template_rows
        .iter()
        .find(|(_, parent, ..)| parent.is_some())
        .ok_or("an inventory-item-template child")?;
    assert_eq!(child.1, Some(root.0), "the child template names its root");
    assert_eq!(
        root.3,
        Some(content_type_id(pool, InventoryEntity::PowerPortTemplate).await?),
        "the root component type"
    );
    assert_eq!(
        child.3,
        Some(content_type_id(pool, InventoryEntity::InterfaceTemplate).await?),
        "the child component type"
    );
    assert!(
        binding_ids(pool, provider, InventoryEntity::PowerPortTemplate)
            .await?
            .contains(&root.2.ok_or("root component")?),
        "the root component is a bound power-port template"
    );
    assert!(
        binding_ids(pool, provider, InventoryEntity::InterfaceTemplate)
            .await?
            .contains(&child.2.ok_or("child component")?),
        "the child component is a bound interface template"
    );

    // Runtime inventory item: its generic reference stays qualified.
    let runtime_item = InventoryEntity::InventoryItem;
    let runtime_table = runtime_item.table().ok_or("an inventory table")?;
    let runtime_component = runtime_item.field("component").ok_or("component")?;
    let FieldKind::GenericReference {
        type_column: runtime_type_column,
        ..
    } = runtime_component.kind()
    else {
        return Err("inventory-item component is not a generic reference".into());
    };
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    query
        .push(runtime_component.column())
        .push(", ")
        .push(runtime_type_column)
        .push(" FROM ")
        .push(runtime_table);
    let (runtime_component_id, runtime_type_id): (Option<Uuid>, Option<Uuid>) =
        query.build_query_as().fetch_one(pool).await?;
    assert_eq!(
        runtime_type_id,
        Some(content_type_id(pool, InventoryEntity::Interface).await?),
        "the runtime component type"
    );
    assert!(
        binding_ids(pool, provider, InventoryEntity::Interface)
            .await?
            .contains(&runtime_component_id.ok_or("runtime component")?),
        "the runtime component is a bound interface"
    );

    // Tenant-group hierarchy: one bound group is the parent of the other.
    let groups = binding_ids(pool, provider, InventoryEntity::TenantGroup).await?;
    assert_eq!(groups.len(), 3, "tenant groups");
    let group_table = InventoryEntity::TenantGroup
        .table()
        .ok_or("a group table")?;
    let group_parent = InventoryEntity::TenantGroup
        .field("parent")
        .ok_or("group parent")?
        .column();
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id FROM ");
    query
        .push(group_table)
        .push(" WHERE ")
        .push(group_parent)
        .push(" IS NOT NULL");
    let child_group: Uuid = query.build_query_scalar().fetch_one(pool).await?;
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    query
        .push(group_parent)
        .push(" FROM ")
        .push(group_table)
        .push(" WHERE id = $1");
    let parent_group: Uuid = query
        .build_query_scalar()
        .bind(child_group)
        .fetch_one(pool)
        .await?;
    assert!(groups.contains(&child_group) && groups.contains(&parent_group));
    let tenant_table = InventoryEntity::Tenant.table().ok_or("a tenant table")?;
    let tenant_group = InventoryEntity::Tenant
        .field("group")
        .ok_or("tenant group")?
        .column();
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
    query
        .push(tenant_table)
        .push(" WHERE ")
        .push(tenant_group)
        .push(" = $1");
    let grouped: i64 = query
        .build_query_scalar()
        .bind(child_group)
        .fetch_one(pool)
        .await?;
    assert_eq!(grouped, 1, "the verified tenant sits in the child group");

    // Four selected custom-field definitions, and the device that carries them.
    let field_table = InventoryEntity::CustomField
        .table()
        .ok_or("a field table")?;
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT name FROM ");
    query.push(field_table).push(" ORDER BY name");
    let names: Vec<String> = query.build_query_scalar().fetch_all(pool).await?;
    assert_eq!(names.len(), 4, "selected custom-field definitions");
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT field_type FROM ");
    query.push(field_table).push(" ORDER BY field_type");
    let kinds: Vec<String> = query.build_query_scalar().fetch_all(pool).await?;
    assert_eq!(
        kinds,
        vec![
            "integer".to_string(),
            "multiobject".to_string(),
            "object".to_string(),
            "text".to_string(),
        ],
        "custom-field kinds"
    );

    let device_table = InventoryEntity::Device.table().ok_or("a device table")?;
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id, custom_field_data FROM ");
    query.push(device_table).push(" ORDER BY id");
    let devices: Vec<(Uuid, Value)> = query.build_query_as().fetch_all(pool).await?;
    let site_ids = binding_ids(pool, provider, InventoryEntity::Site).await?;
    let device_ids = binding_ids(pool, provider, InventoryEntity::Device).await?;
    let (_, data) = devices
        .iter()
        .find(|(_, data)| {
            data.as_object().is_some_and(|map| {
                map.values().any(|value| {
                    value
                        .as_str()
                        .and_then(|text| Uuid::parse_str(text).ok())
                        .is_some_and(|id| site_ids.contains(&id))
                })
            })
        })
        .ok_or("the device carrying the object custom-field edge")?;
    let data = data.as_object().ok_or("device custom-field data")?;
    assert_eq!(data.len(), 4, "the selected custom-field keys only");
    let (mut object_edges, mut multiobject_edges, mut text_values, mut numeric_values) =
        (0, 0, 0, 0);
    for value in data.values() {
        match value {
            Value::String(text) => {
                if Uuid::parse_str(text)
                    .ok()
                    .is_some_and(|id| site_ids.contains(&id))
                {
                    object_edges += 1;
                } else {
                    text_values += 1;
                }
            }
            Value::Array(items) => {
                let ids: Vec<Uuid> = items
                    .iter()
                    .filter_map(|item| item.as_str().and_then(|text| Uuid::parse_str(text).ok()))
                    .collect();
                assert_eq!(ids.len(), 2, "multiobject edge count");
                assert!(
                    ids.iter().all(|id| device_ids.contains(id)),
                    "multiobject edges are bound devices"
                );
                multiobject_edges += 1;
            }
            Value::Number(_) => numeric_values += 1,
            _ => return Err("an unexpected custom-field value shape".into()),
        }
    }
    assert_eq!(
        (object_edges, multiobject_edges, text_values, numeric_values),
        (1, 1, 1, 1),
        "object, multiobject, text and integer custom-field values"
    );
    for excluded in PWA81_TRANSFER_EXCLUDED_CUSTOM_FIELDS {
        assert!(
            !names.iter().any(|name| name.as_str() == excluded),
            "an excluded custom-field definition reached the target"
        );
        assert!(
            !data.contains_key(excluded),
            "an excluded custom-field value reached the target"
        );
    }

    // Tag membership travels as a native tag list on the tagged device.
    let tag_ids = binding_ids(pool, provider, InventoryEntity::Tag).await?;
    assert_eq!(tag_ids.len(), 1, "tag rows");
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
    query.push(device_table).push(" WHERE $1 = ANY(tags)");
    let tagged: i64 = query
        .build_query_scalar()
        .bind(tag_ids[0].to_string())
        .fetch_one(pool)
        .await?;
    assert_eq!(tagged, 1, "the verified tag membership");

    Ok(())
}

async fn operator_cli_roundtrip(pool: &PgPool, target_url: &str) -> TestResult {
    let transport = std::env::var("PWA81_TRANSFER_SNAPSHOT_TRANSPORT")
        .map_err(|_| "PWA81_TRANSFER_SNAPSHOT_TRANSPORT must name the private transport")?;
    let program = std::env::var("PWA81_TRANSFER_CLI_PATH")
        .map_err(|_| "PWA81_TRANSFER_CLI_PATH must name the executable under test")?;
    let proof = std::env::var("PWA81_TRANSFER_PROOF_DIR").map_or_else(
        |_| std::env::temp_dir().join(format!("pwa81_transfer_cli_{}", Uuid::now_v7().simple())),
        PathBuf::from,
    );
    private_dir(&proof)?;

    let transport_bytes = std::fs::read(&transport)?;
    if sha256_hex(&transport_bytes) != PWA81_TRANSFER_TRANSPORT_SHA256 {
        return Err("the configured transport is not the independently verified artifact".into());
    }

    let provider = Uuid::parse_str(PWA81_TRANSFER_PROVIDER_ID)?;
    let operator = Uuid::parse_str(PWA81_TRANSFER_OPERATOR_ID)?;
    common::ensure_test_admin(pool, PWA81_TRANSFER_PASSWORD_PEPPER).await?;
    sqlx::query("INSERT INTO sync_provider(id, name, provider_type, config, is_enabled) VALUES ($1, 'pwa81 populated synthetic', 'netbox', '{}', false)")
        .bind(provider)
        .execute(pool)
        .await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let token = issue_operator_token(pool, operator).await?;
    let mut cli = OperatorCli::new(
        &program,
        &proof,
        target_url,
        &common::redis_url(None)?,
        operator,
        token,
    );

    // Export seals the exact endorsed transport; nothing touches NetBox.
    let bundle_argument = cli.path(&cli.bundle, "bundle")?;
    let export = cli.run(
        "export",
        &[
            "export",
            "--snapshot-transport",
            transport.as_str(),
            "--provider-id",
            PWA81_TRANSFER_PROVIDER_ID,
            "--operator-id",
            cli.operator.as_str(),
            "--endorse-provenance-digest",
            PWA81_TRANSFER_TRANSPORT_SHA256,
            "--output",
            bundle_argument.as_str(),
        ],
        &[],
    )?;
    expect_exit(&export, "export", 0)?;
    let outcome: CaptureOutcome = decode(&export, "export", "/capture_outcome")?;
    assert_eq!(outcome, CaptureOutcome::Complete, "export completed");
    let authority: SourceAttestationIdentity = decode(&export, "export", "/authority")?;
    assert_eq!(
        authority.provenance_sha256, PWA81_TRANSFER_TRANSPORT_SHA256,
        "the seal covers the endorsed transport digest"
    );
    assert_eq!(
        authority.operator_id, operator,
        "the seal names the operator"
    );
    cli.bundle_digest = text(&export, "export", "/bundle_digest")?.to_string();

    // Mapping and profile are built from the closed catalogue and the sealed
    // authority, then pinned by their exact bytes.
    let mapping = identity_mapping(provider);
    let mapping_bytes = serde_json::to_vec(&mapping)?;
    let mapping_digest = sha256_hex(&mapping_bytes);
    let profile = PreflightProfile {
        format_version: 1,
        bundle_digest: cli.bundle_digest.clone(),
        source_provider_id: provider,
        mapping_ref: mapping.reference,
        mapping_digest: mapping_digest.clone(),
        catalog_digest: catalog_digest(),
        tool_digest: tool_digest(),
        snapshot_authority: Some(ExpectedSnapshotAuthority {
            operator_id: authority.operator_id,
            provenance_sha256: authority.provenance_sha256.clone(),
            issuer: authority.issuer.clone(),
            scope_digest: authority.scope_digest.clone(),
            expectations_digest: authority.expectations_sha256.clone(),
        }),
    };
    let profile_bytes = serde_json::to_vec(&profile)?;
    cli.profile_digest = sha256_hex(&profile_bytes);
    write_private(&cli.mapping, &mapping_bytes)?;
    write_private(&cli.profile, &profile_bytes)?;
    write_private(
        &cli.approval,
        &serde_json::to_vec(&TransferApproval {
            not_before: Utc::now() - Duration::minutes(5),
            expires_at: Utc::now() + Duration::hours(2),
        })?,
    )?;
    write_private(
        &cli.scope,
        &serde_json::to_vec(&TransferScope {
            creates: true,
            updates: true,
            owner_assignments: true,
            custom_fields: true,
            object_references: true,
            secret_intents: false,
        })?,
    )?;

    // Offline assessment and read-only target assessment agree with the pins.
    let preflight = cli.assessment("preflight", &[])?;
    let report: PreflightReport = decode(&preflight, "preflight", "")?;
    assert_offline_assessment(&report);
    expect_exit(&preflight, "preflight", 2)?;
    assert_eq!(report.bundle_digest, cli.bundle_digest, "bundle identity");
    assert_eq!(report.mapping_digest, mapping_digest, "mapping identity");
    assert_eq!(
        report.profile_digest, cli.profile_digest,
        "profile identity"
    );
    assert_eq!(report.catalog_digest, catalog_digest(), "catalog identity");
    assert_eq!(report.tool_digest, tool_digest(), "tool identity");
    assert!(
        report.authority.attested,
        "the profile pins the attestation"
    );
    assert_eq!(
        report.authority.attestation.as_ref(),
        Some(&authority),
        "the report exposes the verified attestation"
    );
    let dry_run = cli.assessment(
        "dry-run",
        &[("INVENTORY_TRANSFER_DATABASE_URL", cli.database.as_str())],
    )?;
    expect_exit(&dry_run, "dry-run", 0)?;
    let report: PreflightReport = decode(&dry_run, "dry-run", "")?;
    assert_assessment(&report, "dry-run");
    assert!(report.target.is_some(), "dry-run observes the target");
    assert!(
        report.observed_rows > 0,
        "the populated snapshot prepares rows"
    );

    // A conditional CREATE deny for the console-port-template class must refuse
    // the run before a single inventory row or binding exists.
    let deny_create = "pwa81 transfer deny console port template create";
    set_deny_policy(
        pool,
        deny_create,
        Action::Create,
        operator,
        &console_template_deny_constraint(None)?,
    )
    .await?;
    let denied_create = cli.prepare("denied-create", Uuid::now_v7())?;
    let before = target_snapshot(pool).await?;
    let refused = cli.transfer("apply-denied-create", "apply", &denied_create)?;
    expect_exit(&refused, "apply-denied-create", 2)?;
    assert_eq!(
        decode::<TransferRunState>(&refused, "apply-denied-create", "/state")?,
        TransferRunState::AuthorizationLost,
        "the real policy decision refuses the run"
    );
    assert_eq!(
        text(&refused, "apply-denied-create", "/code")?,
        "inventory_transfer:authorization_lost"
    );
    assert_snapshot(
        &before,
        &target_snapshot(pool).await?,
        "a denied CREATE leaves inventory and bindings untouched",
    );
    assert!(
        target
            .transfer_receipt(denied_create.run_id, &denied_create.digest)
            .await?
            .is_none(),
        "a denied run has no durable receipt"
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_job WHERE id = $1")
        .bind(denied_create.run_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(jobs, 0, "a denied run registers no job");
    assert_eq!(
        binding_count(pool, provider, InventoryEntity::ConsolePortTemplate).await?,
        0,
        "the denied template was never bound"
    );

    // Removing the deny lets a fresh descriptor commit the same snapshot.
    clear_policy(pool, deny_create).await?;
    let committed = cli.prepare("commit", Uuid::now_v7())?;
    let descriptor: TransferDescriptor = serde_json::from_slice(&std::fs::read(&committed.path)?)?;
    assert_eq!(descriptor.run_id, committed.run_id);
    assert_eq!(descriptor.provider_id, provider);
    assert_eq!(descriptor.operator.user_id, operator);
    assert_eq!(
        descriptor.authority.provenance_sha256, PWA81_TRANSFER_TRANSPORT_SHA256,
        "the descriptor pins the endorsed snapshot"
    );
    assert!(
        descriptor.scope.creates
            && descriptor.scope.updates
            && descriptor.scope.custom_fields
            && descriptor.scope.object_references,
        "the pinned scope covers the prepared effects"
    );
    assert!(!descriptor.scope.secret_intents);

    interrupted_apply(pool, &cli, &committed).await?;
    let applied = cli.transfer("resume-after-loss", "resume", &committed)?;
    expect_exit(&applied, "resume-after-loss", 0)?;
    assert_eq!(
        decode::<TransferRunState>(&applied, "apply", "/state")?,
        TransferRunState::Committed
    );
    assert_eq!(
        text(&applied, "apply", "/code")?,
        "inventory_transfer:resumed"
    );
    let receipt: TransferReceipt = decode(&applied, "apply", "/receipt")?;
    assert_eq!(receipt.run_id, committed.run_id);
    assert_eq!(receipt.descriptor_digest, committed.digest);
    assert_eq!(receipt.sync_job_id, committed.run_id);
    let jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sync_job WHERE id = $1 AND status = 'completed'")
            .bind(committed.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(jobs, 1, "resume commits exactly one completed job");
    assert_eq!(
        target
            .transfer_receipt(committed.run_id, &committed.digest)
            .await?
            .ok_or("a durable receipt")?,
        receipt,
        "the reported receipt is the durable one"
    );
    assert!(
        receipt.stats.created > 0,
        "the populated snapshot created rows"
    );
    assert_populated_target(pool, provider).await?;

    // Resume of the committed descriptor returns the same receipt and repeats
    // no effect.
    let after_commit = target_snapshot(pool).await?;
    let resumed = cli.transfer("resume", "resume", &committed)?;
    expect_exit(&resumed, "resume", 0)?;
    assert_eq!(
        decode::<TransferRunState>(&resumed, "resume", "/state")?,
        TransferRunState::Replayed
    );
    assert_eq!(
        decode::<TransferReceipt>(&resumed, "resume", "/receipt")?,
        receipt,
        "resume returns the same durable receipt"
    );
    assert_snapshot(
        &after_commit,
        &target_snapshot(pool).await?,
        "resume repeats no effect",
    );

    // A target-only label drift makes the identity mapping restore the source
    // value. The transport and its provenance stay exactly as exported.
    let console_table = InventoryEntity::ConsolePortTemplate
        .table()
        .ok_or("a template table")?;
    let console_label = InventoryEntity::ConsolePortTemplate
        .field("label")
        .ok_or("label field")?
        .column();
    let console_device_type = InventoryEntity::ConsolePortTemplate
        .field("device_type")
        .ok_or("device type field")?
        .column();
    let console_ids = binding_ids(pool, provider, InventoryEntity::ConsolePortTemplate).await?;
    assert_eq!(console_ids.len(), 1, "the verified console-port template");
    let console_id = console_ids[0];
    let mut label_query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    label_query
        .push(console_label)
        .push(" FROM ")
        .push(console_table)
        .push(" WHERE id = $1");
    let source_label: String = label_query
        .build_query_scalar()
        .bind(console_id)
        .fetch_one(pool)
        .await?;
    if source_label == PWA81_TRANSFER_DRIFT_LABEL {
        return Err("the source label already equals the drift fixture value".into());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE ");
    query
        .push(console_table)
        .push(" SET ")
        .push(console_label)
        .push(" = $1 WHERE id = $2");
    query
        .build()
        .bind(PWA81_TRANSFER_DRIFT_LABEL)
        .bind(console_id)
        .execute(pool)
        .await?;
    let drifted = target_snapshot(pool).await?;

    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    query
        .push(console_device_type)
        .push(" FROM ")
        .push(console_table)
        .push(" WHERE id = $1");
    let device_type: Option<Uuid> = query
        .build_query_scalar()
        .bind(console_id)
        .fetch_one(pool)
        .await?;
    let deny_update = "pwa81 transfer deny console port template update";
    set_deny_policy(
        pool,
        deny_update,
        Action::Update,
        operator,
        &console_template_deny_constraint(Some(device_type.ok_or("the template device type")?))?,
    )
    .await?;
    let denied_update = cli.prepare("denied-update", Uuid::now_v7())?;
    let blocked = cli.transfer("apply-denied-update", "apply", &denied_update)?;
    expect_exit(&blocked, "apply-denied-update", 2)?;
    assert_eq!(
        decode::<TransferRunState>(&blocked, "apply-denied-update", "/state")?,
        TransferRunState::AuthorizationLost,
        "the conditional UPDATE deny refuses the run"
    );
    assert_snapshot(
        &drifted,
        &target_snapshot(pool).await?,
        "a denied UPDATE writes nothing",
    );
    assert!(
        target
            .transfer_receipt(denied_update.run_id, &denied_update.digest)
            .await?
            .is_none()
    );
    let mut label_query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    label_query
        .push(console_label)
        .push(" FROM ")
        .push(console_table)
        .push(" WHERE id = $1");
    let current: String = label_query
        .build_query_scalar()
        .bind(console_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(
        current, PWA81_TRANSFER_DRIFT_LABEL,
        "the denied UPDATE leaves the persisted label"
    );

    // Removing the deny and pinning a new descriptor restores the source value
    // in place, keeping the qualified binding.
    clear_policy(pool, deny_update).await?;
    let restore = cli.prepare("restore", Uuid::now_v7())?;
    let restored = cli.transfer("apply-restore", "apply", &restore)?;
    expect_exit(&restored, "apply-restore", 0)?;
    assert_eq!(
        decode::<TransferRunState>(&restored, "apply-restore", "/state")?,
        TransferRunState::Committed
    );
    let restored_receipt: TransferReceipt = decode(&restored, "apply-restore", "/receipt")?;
    assert!(
        restored_receipt.stats.updated + restored_receipt.stats.created > 0,
        "the restore run wrote the drifted row"
    );
    let mut label_query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    label_query
        .push(console_label)
        .push(" FROM ")
        .push(console_table)
        .push(" WHERE id = $1");
    let current: String = label_query
        .build_query_scalar()
        .bind(console_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(
        current, source_label,
        "the identity mapping restored the source label"
    );
    assert_eq!(
        binding_ids(pool, provider, InventoryEntity::ConsolePortTemplate).await?,
        console_ids,
        "the update kept the qualified binding"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires PWA81_TRANSFER_SNAPSHOT_TRANSPORT, PWA81_TRANSFER_CLI_PATH, the guarded pwa81_transfer_ child database and Redis"]
async fn operator_cli_roundtrip_populated_snapshot_under_policy_decisions() -> TestResult {
    let (child, url) = ChildDatabase::create("pwa81_transfer_").await?;
    let pool = match PgPool::connect(&url).await {
        Ok(pool) => pool,
        Err(error) => {
            child.cleanup().await?;
            return Err(error.into());
        }
    };
    let outcome = std::panic::AssertUnwindSafe(async {
        Migrator::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .as_path(),
        )
        .await?
        .run(&pool)
        .await?;
        operator_cli_roundtrip(&pool, &url).await
    })
    .catch_unwind()
    .await;
    pool.close().await;
    let cleanup = child.cleanup().await;
    match outcome {
        Ok(result) => {
            result?;
            cleanup?;
            Ok(())
        }
        Err(panic) => {
            cleanup?;
            std::panic::resume_unwind(panic)
        }
    }
}
