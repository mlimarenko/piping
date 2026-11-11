//! DB-backed tests for safe target-set deletion.
//!
//! A target set may be referenced by a managed job template (JSON inventory),
//! an assignment (FK) and a schedule (JSON target selection). Deleting a used
//! set must answer DELETE_BLOCKED with every reference and must not cascade the
//! assignment away; deleting a free set must still work.

mod common;
#[path = "common/project_workspace.rs"]
mod project_workspace;

use piping_backend::contexts::playbooks::domain::{
    InventorySelection, JobTemplate, PlaybookAssignmentScopeKind,
    PlaybookScheduleConcurrencyPolicy, PlaybookScheduleMisfirePolicy, PlaybookServicePrincipal,
    PlaybookTargetSet, PlaybookTargetSetMode, ScheduleTargets,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateAssignmentRecord, CreateJobTemplateRecord, CreateScheduleRecord,
    CreateTargetSetRecord, PlaybookRepository,
    SqlxPlaybookRepository, UpdateScheduleRecord,
};
use piping_backend::errors::AppError;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn migrated_pool() -> TestResult<PgPool> {
    std::env::var("PIPING_DATABASE__URL")
        .map_err(|_| "PIPING_DATABASE__URL must be set for ignored target-set delete tests")?;
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|error| format!("run migrations: {error}"))?;
    Ok(pool)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn delete_target_set_reports_every_reference_without_cascading_assignments() -> TestResult {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let workspace = project_workspace::seed_workspace(
        &pool,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let suffix = Uuid::now_v7();

    let used_set = repository
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("delete-blocked-{suffix}"),
                mode: PlaybookTargetSetMode::Dynamic,
                selector: json!({}),
                members: vec![],
                user_id: None,
            },
            50,
        )
        .await?
        .target_set;

    // A managed job template holds the reference in JSON, not through a FK.
    sqlx::query("UPDATE playbook_job_template SET inventory = $2 WHERE id = $1")
        .bind(workspace.template.id)
        .bind(serde_json::to_value(InventorySelection::Managed {
            target_set_id: used_set.id,
        })?)
        .execute(&pool)
        .await?;

    let assignment = repository
        .create_assignment(CreateAssignmentRecord {
            job_template_id: workspace.template.id,
            target_set_id: used_set.id,
            name: Some(format!("delete-blocked-assignment-{suffix}")),
            scope_kind: PlaybookAssignmentScopeKind::Global,
            scope_id: None,
            priority: 0,
            enabled: true,
            visible: true,
            executable: true,
            quick_action_enabled: false,
            default_input: json!({}),
            default_metadata: json!({}),
            default_timeout_seconds: None,
            default_target_limit: None,
            metadata: json!({}),
            user_id: None,
        })
        .await?;

    let user_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source) \
         VALUES ($1, $2, $3, 'active', 'local')",
    )
    .bind(user_id)
    .bind(format!("delete-blocked-{suffix}"))
    .bind(format!("delete-blocked-{suffix}@test.local"))
    .execute(&pool)
    .await?;
    let schedule = repository
        .create_schedule(CreateScheduleRecord {
            overrides: Default::default(),
            template_id: workspace.template.id,
            targets: ScheduleTargets::Dynamic {
                selector: json!({}),
                target_set_id: Some(used_set.id),
            },
            owner_user_id: user_id,
            name: format!("delete-blocked-schedule-{suffix}"),
            cron_expression: "0 0 * * *".into(),
            timezone: "UTC".into(),
            misfire_policy: PlaybookScheduleMisfirePolicy::FireOnce,
            concurrency_policy: PlaybookScheduleConcurrencyPolicy::SkipIfRunning,
            enabled: false,
            next_run_at: None,
            metadata: json!({}),
            user_id: None,
        })
        .await?;

    let error = repository
        .delete_target_set(used_set.id)
        .await
        .expect_err("a used target set must block deletion");
    let AppError::DeleteBlocked(details) = &error else {
        panic!("expected DeleteBlocked, got {error:?}");
    };
    assert_eq!(details.total_count, 3, "{details:?}");
    for table in [
        "playbook_job_template",
        "playbook_assignment",
        "playbook_schedule",
    ] {
        let group = details
            .blockers
            .iter()
            .find(|group| group.table == table)
            .unwrap_or_else(|| panic!("missing {table} blocker: {details:?}"));
        assert_eq!(group.count, 1, "{group:?}");
        assert_eq!(group.examples.len(), 1, "{group:?}");
        assert!(group.examples[0].id.is_some(), "{group:?}");
    }

    // The assignment must not be cascaded away by the refused delete.
    assert!(
        repository.get_target_set(used_set.id).await?.is_some(),
        "refused delete must keep the target set"
    );
    let assignment_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM playbook_assignment WHERE id = $1")
            .bind(assignment.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(assignment_count, 1, "assignment was cascaded away");

    // Removing the references makes the same delete succeed.
    assert!(repository.delete_schedule(schedule.id).await?);
    workspace.cleanup(&pool).await?;
    assert!(repository.delete_target_set(used_set.id).await?);
    assert!(!repository.delete_target_set(used_set.id).await?);

    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn delete_free_target_set_succeeds_and_absent_is_false() -> TestResult {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let suffix = Uuid::now_v7();
    let free_set = repository
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("delete-free-{suffix}"),
                mode: PlaybookTargetSetMode::Dynamic,
                selector: json!({}),
                members: vec![],
                user_id: None,
            },
            50,
        )
        .await?
        .target_set;

    assert!(repository.delete_target_set(free_set.id).await?);
    assert!(!repository.delete_target_set(free_set.id).await?);
    Ok(())
}

/// A pool large enough for the blocked delete, its reference writer, the
/// ordering transaction and the lock observer to coexist.
async fn concurrency_pool() -> TestResult<PgPool> {
    let url = std::env::var("PIPING_DATABASE__URL")
        .map_err(|_| "PIPING_DATABASE__URL must be set for ignored target-set delete tests")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await?;
    Ok(pool)
}

/// Wait until `expected` other backends are blocked on a statement that touches
/// `playbook_target_set`. The wait is bounded and observes real lock state, so
/// the test never guesses a race window.
async fn wait_for_target_set_lock_waiters(pool: &PgPool, expected: i64) -> TestResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT activity.pid) \
             FROM pg_stat_activity AS activity \
             JOIN pg_locks AS lock ON lock.pid = activity.pid \
             WHERE NOT lock.granted \
               AND activity.datname = current_database() \
               AND activity.pid <> pg_backend_pid() \
               AND activity.query LIKE '%playbook_target_set%'",
        )
        .fetch_one(pool)
        .await?;
        if waiting >= expected {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for {expected} target-set lock waiter(s); observed {waiting}"
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Hold the same lock a delete takes, so the test can order the writer and the
/// delete on the target set row without a product test hook.
async fn hold_delete_lock(
    blocker: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> TestResult {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM playbook_target_set WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut **blocker)
        .await?;
    Ok(())
}

async fn seed_user(pool: &PgPool, suffix: Uuid) -> TestResult<Uuid> {
    let user_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source) \
         VALUES ($1, $2, $3, 'active', 'local')",
    )
    .bind(user_id)
    .bind(format!("target-set-lock-{suffix}"))
    .bind(format!("target-set-lock-{suffix}@test.local"))
    .execute(pool)
    .await?;
    Ok(user_id)
}


async fn seed_principal(pool: &PgPool, suffix: Uuid) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_service_principal (id, name, slug) VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(format!("Target set lock {suffix}"))
    .bind(format!("target-set-lock-{suffix}"))
    .execute(pool)
    .await?;
    Ok(id)
}

async fn create_dynamic_set(
    repository: &SqlxPlaybookRepository,
    suffix: Uuid,
) -> TestResult<PlaybookTargetSet> {
    Ok(repository
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("lock-set-{suffix}"),
                mode: PlaybookTargetSetMode::Dynamic,
                selector: json!({}),
                members: vec![],
                user_id: None,
            },
            50,
        )
        .await?
        .target_set)
}

fn schedule_record(
    template_id: Uuid,
    user_id: Uuid,
    name: String,
    targets: ScheduleTargets,
) -> CreateScheduleRecord {
    CreateScheduleRecord {
        overrides: Default::default(),
        template_id,
        targets,
        owner_user_id: user_id,
        name,
        cron_expression: "0 0 * * *".into(),
        timezone: "UTC".into(),
        misfire_policy: PlaybookScheduleMisfirePolicy::FireOnce,
        concurrency_policy: PlaybookScheduleConcurrencyPolicy::SkipIfRunning,
        enabled: false,
        next_run_at: None,
        metadata: json!({}),
        user_id: None,
    }
}

async fn cleanup_principal_user(pool: &PgPool, principal_id: Uuid, user_id: Uuid) -> TestResult {
    sqlx::query("DELETE FROM playbook_service_principal WHERE id = $1")
        .bind(principal_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn managed_template(
    workspace: &project_workspace::WorkspaceFixture,
    name: String,
    target_set_id: Uuid,
) -> JobTemplate {
    JobTemplate {
        id: Uuid::now_v7(),
        name,
        project_id: workspace.project.id,
        workdir: workspace.template.workdir.clone(),
        playbook_paths: workspace.template.playbook_paths.clone(),
        revision: workspace.template.revision.clone(),
        inventory: InventorySelection::Managed { target_set_id },
        vars_schema: Default::default(),
        options: workspace.template.options.clone(),
        execution_environment_id: workspace.execution_environment.id,
    }
}

/// Delete-first: once the set is gone, every JSON reference writer must reject
/// the write inside its transaction. The old plain-read check left a dangling
/// schedule or job template behind.
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn deleted_target_set_rejects_new_managed_and_schedule_references() -> TestResult {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let workspace = project_workspace::seed_workspace(
        &pool,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let suffix = Uuid::now_v7();
    let target_set = create_dynamic_set(&repository, suffix).await?;
    let user_id = seed_user(&pool, suffix).await?;
    let principal = seed_principal(&pool, suffix).await?;
    let existing_schedule = repository
        .create_schedule(schedule_record(
            workspace.template.id,
            user_id,
            format!("lock-existing-{suffix}"),
            ScheduleTargets::Dynamic {
                selector: json!({}),
                target_set_id: None,
            },
        ))
        .await?;

    assert!(repository.delete_target_set(target_set.id).await?);

    let managed_error = repository
        .create_job_template(CreateJobTemplateRecord {
            template: managed_template(&workspace, format!("lock-managed-{suffix}"), target_set.id),
            user_id: None,
        })
        .await
        .expect_err("a managed job template must not reference a deleted target set");
    assert!(
        matches!(managed_error, AppError::Validation(_)),
        "expected validation, got {managed_error:?}"
    );

    for (label, targets) in [
        (
            "fixed",
            ScheduleTargets::Fixed {
                targets: vec![],
                target_set_id: Some(target_set.id),
            },
        ),
        (
            "dynamic",
            ScheduleTargets::Dynamic {
                selector: json!({}),
                target_set_id: Some(target_set.id),
            },
        ),
    ] {
        let error = repository
            .create_schedule(schedule_record(
                workspace.template.id,
                user_id,
                format!("lock-{label}-{suffix}"),
                targets,
            ))
            .await
            .expect_err("a schedule must not reference a deleted target set");
        assert!(
            matches!(error, AppError::Validation(_)),
            "{label} schedule: expected validation, got {error:?}"
        );
    }

    let update_error = repository
        .update_schedule(UpdateScheduleRecord {
            overrides: None,
            id: existing_schedule.id,
            targets: Some(ScheduleTargets::Dynamic {
                selector: json!({}),
                target_set_id: Some(target_set.id),
            }),
            name: None,
            cron_expression: None,
            timezone: None,
            misfire_policy: None,
            concurrency_policy: None,
            enabled: None,
            next_run_at: None,
            metadata: None,
            user_id: None,
        })
        .await
        .expect_err("a schedule update must not reference a deleted target set");
    assert!(
        matches!(update_error, AppError::Validation(_)),
        "expected validation, got {update_error:?}"
    );

    let dangling: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM playbook_job_template \
                  WHERE inventory->>'target_set_id' = $1) \
              + (SELECT COUNT(*) FROM playbook_schedule \
                  WHERE targets->>'target_set_id' = $1)",
    )
    .bind(target_set.id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(dangling, 0, "a rejected write left a dangling reference");
    assert!(repository.get_target_set(target_set.id).await?.is_none());

    let reloaded = repository
        .get_schedule(existing_schedule.id)
        .await?
        .expect("the rejected update must keep the schedule");
    assert!(
        matches!(
            &reloaded.targets,
            ScheduleTargets::Dynamic {
                target_set_id: None,
                ..
            }
        ),
        "the rejected update changed the stored targets: {:?}",
        reloaded.targets
    );

    repository.delete_schedule(existing_schedule.id).await?;
    workspace.cleanup(&pool).await?;
    cleanup_principal_user(&pool, principal, user_id).await?;
    Ok(())
}

/// The reference write and the delete serialize on the target set row: either
/// the committed schedule is reported by the waiting delete, or the delete wins
/// and the writer is rejected. Both succeeding would leave a dangling JSON
/// reference.
async fn concurrent_schedule_create_and_delete(
    label: &'static str,
    targets: impl FnOnce(Uuid) -> ScheduleTargets + Send + 'static,
) -> TestResult {
    let setup = migrated_pool().await?;
    let concurrency = concurrency_pool().await?;
    let repository = SqlxPlaybookRepository::new(concurrency.clone());
    let workspace = project_workspace::seed_workspace(
        &setup,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let suffix = Uuid::now_v7();
    let target_set = create_dynamic_set(&repository, suffix).await?;
    let user_id = seed_user(&setup, suffix).await?;
    let principal = seed_principal(&setup, suffix).await?;
    let template_id = workspace.template.id;
    let set_id = target_set.id;

    let mut blocker = concurrency.begin().await?;
    hold_delete_lock(&mut blocker, set_id).await?;

    // The delete starts first but blocks before its usage read; only the row
    // lock can keep the two from both committing.
    let delete_repository = repository.clone();
    let deleter = tokio::spawn(async move { delete_repository.delete_target_set(set_id).await });
    wait_for_target_set_lock_waiters(&concurrency, 1).await?;

    let writer_repository = repository.clone();
    let writer = tokio::spawn(async move {
        writer_repository
            .create_schedule(schedule_record(
                template_id,
                user_id,
                format!("lock-{label}-{suffix}"),
                targets(set_id),
            ))
            .await
    });
    let writer_queued = wait_for_target_set_lock_waiters(&concurrency, 2)
        .await
        .is_ok();

    blocker.commit().await?;

    let delete_result = tokio::time::timeout(Duration::from_secs(30), deleter)
        .await
        .expect("delete task timed out")
        .expect("delete task panicked");
    let writer_result = tokio::time::timeout(Duration::from_secs(30), writer)
        .await
        .expect("writer task timed out")
        .expect("writer task panicked");

    assert!(
        writer_queued,
        "the schedule writer did not contend on the target-set row lock"
    );
    match (delete_result, writer_result) {
        (Ok(true), Ok(schedule)) => {
            let stored: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM playbook_schedule WHERE id = $1")
                    .bind(schedule.id)
                    .fetch_one(&setup)
                    .await?;
            panic!(
                "the delete and the schedule write both committed; dangling schedule rows: {stored}"
            );
        }
        (Err(AppError::DeleteBlocked(details)), Ok(schedule)) => {
            assert_eq!(details.total_count, 1, "{details:?}");
            assert_eq!(
                details.blockers[0].table, "playbook_schedule",
                "{details:?}"
            );
            assert!(repository.get_target_set(set_id).await?.is_some());
            let stored: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM playbook_schedule WHERE id = $1")
                    .bind(schedule.id)
                    .fetch_one(&setup)
                    .await?;
            assert_eq!(stored, 1, "the committed schedule was not stored");
            repository.delete_schedule(schedule.id).await?;
        }
        (Ok(true), Err(error)) => {
            assert!(
                matches!(error, AppError::Validation(_)),
                "unexpected writer error: {error:?}"
            );
            let dangling: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM playbook_schedule WHERE targets->>'target_set_id' = $1",
            )
            .bind(set_id.to_string())
            .fetch_one(&setup)
            .await?;
            assert_eq!(dangling, 0, "the rejected writer left a dangling schedule");
            assert!(repository.get_target_set(set_id).await?.is_none());
        }
        (delete, writer) => panic!("unexpected outcomes: delete={delete:?}, writer={writer:?}"),
    }

    repository.delete_target_set(set_id).await?;
    workspace.cleanup(&setup).await?;
    cleanup_principal_user(&setup, principal, user_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn concurrent_dynamic_schedule_reference_and_delete_never_both_commit() -> TestResult {
    concurrent_schedule_create_and_delete("dynamic", |target_set_id| ScheduleTargets::Dynamic {
        selector: json!({}),
        target_set_id: Some(target_set_id),
    })
    .await
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn concurrent_fixed_schedule_reference_and_delete_never_both_commit() -> TestResult {
    concurrent_schedule_create_and_delete("fixed", |target_set_id| ScheduleTargets::Fixed {
        targets: vec![],
        target_set_id: Some(target_set_id),
    })
    .await
}

/// The update path takes the same reference lock: the committed dynamic target
/// selection is either reported by the waiting delete or rejected after the
/// delete wins. Both succeeding would leave a dangling JSON reference.
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn concurrent_dynamic_schedule_update_and_delete_never_both_commit() -> TestResult {
    let setup = migrated_pool().await?;
    let concurrency = concurrency_pool().await?;
    let repository = SqlxPlaybookRepository::new(concurrency.clone());
    let workspace = project_workspace::seed_workspace(
        &setup,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let suffix = Uuid::now_v7();
    let target_set = create_dynamic_set(&repository, suffix).await?;
    let user_id = seed_user(&setup, suffix).await?;
    let principal = seed_principal(&setup, suffix).await?;
    let schedule = repository
        .create_schedule(schedule_record(
            workspace.template.id,
            user_id,
            format!("lock-update-{suffix}"),
            ScheduleTargets::Dynamic {
                selector: json!({}),
                target_set_id: None,
            },
        ))
        .await?;
    let schedule_id = schedule.id;
    let set_id = target_set.id;

    let mut blocker = concurrency.begin().await?;
    hold_delete_lock(&mut blocker, set_id).await?;

    let delete_repository = repository.clone();
    let deleter = tokio::spawn(async move { delete_repository.delete_target_set(set_id).await });
    wait_for_target_set_lock_waiters(&concurrency, 1).await?;

    let writer_repository = repository.clone();
    let writer = tokio::spawn(async move {
        writer_repository
            .update_schedule(UpdateScheduleRecord {
                overrides: None,
                id: schedule_id,
                targets: Some(ScheduleTargets::Dynamic {
                    selector: json!({}),
                    target_set_id: Some(set_id),
                }),
                name: None,
                cron_expression: None,
                timezone: None,
                misfire_policy: None,
                concurrency_policy: None,
                enabled: None,
                next_run_at: None,
                metadata: None,
                user_id: None,
            })
            .await
    });
    let writer_queued = wait_for_target_set_lock_waiters(&concurrency, 2)
        .await
        .is_ok();

    blocker.commit().await?;

    let delete_result = tokio::time::timeout(Duration::from_secs(30), deleter)
        .await
        .expect("delete task timed out")
        .expect("delete task panicked");
    let writer_result = tokio::time::timeout(Duration::from_secs(30), writer)
        .await
        .expect("writer task timed out")
        .expect("writer task panicked");

    assert!(
        writer_queued,
        "the schedule update did not contend on the target-set row lock"
    );
    match (delete_result, writer_result) {
        (Ok(true), Ok(Some(updated))) => panic!(
            "the delete and the schedule update both committed; stored targets: {:?}",
            updated.targets
        ),
        (Err(AppError::DeleteBlocked(details)), Ok(Some(updated))) => {
            assert_eq!(details.total_count, 1, "{details:?}");
            assert_eq!(
                details.blockers[0].table, "playbook_schedule",
                "{details:?}"
            );
            assert!(matches!(
                &updated.targets,
                ScheduleTargets::Dynamic {
                    target_set_id: Some(id),
                    ..
                } if *id == set_id
            ));
            assert!(repository.get_target_set(set_id).await?.is_some());
        }
        (Ok(true), Err(error)) => {
            assert!(
                matches!(error, AppError::Validation(_)),
                "unexpected writer error: {error:?}"
            );
            let dangling: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM playbook_schedule WHERE targets->>'target_set_id' = $1",
            )
            .bind(set_id.to_string())
            .fetch_one(&setup)
            .await?;
            assert_eq!(dangling, 0, "the rejected update left a dangling schedule");
            assert!(repository.get_target_set(set_id).await?.is_none());
        }
        (delete, writer) => panic!("unexpected outcomes: delete={delete:?}, writer={writer:?}"),
    }

    repository.delete_schedule(schedule_id).await?;
    repository.delete_target_set(set_id).await?;
    workspace.cleanup(&setup).await?;
    cleanup_principal_user(&setup, principal, user_id).await?;
    Ok(())
}

/// The managed job-template update path takes the reference lock too: the
/// committed managed inventory is either reported by the waiting delete or
/// rejected after the delete wins.
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn concurrent_managed_job_template_update_and_delete_never_both_commit() -> TestResult {
    let setup = migrated_pool().await?;
    let concurrency = concurrency_pool().await?;
    let repository = SqlxPlaybookRepository::new(concurrency.clone());
    let workspace = project_workspace::seed_workspace(
        &setup,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let suffix = Uuid::now_v7();
    let target_set = create_dynamic_set(&repository, suffix).await?;
    let template_id = workspace.template.id;
    let set_id = target_set.id;

    let mut blocker = concurrency.begin().await?;
    hold_delete_lock(&mut blocker, set_id).await?;

    let delete_repository = repository.clone();
    let deleter = tokio::spawn(async move { delete_repository.delete_target_set(set_id).await });
    wait_for_target_set_lock_waiters(&concurrency, 1).await?;

    let writer_repository = repository.clone();
    let template = managed_template(&workspace, format!("lock-template-{suffix}"), set_id);
    let writer = tokio::spawn(async move {
        writer_repository
            .update_job_template(
                template_id,
                CreateJobTemplateRecord {
                    template,
                    user_id: None,
                },
            )
            .await
    });
    let writer_queued = wait_for_target_set_lock_waiters(&concurrency, 2)
        .await
        .is_ok();

    blocker.commit().await?;

    let delete_result = tokio::time::timeout(Duration::from_secs(30), deleter)
        .await
        .expect("delete task timed out")
        .expect("delete task panicked");
    let writer_result = tokio::time::timeout(Duration::from_secs(30), writer)
        .await
        .expect("writer task timed out")
        .expect("writer task panicked");

    assert!(
        writer_queued,
        "the template update did not contend on the target-set row lock"
    );
    match (delete_result, writer_result) {
        (Ok(true), Ok(Some(updated))) => panic!(
            "the delete and the template update both committed; stored inventory: {:?}",
            updated.inventory
        ),
        (Err(AppError::DeleteBlocked(details)), Ok(Some(updated))) => {
            assert_eq!(details.total_count, 1, "{details:?}");
            assert_eq!(
                details.blockers[0].table, "playbook_job_template",
                "{details:?}"
            );
            assert!(matches!(
                &updated.inventory,
                InventorySelection::Managed { target_set_id } if *target_set_id == set_id
            ));
            assert!(repository.get_target_set(set_id).await?.is_some());
        }
        (Ok(true), Err(error)) => {
            assert!(
                matches!(error, AppError::Validation(_)),
                "unexpected writer error: {error:?}"
            );
            let dangling: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM playbook_job_template \
                 WHERE inventory->>'target_set_id' = $1",
            )
            .bind(set_id.to_string())
            .fetch_one(&setup)
            .await?;
            assert_eq!(dangling, 0, "the rejected update left a dangling template");
            assert!(repository.get_target_set(set_id).await?.is_none());
        }
        (delete, writer) => panic!("unexpected outcomes: delete={delete:?}, writer={writer:?}"),
    }

    workspace.cleanup(&setup).await?;
    repository.delete_target_set(set_id).await?;
    Ok(())
}
