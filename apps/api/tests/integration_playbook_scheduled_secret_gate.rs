//! Manual-selection and scheduled-fire service/persistence boundaries. The controlled authorizer
//! tests wiring, not the real access-control policy (covered by secret_gate).

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use piping_backend::contexts::access_control::domain::Principal;
use piping_backend::contexts::playbooks::application::{
    CreateScheduleInput, LaunchAuthorizationSnapshot, LaunchJobTemplateInput,
    PlaybookLaunchAuthorizer, PlaybookService,
};
use piping_backend::contexts::playbooks::domain::LaunchSpec;
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    PlaybookRepository, SqlxPlaybookRepository,
};
use piping_backend::contexts::shared::request_context::RequestContext;
use piping_backend::errors::AppError;
use piping_contracts::playbook_runner::{SnapshotContent, SnapshotEntry, snapshot_tree_sha256};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct Observed {
    owners: Vec<Uuid>,
    launches: Vec<(Principal, LaunchSpec)>,
}

struct ControlledAuthorizer {
    allow_owner: bool,
    allow_launch: bool,
    observed: Mutex<Observed>,
    remove_members_on_authorize: Option<(PgPool, Uuid)>,
    ssh_record_contexts: Vec<Uuid>,
    owner_pool: Option<PgPool>,
}

impl ControlledAuthorizer {
    fn new(allow_owner: bool, allow_launch: bool) -> Self {
        Self {
            allow_owner,
            allow_launch,
            observed: Mutex::new(Observed::default()),
            remove_members_on_authorize: None,
            ssh_record_contexts: Vec::new(),
            owner_pool: None,
        }
    }
}

#[async_trait]
impl PlaybookLaunchAuthorizer for ControlledAuthorizer {
    async fn validate_schedule_owner(&self, user: Uuid) -> Result<(), AppError> {
        self.observed
            .lock()
            .expect("authorizer lock")
            .owners
            .push(user);
        let active = if let Some(pool) = &self.owner_pool {
            sqlx::query_scalar::<_, bool>(
                "SELECT status = 'active' FROM identity_user WHERE id = $1",
            )
            .bind(user)
            .fetch_optional(pool)
            .await
            .map_err(|error| AppError::Validation(error.to_string()))?
            .unwrap_or(false)
        } else {
            self.allow_owner
        };
        if active {
            Ok(())
        } else {
            Err(AppError::Forbidden("owner inactive".into()))
        }
    }

    async fn authorize_launch(
        &self,
        principal: &Principal,
        request_context: Option<&RequestContext>,
        spec: &LaunchSpec,
    ) -> Result<LaunchAuthorizationSnapshot, AppError> {
        match principal {
            Principal::ServicePrincipal(_) => assert!(
                request_context.is_none(),
                "scheduled fires have no user request"
            ),
            Principal::User { id } => {
                if let Some(context) = request_context {
                    assert_eq!(context.user_id, Some(*id));
                }
            }
        }
        self.observed
            .lock()
            .expect("authorizer lock")
            .launches
            .push((principal.clone(), spec.clone()));
        if !self.allow_launch {
            return Err(AppError::Forbidden("launch denied".into()));
        }
        if let Some((pool, target_set_id)) = &self.remove_members_on_authorize {
            sqlx::query("DELETE FROM playbook_target_set_member WHERE target_set_id = $1")
                .bind(target_set_id)
                .execute(pool)
                .await
                .map_err(|error| {
                    AppError::Validation(format!("test target-set mutation: {error}"))
                })?;
        }
        let mut env = serde_json::Map::new();
        for (name, id) in &spec.options.secret_refs {
            env.insert(
                name.clone(),
                json!({"secret_id": id, "key": format!("schedule-secret-{}", id.simple())}),
            );
        }
        let subject = match principal {
            Principal::ServicePrincipal(id) => format!("service_principal:{}", id.get()),
            Principal::User { id } => format!("user:{id}"),
        };
        Ok(LaunchAuthorizationSnapshot {
            secret_env: Some(
                json!({"decided_at": Utc::now().to_rfc3339(), "subject": subject, "env": env}),
            ),
            ssh_record_contexts: self.ssh_record_contexts.clone(),
        })
    }
}

struct Fixture {
    pool: PgPool,
    repo: Arc<SqlxPlaybookRepository>,
    template_id: Uuid,
    target_set_id: Uuid,
    schedule_id: Uuid,
    scheduled_for: DateTime<Utc>,
    delegator: Uuid,
    principal: Uuid,
    secret: Uuid,
    fixed_target: Uuid,
    fixed_source: Uuid,
}

async fn seed(native: bool) -> Result<Fixture, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    let repo = Arc::new(SqlxPlaybookRepository::new(pool.clone()));
    let suffix = Uuid::now_v7().simple().to_string();
    let project = Uuid::now_v7();
    let revision = Uuid::now_v7();
    let environment = Uuid::now_v7();
    let template = Uuid::now_v7();
    let target_set = Uuid::now_v7();
    let (fixed_source, _) = common::seed_device(&pool, "schedule-fixed").await?;
    let fixed_target = fixed_source;
    let principal = Uuid::now_v7();
    let delegator = Uuid::now_v7();
    let secret = Uuid::now_v7();
    let schedule_id = Uuid::now_v7();
    let scheduled_for = Utc::now();
    let entries = vec![
        SnapshotEntry {
            path: "inventory.yml".into(),
            content: SnapshotContent::File {
                bytes: b"all:\n  hosts: {}\n".to_vec(),
                executable: false,
            },
        },
        SnapshotEntry {
            path: "site.yml".into(),
            content: SnapshotContent::File {
                bytes: b"---\n- hosts: all\n  tasks: []\n".to_vec(),
                executable: false,
            },
        },
    ];
    let digest = snapshot_tree_sha256(&entries)?;
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(delegator).bind(format!("schedule-{suffix}")).bind(format!("schedule-{suffix}@test.local")).execute(&pool).await?;
    sqlx::query("INSERT INTO playbook_service_principal (id, name, slug) VALUES ($1, $2, $3)")
        .bind(principal)
        .bind(format!("schedule-{suffix}"))
        .bind(format!("schedule-{suffix}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO secrets_variable (id, key, is_masked, is_protected) VALUES ($1, $2, true, false)")
        .bind(secret).bind(format!("schedule-secret-{}", secret.simple())).execute(&pool).await?;
    sqlx::query("INSERT INTO playbook_project (id, name, source) VALUES ($1, $2, $3)")
        .bind(project)
        .bind(format!("schedule-project-{suffix}"))
        .bind(json!({"kind":"editable"}))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, entries) VALUES ($1, $2, $3, $4)")
        .bind(revision).bind(project).bind(digest).bind(serde_json::to_value(entries)?).execute(&pool).await?;
    sqlx::query("UPDATE playbook_project SET current_revision_id = $2 WHERE id = $1")
        .bind(project)
        .bind(revision)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO playbook_execution_environment (id, name, image) VALUES ($1, $2, $3)")
        .bind(environment)
        .bind(format!("schedule-ee-{suffix}"))
        .bind(format!(
            "registry.example/ansible@sha256:{}",
            "a".repeat(64)
        ))
        .execute(&pool)
        .await?;
    // An empty target set makes accidental re-expansion visible: only the fixed
    // schedule snapshot can supply the managed target.
    sqlx::query("INSERT INTO playbook_target_set (id, name, mode) VALUES ($1, $2, 'static')")
        .bind(target_set)
        .bind(format!("schedule-targets-{suffix}"))
        .execute(&pool)
        .await?;
    let inventory = if native {
        json!({"kind":"native", "sources":["inventory.yml"], "ssh_record_id":null})
    } else {
        json!({"kind":"managed", "target_set_id":target_set})
    };
    let options = json!({"argv":[], "extra_vars":{}, "limit":null, "env":{}, "secret_refs":{"GRAD_TOKEN":secret}, "timeout_seconds":60});
    sqlx::query("INSERT INTO playbook_job_template (id, name, project_id, execution_environment_id, workdir, playbook_paths, revision_selection, inventory, options) VALUES ($1, $2, $3, $4, '.', ARRAY['site.yml'], $5, $6, $7)")
        .bind(template).bind(format!("schedule-template-{suffix}")).bind(project).bind(environment)
        .bind(json!({"kind":"fixed", "revision_id":revision})).bind(inventory).bind(options).execute(&pool).await?;
    let targets = if native {
        json!({"kind":"fixed", "targets":[]})
    } else {
        json!({"kind":"fixed", "targets":[{"hostname":"fixed-host", "target_id":fixed_target, "source_id":fixed_source}]})
    };
    sqlx::query("INSERT INTO playbook_schedule (id, template_id, targets, owner_user_id, name, cron_expression, timezone, misfire_policy, concurrency_policy, enabled, next_run_at) VALUES ($1, $2, $3, $4, $5, '0 3 * * *', 'UTC', 'fire_once', 'queue_pending', true, now() + interval '1 day')")
        .bind(schedule_id).bind(template).bind(targets).bind(delegator).bind(format!("schedule-{suffix}")).execute(&pool).await?;
    sqlx::query("INSERT INTO playbook_schedule_occurrence (schedule_id, scheduled_for, fired_at, created_at, updated_at) VALUES ($1, $2, $2, now(), now())")
        .bind(schedule_id).bind(scheduled_for).execute(&pool).await?;
    Ok(Fixture {
        pool,
        repo,
        template_id: template,
        target_set_id: target_set,
        schedule_id,
        scheduled_for,
        delegator,
        principal,
        secret,
        fixed_target,
        fixed_source,
    })
}

async fn assert_no_runs(f: &Fixture) -> TestResult {
    let (groups, runs): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM playbook_run_group WHERE schedule_id = $1), (SELECT count(*) FROM playbook_run r JOIN playbook_run_group g ON g.id = r.run_group_id WHERE g.schedule_id = $1)")
        .bind(f.schedule_id).fetch_one(&f.pool).await?;
    assert_eq!((groups, runs), (0, 0));
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn denied_schedule_save_persists_nothing() -> TestResult {
    let f = seed(false).await?;
    let schedule = f.repo.get_schedule(f.schedule_id).await?.expect("schedule");
    let service = PlaybookService::new(f.repo.clone())
        .with_launch_authorizer(Arc::new(ControlledAuthorizer::new(true, false)));
    let result = service
        .create_schedule(
            CreateScheduleInput {
                template_id: f.template_id,
                targets: schedule.targets,
                overrides: None,
                name: "denied schedule".into(),
                cron_expression: "0 3 * * *".into(),
                timezone: None,
                misfire_policy: None,
                concurrency_policy: None,
                enabled: Some(false),
                next_run_at: None,
                metadata: None,
            },
            Some(f.delegator),
        )
        .await;
    assert!(matches!(result, Err(AppError::Forbidden(_))));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM playbook_schedule WHERE name = 'denied schedule'")
            .fetch_one(&f.pool)
            .await?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn denied_launch_persists_nothing() -> TestResult {
    let f = seed(false).await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, false));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    assert!(matches!(
        service
            .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
            .await,
        Err(AppError::Forbidden(_))
    ));
    assert_no_runs(&f).await?;
    let observed = auth.observed.lock().expect("authorizer lock");
    assert_eq!(observed.owners, [f.delegator]);
    assert_eq!(observed.launches.len(), 1);
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn authorized_fire_preserves_identity_secret_refs_and_fixed_targets() -> TestResult {
    let f = seed(false).await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, true));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    let created = service
        .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
        .await?;
    assert_eq!(created.run_group.service_principal_id, None);
    assert_eq!(created.run_group.requested_by_user_id, Some(f.delegator));
    assert_eq!(created.run_group.launch_spec.options.secret_refs.len(), 1);
    assert_eq!(
        created
            .run_group
            .launch_spec
            .options
            .secret_refs
            .get("GRAD_TOKEN"),
        Some(&f.secret)
    );
    assert_eq!(
        created.run_group.metadata["secret_env_authorization"]["env"]["GRAD_TOKEN"]["secret_id"],
        json!(f.secret)
    );
    assert_eq!(
        created.run_group.metadata["secret_env_authorization"]["subject"],
        json!(format!("user:{}", f.delegator))
    );
    assert_eq!(created.runs.len(), 1);
    let targets = &created.run_group.launch_spec.targets;
    assert_eq!(targets.len(), 1);
    assert_eq!(
        (
            targets[0].hostname.as_str(),
            targets[0].target_id,
            targets[0].source_id
        ),
        ("fixed-host", Some(f.fixed_target), Some(f.fixed_source))
    );
    let observed = auth.observed.lock().expect("authorizer lock");
    assert_eq!(observed.owners, [f.delegator]);
    assert_eq!(observed.launches.len(), 1);
    assert!(matches!(&observed.launches[0].0, Principal::User { id } if *id == f.delegator));
    assert_eq!(
        observed.launches[0].1.options.secret_refs.get("GRAD_TOKEN"),
        Some(&f.secret)
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn missing_authorizer_fails_closed() -> TestResult {
    let f = seed(false).await?;
    let service = PlaybookService::new(f.repo.clone());
    assert!(matches!(
        service
            .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
            .await,
        Err(AppError::Config(_))
    ));
    assert_no_runs(&f).await
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn inactive_owner_denies_fire() -> TestResult {
    let f = seed(false).await?;
    sqlx::query("UPDATE identity_user SET status = 'inactive' WHERE id = $1")
        .bind(f.delegator)
        .execute(&f.pool)
        .await?;
    let mut controlled = ControlledAuthorizer::new(true, true);
    controlled.owner_pool = Some(f.pool.clone());
    let auth = Arc::new(controlled);
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    assert!(matches!(
        service
            .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
            .await,
        Err(AppError::Forbidden(_))
    ));
    assert_no_runs(&f).await?;
    assert!(
        auth.observed
            .lock()
            .expect("authorizer lock")
            .launches
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn deactivated_owner_stops_schedule_instead_of_retrying() -> TestResult {
    let f = seed(false).await?;
    sqlx::query("UPDATE identity_user SET status = 'inactive' WHERE id = $1")
        .bind(f.delegator)
        .execute(&f.pool)
        .await?;
    let mut controlled = ControlledAuthorizer::new(true, true);
    controlled.owner_pool = Some(f.pool.clone());
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(Arc::new(controlled));
    assert!(matches!(
        service
            .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
            .await,
        Err(AppError::Forbidden(_))
    ));
    assert_no_runs(&f).await?;
    // The scheduler stops the schedule after a permanent authority refusal
    // instead of leaving it enabled for the next cron tick.
    assert!(service.stop_schedule_for_owner_loss(f.schedule_id).await?);
    let schedule = f.repo.get_schedule(f.schedule_id).await?.expect("schedule");
    assert!(!schedule.enabled, "a stopped schedule must not fire again");
    assert_eq!(schedule.metadata["owner_migration_paused"], json!(true));
    assert_eq!(
        schedule.metadata["owner_migration_pause_reason"],
        json!("owner_access_revoked")
    );
    // The same occurrence is never retried and a repeated stop is a no-op.
    assert!(matches!(
        service
            .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
            .await,
        Err(AppError::Forbidden(_))
    ));
    assert!(!service.stop_schedule_for_owner_loss(f.schedule_id).await?);
    assert_no_runs(&f).await?;
    Ok(())
}

async fn seed_managed_members(
    f: &Fixture,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let (other, _) = common::seed_device(&f.pool, "manual-other").await?;
    for target_id in [f.fixed_target, other] {
        sqlx::query("INSERT INTO playbook_target_set_member (target_set_id, target_kind, target_id, source_kind, source_id) VALUES ($1, 'connection_target', $2, 'device', $2)")
            .bind(f.target_set_id).bind(target_id).execute(&f.pool).await?;
    }
    Ok(other)
}

fn manual_context(f: &Fixture) -> RequestContext {
    RequestContext::new(
        Some(f.delegator),
        "session".into(),
        None,
        format!("manual-selection-{}", Uuid::now_v7()),
        Default::default(),
    )
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn launch_persists_request_correlation_and_pat_initiator() -> TestResult {
    use piping_backend::contexts::audit::domain::{AuditEmitterKind, AuditOutcome};
    use piping_backend::contexts::playbooks::domain::automation_lifecycle_event;
    let f = seed(true).await?;
    let service = PlaybookService::new(f.repo.clone())
        .with_launch_authorizer(Arc::new(ControlledAuthorizer::new(true, true)));
    let request_id = Uuid::now_v7();
    let mut context = manual_context(&f);
    context.correlation_id = request_id.to_string();
    context.auth_method = "api_token".into();
    context.api_token_id = Some(Uuid::now_v7().to_string());
    let created = service
        .launch_job_template(
            f.template_id,
            LaunchJobTemplateInput {
                overrides: None,
                metadata: None,
                target_ids: None,
            },
            f.delegator,
            &context,
        )
        .await?;
    let stored = f
        .repo
        .get_run_group_with_runs(created.run_group.id, 1)
        .await?
        .expect("persisted group");
    let event = automation_lifecycle_event(
        "playbook.run_group.created",
        &stored.run_group,
        None,
        AuditOutcome::Unknown,
        AuditEmitterKind::Api,
        stored.run_group.created_at,
    )?;
    assert_eq!(event.actor_id, Some(f.delegator));
    assert_eq!(event.actor_kind, "api_token");
    assert_eq!(
        event.audit_context().correlation.request_id,
        Some(request_id)
    );
    assert_eq!(
        event.audit_context().correlation.run_group_id,
        Some(created.run_group.id)
    );
    assert!(event.audit_context().delegated_principal.is_none());
    let forged = service
        .launch_job_template(
            f.template_id,
            LaunchJobTemplateInput {
                overrides: None,
                metadata: Some(json!({"audit_context": event.audit_context()})),
                target_ids: None,
            },
            f.delegator,
            &context,
        )
        .await;
    assert!(matches!(forged, Err(AppError::Validation(_))));
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn launch_persists_only_authorizer_ssh_record_contexts() -> TestResult {
    let f = seed(true).await?;
    let authorized = [Uuid::now_v7(), Uuid::now_v7()];
    let forged = Uuid::now_v7();
    for contexts in [authorized.to_vec(), Vec::new()] {
        let mut gate = ControlledAuthorizer::new(true, true);
        gate.ssh_record_contexts = contexts.clone();
        let gate = Arc::new(gate);
        let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(gate.clone());
        let created = service
            .launch_job_template(
                f.template_id,
                LaunchJobTemplateInput {
                    overrides: None,
                    metadata: Some(json!({"ssh_record_contexts": [forged]})),
                    target_ids: None,
                },
                f.delegator,
                &manual_context(&f),
            )
            .await?;
        let persisted: Value =
            sqlx::query_scalar("SELECT metadata FROM playbook_run_group WHERE id = $1")
                .bind(created.run_group.id)
                .fetch_one(&f.pool)
                .await?;
        assert_eq!(persisted["ssh_record_contexts"], json!(contexts));
        assert_eq!(created.run_group.metadata, persisted);
        assert_eq!(
            persisted["secret_env_authorization"]["env"]["GRAD_TOKEN"]["secret_id"],
            json!(f.secret)
        );
        assert_eq!(
            gate.observed
                .lock()
                .expect("authorizer lock")
                .launches
                .len(),
            1
        );
    }
    Ok(())
}

async fn assert_no_manual_runs(f: &Fixture) -> TestResult {
    let (groups, runs): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM playbook_run_group WHERE template_id = $1), (SELECT count(*) FROM playbook_run r JOIN playbook_run_group g ON g.id = r.run_group_id WHERE g.template_id = $1)")
        .bind(f.template_id).fetch_one(&f.pool).await?;
    assert_eq!((groups, runs), (0, 0));
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn manual_selection_authorizes_and_persists_only_selected_snapshot() -> TestResult {
    let f = seed(false).await?;
    let selected = seed_managed_members(&f).await?;
    let mut authorizer = ControlledAuthorizer::new(true, true);
    // Mutation after authorization makes a second target-set resolution fail.
    // The persisted spec must remain exactly what the authorizer received.
    authorizer.remove_members_on_authorize = Some((f.pool.clone(), f.target_set_id));
    let auth = Arc::new(authorizer);
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    let created = service
        .launch_job_template(
            f.template_id,
            LaunchJobTemplateInput {
                overrides: None,
                metadata: None,
                target_ids: Some(vec![selected]),
            },
            f.delegator,
            &manual_context(&f),
        )
        .await?;
    assert_eq!(created.runs.len(), 1);
    assert_eq!(created.runs[0].target_id, Some(selected));
    assert_eq!(created.run_group.requested_by_user_id, Some(f.delegator));
    assert_eq!(created.run_group.service_principal_id, None);
    let stored: Value =
        sqlx::query_scalar("SELECT launch_spec FROM playbook_run_group WHERE id = $1")
            .bind(created.run_group.id)
            .fetch_one(&f.pool)
            .await?;
    let persisted_targets: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT target_id FROM playbook_run WHERE run_group_id = $1")
            .bind(created.run_group.id)
            .fetch_all(&f.pool)
            .await?;
    assert_eq!(persisted_targets, vec![Some(selected)]);
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM playbook_target_set_member WHERE target_set_id = $1",
    )
    .bind(f.target_set_id)
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(members, 0);
    let observed = auth.observed.lock().expect("authorizer lock");
    assert!(observed.owners.is_empty());
    assert_eq!(observed.launches.len(), 1);
    assert!(matches!(
        &observed.launches[0].0,
        Principal::User { id } if *id == f.delegator
    ));
    let spec = &observed.launches[0].1;
    assert_eq!(spec.targets.len(), 1);
    assert_eq!(spec.targets[0].target_id, Some(selected));
    assert_eq!(stored, serde_json::to_value(spec)?);
    assert_eq!(
        stored,
        serde_json::to_value(&created.run_group.launch_spec)?
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn manual_selection_absent_keeps_the_resolved_template_membership() -> TestResult {
    let f = seed(false).await?;
    let other = seed_managed_members(&f).await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, true));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    let input = serde_json::from_value::<LaunchJobTemplateInput>(json!({}))?;
    let created = service
        .launch_job_template(f.template_id, input, f.delegator, &manual_context(&f))
        .await?;
    let mut ids: Vec<_> = created
        .runs
        .iter()
        .filter_map(|run| run.target_id)
        .collect();
    ids.sort_unstable();
    let mut expected = vec![f.fixed_target, other];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert_eq!(
        auth.observed.lock().expect("authorizer lock").launches[0]
            .1
            .targets
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn manual_selection_rejects_empty_duplicate_foreign_and_nonexistent_ids() -> TestResult {
    let f = seed(false).await?;
    let selected = seed_managed_members(&f).await?;
    let (foreign, _) = common::seed_device(&f.pool, "manual-outside-set").await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, true));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    for ids in [
        vec![],
        vec![selected, selected],
        vec![selected, foreign],
        vec![Uuid::now_v7()],
        vec![Uuid::nil()],
    ] {
        let result = service
            .launch_job_template(
                f.template_id,
                LaunchJobTemplateInput {
                    overrides: None,
                    metadata: None,
                    target_ids: Some(ids),
                },
                f.delegator,
                &manual_context(&f),
            )
            .await;
        assert!(matches!(result, Err(AppError::Validation(_))));
        assert_no_manual_runs(&f).await?;
    }
    assert!(
        auth.observed
            .lock()
            .expect("authorizer lock")
            .launches
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn manual_selection_is_rejected_for_native_inventory_without_persistence() -> TestResult {
    let f = seed(true).await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, true));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    for ids in [vec![], vec![f.fixed_target]] {
        let result = service
            .launch_job_template(
                f.template_id,
                LaunchJobTemplateInput {
                    overrides: None,
                    metadata: None,
                    target_ids: Some(ids),
                },
                f.delegator,
                &manual_context(&f),
            )
            .await;
        assert!(matches!(result, Err(AppError::Validation(_))));
        assert_no_manual_runs(&f).await?;
    }
    assert!(
        auth.observed
            .lock()
            .expect("authorizer lock")
            .launches
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn manual_selection_still_requires_authorization_of_the_reduced_spec() -> TestResult {
    let f = seed(false).await?;
    let selected = seed_managed_members(&f).await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, false));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    let result = service
        .launch_job_template(
            f.template_id,
            LaunchJobTemplateInput {
                overrides: None,
                metadata: None,
                target_ids: Some(vec![selected]),
            },
            f.delegator,
            &manual_context(&f),
        )
        .await;
    assert!(matches!(result, Err(AppError::Forbidden(_))));
    assert_no_manual_runs(&f).await?;
    let observed = auth.observed.lock().expect("authorizer lock");
    assert_eq!(observed.launches.len(), 1);
    assert_eq!(observed.launches[0].1.targets.len(), 1);
    assert_eq!(observed.launches[0].1.targets[0].target_id, Some(selected));
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database migrated through schema 0016"]
async fn native_inventory_authorizes_and_persists_zero_hosts() -> TestResult {
    let f = seed(true).await?;
    let auth = Arc::new(ControlledAuthorizer::new(true, true));
    let service = PlaybookService::new(f.repo.clone()).with_launch_authorizer(auth.clone());
    let created = service
        .create_scheduled_run_group(f.schedule_id, f.scheduled_for)
        .await?;
    assert!(created.runs.is_empty());
    assert!(created.run_group.launch_spec.targets.is_empty());
    assert_eq!(created.run_group.total_runs, 0);
    assert_eq!(
        created
            .run_group
            .launch_spec
            .options
            .secret_refs
            .get("GRAD_TOKEN"),
        Some(&f.secret)
    );
    assert_eq!(
        auth.observed
            .lock()
            .expect("authorizer lock")
            .launches
            .len(),
        1
    );
    Ok(())
}
