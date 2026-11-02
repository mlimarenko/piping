//! DB-backed integration tests for the playbook executor data-plane.
//!
//! These exercise the new repository surface directly against a real Postgres:
//! - the timeout reaper (`timeout_stuck_runs`) fails a stuck `running` run and
//!   rolls its run group to a terminal state;
//! - schedule CRUD invariants; occurrence-ledger claiming and retry invariants
//!   are pinned by `compliance_playbooks_durable_leases`, while cron advancement
//!   is covered by the worker's pure `compute_next_run_at` unit tests.
//!
//! They are `#[ignore]`d like the rest of the repo's DB tests and run under
//! `make test` (which brings up the dev Postgres). If you cannot start Postgres
//! they will be skipped.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use piping_backend::contexts::playbooks::application::{
    CreateAssignmentInput, CreateProjectInput, PlaybookService, UpdateAssignmentInput,
};
use piping_backend::contexts::playbooks::domain::{
    PlaybookAssignmentScopeKind, PlaybookRunStatus, PlaybookScheduleConcurrencyPolicy,
    PlaybookScheduleMisfirePolicy, PlaybookServicePrincipalStatus, PlaybookTargetSetMode,
};
use piping_backend::contexts::playbooks::infrastructure::AnsibleProjectFile;
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateScheduleRecord, CreateServicePrincipalRecord, CreateTargetSetRecord, PlaybookRepository,
    PlaybookServicePrincipalListFilter, RevokeServicePrincipalRecord, SqlxPlaybookRepository,
    StaticTargetMemberRecord, TimeoutStuckRunsRecord, UpdateScheduleRecord,
};
use piping_backend::errors::AppError;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

async fn seed_playbook_with_version_and_risk(
    pool: &sqlx::PgPool,
    risk: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error + Send + Sync>> {
    let suffix = common::unique_suffix();
    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook (id, slug, name, status, runtime, risk, category)
        VALUES ($1, $2, 'Executor Reaper Test', 'active', 'ansible', $3, 'general')
        "#,
    )
    .bind(playbook_id)
    .bind(format!("exec-reaper-{suffix}"))
    .bind(risk)
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_version (
            id, playbook_id, version, language, runtime, content, content_sha256
        )
        VALUES ($1, $2, 1, 'yaml', 'ansible', '---\n- hosts: all\n', $3)
        "#,
    )
    .bind(version_id)
    .bind(playbook_id)
    // 64 hex chars (sha256 of "echo hi" is not validated here; any 64-hex passes the CHECK).
    .bind("0000000000000000000000000000000000000000000000000000000000000000")
    .execute(pool)
    .await?;
    sqlx::query("UPDATE playbook SET current_version_id = $2 WHERE id = $1")
        .bind(playbook_id)
        .bind(version_id)
        .execute(pool)
        .await?;
    Ok((playbook_id, version_id))
}

async fn seed_playbook_with_version(
    pool: &sqlx::PgPool,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error + Send + Sync>> {
    seed_playbook_with_version_and_risk(pool, "low").await
}

async fn seed_static_target_set(
    repo: &SqlxPlaybookRepository,
    suffix: &str,
    name: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let preview = repo
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("{name}-{suffix}"),
                mode: PlaybookTargetSetMode::Static,
                selector: json!({}),
                members: vec![StaticTargetMemberRecord {
                    target_kind: "connection_target".to_string(),
                    target_id: Uuid::now_v7(),
                    source_kind: "device".to_string(),
                    source_id: Uuid::now_v7(),
                    sort_order: 0,
                }],
                user_id: None,
            },
            100,
        )
        .await?;
    Ok(preview.target_set.id)
}

#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn timeout_stuck_runs_fails_overdue_run_and_rolls_up_group()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let (playbook_id, version_id) = seed_playbook_with_version(&pool).await?;

    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, playbook_id, playbook_version_id, trigger_kind, status, total_runs, started_at
        )
        VALUES ($1, $2, $3, 'manual', 'running', 1, NOW())
        "#,
    )
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(&pool)
    .await?;

    // A run that started long ago and exceeded its (tiny) timeout, mirroring the
    // crashed-worker state: status=running with lease_expires_at NULL.
    let run_id = Uuid::now_v7();
    let started_at = Utc::now() - Duration::seconds(120);
    sqlx::query(
        r#"
        INSERT INTO playbook_run (
            id, run_group_id, playbook_id, playbook_version_id, target_kind, target_id,
            source_kind, source_id, status, started_at, timeout_seconds, lease_owner, leased_at
        )
        VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $5, 'running', $6, 5,
                'worker-test', $6)
        "#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(Uuid::now_v7())
    .bind(started_at)
    .execute(&pool)
    .await?;

    let repo = SqlxPlaybookRepository::new(pool.clone());
    let result = repo
        .timeout_stuck_runs(TimeoutStuckRunsRecord {
            now: Utc::now(),
            limit: 50,
        })
        .await?;

    assert_eq!(result.runs.len(), 1, "exactly one run should be reaped");
    assert_eq!(result.runs[0].id, run_id);
    assert_eq!(result.runs[0].status, PlaybookRunStatus::TimedOut);
    assert!(result.runs[0].finished_at.is_some());

    assert_eq!(result.run_groups.len(), 1, "the group should roll up");
    let group = &result.run_groups[0];
    assert_eq!(group.id, group_id);
    assert_eq!(group.timeout_runs, 1);
    assert_eq!(group.status, PlaybookRunStatus::TimedOut);
    assert!(group.finished_at.is_some());
    assert!(
        group.lease_owner.is_none(),
        "terminal group clears its lease"
    );

    // Idempotency: a second reap finds nothing (the run is now terminal).
    let second = repo
        .timeout_stuck_runs(TimeoutStuckRunsRecord {
            now: Utc::now(),
            limit: 50,
        })
        .await?;
    assert!(
        second.runs.iter().all(|run| run.id != run_id),
        "a terminal run must not be reaped twice"
    );

    // Cleanup.
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(&pool)
        .await?;
    // `playbook_version` rows are immutable and reject cascade deletes unless
    // this transaction-local GUC is set (mirrors what the app's
    // `delete_playbook` service does inside its own transaction).
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(playbook_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(())
}

/// Smoke-test the new service-principal repository SQL round-trip
/// (create -> get -> list -> revoke) so a misspelled column, bind mismatch, or
/// tripped CHECK constraint surfaces here instead of as a runtime 500.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn service_principal_lifecycle_round_trips()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let repo = SqlxPlaybookRepository::new(pool.clone());
    let suffix = common::unique_suffix();
    let slug = format!("sp-{suffix}");

    let created = repo
        .create_service_principal(CreateServicePrincipalRecord {
            name: format!("Scheduler Identity {suffix}"),
            slug: slug.clone(),
            description: Some("scheduled-run identity".to_string()),
            metadata: json!({}),
            user_id: None,
        })
        .await?;
    assert_eq!(created.slug, slug);
    assert_eq!(created.status, PlaybookServicePrincipalStatus::Active);
    assert!(created.revoked_at.is_none());

    let fetched = repo
        .get_service_principal(created.id)
        .await?
        .expect("created service principal must be fetchable");
    assert_eq!(fetched.id, created.id);

    // Only the page matters here; the total match count is for pagination UIs.
    let (listed, _total) = repo
        .list_service_principals(&PlaybookServicePrincipalListFilter::default(), 500, 0)
        .await?;
    assert!(
        listed.iter().any(|principal| principal.id == created.id),
        "created service principal must appear in the list"
    );

    let revoked = repo
        .revoke_service_principal(RevokeServicePrincipalRecord {
            id: created.id,
            revocation_reason: Some("rotated".to_string()),
            user_id: None,
            now: Utc::now(),
        })
        .await?
        .expect("revoke must return the row");
    assert_eq!(revoked.status, PlaybookServicePrincipalStatus::Revoked);
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoked.revocation_reason.as_deref(), Some("rotated"));

    // Revoke is idempotent: the audit columns are not overwritten on repeat.
    let revoked_again = repo
        .revoke_service_principal(RevokeServicePrincipalRecord {
            id: created.id,
            revocation_reason: Some("second reason ignored".to_string()),
            user_id: None,
            now: Utc::now(),
        })
        .await?
        .expect("repeat revoke still returns the row");
    assert_eq!(revoked_again.revocation_reason.as_deref(), Some("rotated"));

    sqlx::query("DELETE FROM playbook_service_principal WHERE id = $1")
        .bind(created.id)
        .execute(&pool)
        .await?;

    Ok(())
}

/// Quick actions are intentionally limited to low-risk playbooks, including
/// when a PATCH retargets an otherwise eligible assignment.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn assignment_quick_action_requires_low_risk_playbook()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let repo = Arc::new(SqlxPlaybookRepository::new(pool.clone()));
    let service = PlaybookService::new(repo.clone());
    let suffix = common::unique_suffix();
    let (low_playbook_id, _) = seed_playbook_with_version(&pool).await?;
    let (medium_playbook_id, _) = seed_playbook_with_version_and_risk(&pool, "medium").await?;
    let target_set_id = seed_static_target_set(&repo, &suffix, "quick-action-set").await?;

    let rejected_create = service
        .create_assignment(
            CreateAssignmentInput {
                playbook_id: medium_playbook_id,
                target_set_id,
                default_version_id: None,
                name: None,
                scope_kind: PlaybookAssignmentScopeKind::Global,
                scope_id: None,
                priority: None,
                enabled: None,
                visible: None,
                executable: None,
                quick_action_enabled: Some(true),
                default_input: None,
                default_metadata: None,
                default_timeout_seconds: None,
                default_target_limit: None,
                metadata: None,
            },
            None,
        )
        .await;
    assert!(
        matches!(rejected_create, Err(AppError::Validation(message)) if message.contains("low-risk"))
    );

    let assignment = service
        .create_assignment(
            CreateAssignmentInput {
                playbook_id: low_playbook_id,
                target_set_id,
                default_version_id: None,
                name: None,
                scope_kind: PlaybookAssignmentScopeKind::Global,
                scope_id: None,
                priority: None,
                enabled: None,
                visible: None,
                executable: None,
                quick_action_enabled: Some(true),
                default_input: None,
                default_metadata: None,
                default_timeout_seconds: None,
                default_target_limit: None,
                metadata: None,
            },
            None,
        )
        .await?;

    let rejected_update = service
        .update_assignment(
            assignment.id,
            UpdateAssignmentInput {
                playbook_id: Some(medium_playbook_id),
                target_set_id: None,
                default_version_id: None,
                name: None,
                scope_kind: None,
                scope_id: None,
                priority: None,
                enabled: None,
                visible: None,
                executable: None,
                quick_action_enabled: None,
                default_input: None,
                default_metadata: None,
                default_timeout_seconds: None,
                default_target_limit: None,
                metadata: None,
            },
            None,
        )
        .await;
    assert!(
        matches!(rejected_update, Err(AppError::Validation(message)) if message.contains("low-risk"))
    );

    sqlx::query("DELETE FROM playbook_target_set WHERE id = $1")
        .bind(target_set_id)
        .execute(&pool)
        .await?;
    // `playbook_version` rows are immutable and reject cascade deletes unless
    // this transaction-local GUC is set (mirrors what the app's
    // `delete_playbook` service does inside its own transaction).
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(low_playbook_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(medium_playbook_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(())
}

/// Exercise the assignment service PATCH semantics over the SQL repository:
/// nullable-field clearing, default-version retention/clearing, scope validation,
/// referenced-resource failure, and delete.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn assignment_update_and_delete_round_trip()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let repo = Arc::new(SqlxPlaybookRepository::new(pool.clone()));
    let service = PlaybookService::new(repo.clone());
    let suffix = common::unique_suffix();
    let (playbook_id, version_id) = seed_playbook_with_version(&pool).await?;
    let (other_playbook_id, _other_version_id) = seed_playbook_with_version(&pool).await?;
    let target_set_id = seed_static_target_set(&repo, &suffix, "assignment-set").await?;
    let other_target_set_id =
        seed_static_target_set(&repo, &suffix, "assignment-set-other").await?;

    let created = service
        .create_assignment(
            CreateAssignmentInput {
                playbook_id,
                target_set_id,
                default_version_id: Some(version_id),
                name: Some(format!("assignment-{suffix}")),
                scope_kind: PlaybookAssignmentScopeKind::Global,
                scope_id: None,
                priority: Some(10),
                enabled: Some(true),
                visible: Some(true),
                executable: Some(true),
                quick_action_enabled: Some(false),
                default_input: Some(json!({"limit": 5})),
                default_metadata: Some(json!({})),
                default_timeout_seconds: Some(120),
                default_target_limit: Some(50),
                metadata: Some(json!({"owner": "test"})),
            },
            None,
        )
        .await?;

    let patched = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                playbook_id: None,
                target_set_id: None,
                default_version_id: None,
                name: Some(None),
                scope_kind: None,
                scope_id: None,
                priority: Some(25),
                enabled: Some(false),
                visible: None,
                executable: None,
                quick_action_enabled: Some(true),
                default_input: Some(json!({})),
                default_metadata: None,
                default_timeout_seconds: Some(None),
                default_target_limit: None,
                metadata: Some(json!({"owner": "updated"})),
            },
            None,
        )
        .await?
        .expect("assignment should update");
    assert_eq!(patched.name, None);
    assert_eq!(patched.priority, 25);
    assert!(!patched.enabled);
    assert!(patched.quick_action_enabled);
    assert_eq!(patched.default_version_id, Some(version_id));
    assert_eq!(patched.default_timeout_seconds, None);

    let global_scope_with_id = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                playbook_id: None,
                target_set_id: None,
                default_version_id: None,
                name: None,
                scope_kind: Some(PlaybookAssignmentScopeKind::Global),
                scope_id: Some(Some(Uuid::now_v7())),
                priority: None,
                enabled: None,
                visible: None,
                executable: None,
                quick_action_enabled: None,
                default_input: None,
                default_metadata: None,
                default_timeout_seconds: None,
                default_target_limit: None,
                metadata: None,
            },
            None,
        )
        .await;
    assert!(
        matches!(global_scope_with_id, Err(AppError::Validation(message)) if message.contains("must not set scope_id"))
    );

    let retargeted = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                playbook_id: Some(other_playbook_id),
                target_set_id: Some(other_target_set_id),
                default_version_id: None,
                name: Some(Some(format!("assignment-retargeted-{suffix}"))),
                scope_kind: None,
                scope_id: None,
                priority: None,
                enabled: Some(true),
                visible: None,
                executable: None,
                quick_action_enabled: None,
                default_input: None,
                default_metadata: None,
                default_timeout_seconds: None,
                default_target_limit: None,
                metadata: None,
            },
            None,
        )
        .await?
        .expect("assignment should retarget");
    assert_eq!(retargeted.playbook_id, other_playbook_id);
    assert_eq!(retargeted.target_set_id, other_target_set_id);
    assert_eq!(retargeted.default_version_id, None);

    let missing_target = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                playbook_id: None,
                target_set_id: Some(Uuid::now_v7()),
                default_version_id: None,
                name: None,
                scope_kind: None,
                scope_id: None,
                priority: None,
                enabled: None,
                visible: None,
                executable: None,
                quick_action_enabled: None,
                default_input: None,
                default_metadata: None,
                default_timeout_seconds: None,
                default_target_limit: None,
                metadata: None,
            },
            None,
        )
        .await;
    assert!(
        matches!(missing_target, Err(AppError::NotFound(message)) if message.contains("target set"))
    );

    assert!(service.delete_assignment(created.id).await?);
    assert!(service.get_assignment(created.id).await?.is_none());

    sqlx::query("DELETE FROM playbook_target_set WHERE id = $1")
        .bind(target_set_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM playbook_target_set WHERE id = $1")
        .bind(other_target_set_id)
        .execute(&pool)
        .await?;
    // `playbook_version` rows are immutable and reject cascade deletes unless
    // this transaction-local GUC is set (mirrors what the app's
    // `delete_playbook` service does inside its own transaction).
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(playbook_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(other_playbook_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(())
}

/// Smoke-test the schedule update SQL across the two CHECK-constraint-sensitive
/// paths: enable (requires a non-null `next_run_at`) and disable (must clear the
/// `lease_owner`/`lease_expires_at`/`leased_at` triple together), plus delete.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn schedule_update_toggles_enabled_and_updates_configuration()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let repo = SqlxPlaybookRepository::new(pool.clone());
    let (playbook_id, _version_id) = seed_playbook_with_version(&pool).await?;
    let suffix = common::unique_suffix();

    // A static target set referencing one arbitrary connection_target id. The
    // schedule create only validates FK existence (it does not preview), so the
    // member does not need to resolve to a real device for this test.
    let target_set_preview = repo
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("sched-set-{suffix}"),
                mode: PlaybookTargetSetMode::Static,
                selector: json!({}),
                members: vec![StaticTargetMemberRecord {
                    target_kind: "connection_target".to_string(),
                    target_id: Uuid::now_v7(),
                    source_kind: "device".to_string(),
                    source_id: Uuid::now_v7(),
                    sort_order: 0,
                }],
                user_id: None,
            },
            100,
        )
        .await?;
    let target_set_id = target_set_preview.target_set.id;

    let principal = repo
        .create_service_principal(CreateServicePrincipalRecord {
            name: format!("Sched Identity {suffix}"),
            slug: format!("sched-sp-{suffix}"),
            description: None,
            metadata: json!({}),
            user_id: None,
        })
        .await?;

    // Create an enabled schedule with a future next_run_at.
    let next_run_at = Utc::now() + Duration::hours(1);
    let schedule = repo
        .create_schedule(CreateScheduleRecord {
            playbook_id,
            target_set_id,
            service_principal_id: principal.id,
            name: format!("nightly-{suffix}"),
            cron_expression: "0 0 * * *".to_string(),
            timezone: "UTC".to_string(),
            misfire_policy: PlaybookScheduleMisfirePolicy::FireOnce,
            concurrency_policy: PlaybookScheduleConcurrencyPolicy::SkipIfRunning,
            enabled: true,
            next_run_at: Some(next_run_at),
            metadata: json!({}),
            user_id: None,
        })
        .await?;
    assert!(schedule.enabled);

    let disabled = repo
        .update_schedule(UpdateScheduleRecord {
            id: schedule.id,
            target_set_id: None,
            name: None,
            cron_expression: None,
            timezone: None,
            misfire_policy: None,
            concurrency_policy: None,
            enabled: Some(false),
            next_run_at: None,
            metadata: None,
            user_id: None,
        })
        .await?
        .expect("disable must return the row");
    assert!(!disabled.enabled);

    // Re-enable with a fresh next_run_at + a cron/timezone/concurrency change.
    let re_enabled = repo
        .update_schedule(UpdateScheduleRecord {
            id: schedule.id,
            target_set_id: None,
            name: None,
            cron_expression: Some("*/15 * * * *".to_string()),
            timezone: Some("Europe/Berlin".to_string()),
            misfire_policy: None,
            concurrency_policy: Some(PlaybookScheduleConcurrencyPolicy::QueuePending),
            enabled: Some(true),
            next_run_at: Some(Utc::now() + Duration::hours(2)),
            metadata: Some(json!({"note": "re-enabled"})),
            user_id: None,
        })
        .await?
        .expect("re-enable must return the row");
    assert!(re_enabled.enabled);
    assert_eq!(re_enabled.cron_expression, "*/15 * * * *");
    assert_eq!(re_enabled.timezone, "Europe/Berlin");
    assert_eq!(
        re_enabled.concurrency_policy,
        PlaybookScheduleConcurrencyPolicy::QueuePending
    );
    assert!(re_enabled.next_run_at.is_some());

    // Delete the schedule.
    assert!(repo.delete_schedule(schedule.id).await?);
    assert!(repo.get_schedule(schedule.id).await?.is_none());
    assert!(!repo.delete_schedule(schedule.id).await?);

    // Cleanup (FKs are ON DELETE RESTRICT for schedule->target_set/principal, so
    // delete the schedule first — already done above — then the rest).
    sqlx::query("DELETE FROM playbook_target_set WHERE id = $1")
        .bind(target_set_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM playbook_service_principal WHERE id = $1")
        .bind(principal.id)
        .execute(&pool)
        .await?;
    // `playbook_version` rows are immutable and reject cascade deletes unless
    // this transaction-local GUC is set (mirrors what the app's
    // `delete_playbook` service does inside its own transaction).
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(playbook_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(())
}

#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn timeout_stuck_runs_leaves_within_timeout_runs_alone()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let (playbook_id, version_id) = seed_playbook_with_version(&pool).await?;

    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, playbook_id, playbook_version_id, trigger_kind, status, total_runs, started_at
        )
        VALUES ($1, $2, $3, 'manual', 'running', 1, NOW())
        "#,
    )
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(&pool)
    .await?;

    // A run that started recently and is well within its timeout: must NOT reap.
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run (
            id, run_group_id, playbook_id, playbook_version_id, target_kind, target_id,
            source_kind, source_id, status, started_at, timeout_seconds
        )
        VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $5, 'running', NOW(), 1800)
        "#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await?;

    let repo = SqlxPlaybookRepository::new(pool.clone());
    let result = repo
        .timeout_stuck_runs(TimeoutStuckRunsRecord {
            now: Utc::now(),
            limit: 50,
        })
        .await?;
    assert!(
        result.runs.iter().all(|run| run.id != run_id),
        "a run within its timeout must not be reaped"
    );

    let status: String = sqlx::query("SELECT status FROM playbook_run WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await?
        .get("status");
    assert_eq!(status, "running");

    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(&pool)
        .await?;
    // `playbook_version` rows are immutable and reject cascade deletes unless
    // this transaction-local GUC is set (mirrors what the app's
    // `delete_playbook` service does inside its own transaction).
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(playbook_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(())
}

/// Upload -> read-back round trip for manual project files, plus the
/// runner-reserved `.piping_keys/` rejection at the same write boundary.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn project_files_upload_read_back_and_reserved_key_rejection()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let repo = Arc::new(SqlxPlaybookRepository::new(pool.clone()));
    let service = PlaybookService::new(repo.clone());
    let suffix = common::unique_suffix();

    let project = service
        .create_project(
            CreateProjectInput {
                name: format!("files-round-trip-{suffix}"),
                description: None,
                scm_type: Some("manual".to_string()),
                scm_url: None,
                scm_branch: None,
                scm_ref: None,
                playbook_base_path: Some("playbooks".to_string()),
                credential_id: None,
                default_execution_environment_id: None,
                sync_policy: None,
                metadata: None,
            },
            None,
        )
        .await?;

    // A project with no upload reads back as an empty list, never a 4xx.
    assert!(
        service
            .get_project_files(project.id)
            .await?
            .expect("project exists")
            .is_empty()
    );

    // The reserved key directory is rejected before anything is stored.
    let rejected = service
        .set_project_files(
            project.id,
            vec![AnsibleProjectFile {
                path: ".piping_keys/host1".to_string(),
                content: "key-material".to_string(),
                executable: false,
            }],
            None,
        )
        .await;
    assert!(
        matches!(rejected, Err(AppError::Validation(message)) if message.contains(".piping_keys"))
    );
    assert!(
        service
            .get_project_files(project.id)
            .await?
            .expect("project exists")
            .is_empty(),
        "a rejected upload must not persist anything"
    );

    let stored = service
        .set_project_files(
            project.id,
            vec![
                AnsibleProjectFile {
                    path: "./playbooks/site.yml".to_string(),
                    content: "---\n- hosts: all\n  tasks: []\n".to_string(),
                    executable: false,
                },
                AnsibleProjectFile {
                    path: "roles/common/tasks/main.yml".to_string(),
                    content: "---\n[]\n".to_string(),
                    executable: true,
                },
            ],
            None,
        )
        .await?
        .expect("project exists");
    assert_eq!(
        stored.metadata["project_files"]
            .as_array()
            .expect("stored project_files array")
            .len(),
        2
    );

    let read_back = service
        .get_project_files(project.id)
        .await?
        .expect("project exists");
    assert_eq!(read_back.len(), 2);
    // Normalization is persisted, so the leading `./` does not round-trip.
    assert_eq!(read_back[0].path, "playbooks/site.yml");
    assert!(!read_back[0].executable);
    assert_eq!(read_back[1].path, "roles/common/tasks/main.yml");
    assert!(read_back[1].executable);

    // Unknown project reads back as None, which the route maps to 404.
    assert!(service.get_project_files(Uuid::now_v7()).await?.is_none());

    sqlx::query("DELETE FROM playbook_project WHERE id = $1")
        .bind(project.id)
        .execute(&pool)
        .await?;

    Ok(())
}
