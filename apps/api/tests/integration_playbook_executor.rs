//! PostgreSQL integration tests for executor timeouts, assignments and schedules.

mod common;
#[path = "common/project_workspace.rs"]
mod project_workspace;

use std::sync::Arc;

use chrono::{Duration, Utc};
use piping_backend::contexts::playbooks::application::{
    CreateAssignmentInput, PlaybookService, UpdateAssignmentInput,
};
use piping_backend::contexts::playbooks::domain::{
    InventorySelection, PlaybookAssignmentScopeKind, PlaybookRunStatus,
    PlaybookScheduleConcurrencyPolicy, PlaybookScheduleMisfirePolicy,
    PlaybookTargetSetMode, ProjectEntry, ProjectEntryContent,
    RevisionChange, ScheduleTargets,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateScheduleRecord, CreateTargetSetRecord, PlaybookRepository, SqlxPlaybookRepository,
    StaticTargetMemberRecord, TimeoutStuckRunsRecord, UpdateScheduleRecord, UpdateTargetSetRecord,
};
use piping_backend::errors::AppError;
use serde_json::json;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn native_inventory() -> InventorySelection {
    InventorySelection::Native {
        sources: vec!["inventory.yml".into()],
        ssh_record_id: None,
    }
}

async fn seed_static_target_set(
    repo: &SqlxPlaybookRepository,
    suffix: &str,
    name: &str,
) -> TestResult<Uuid> {
    let preview = repo
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("{name}-{suffix}"),
                mode: PlaybookTargetSetMode::Static,
                selector: json!({}),
                members: vec![StaticTargetMemberRecord {
                    target_kind: "connection_target".into(),
                    target_id: Uuid::now_v7(),
                    source_kind: "device".into(),
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

async fn seed_running_run(
    pool: &sqlx::PgPool,
    workspace: &project_workspace::WorkspaceFixture,
    started_at: chrono::DateTime<Utc>,
    timeout_seconds: i32,
) -> TestResult<(Uuid, Uuid)> {
    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO playbook_run_group
        (id, template_id, launch_spec, trigger_kind, status, total_runs, started_at)
        VALUES ($1, $2, $3, 'manual', 'running', 1, $4)"#,
    )
    .bind(group_id)
    .bind(workspace.template.id)
    .bind(serde_json::to_value(workspace.launch_spec(vec![]))?)
    .bind(started_at)
    .execute(pool)
    .await?;
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO playbook_run
        (id, run_group_id, hostname, status, started_at, timeout_seconds, lease_owner, leased_at)
        VALUES ($1, $2, 'executor-test-host', 'running', $3, $4, 'worker-test', $3)"#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(started_at)
    .bind(timeout_seconds)
    .execute(pool)
    .await?;
    Ok((group_id, run_id))
}

async fn delete_group(pool: &sqlx::PgPool, group_id: Uuid) -> TestResult {
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn timeout_stuck_runs_fails_overdue_run_and_rolls_up_group() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let (group_id, run_id) =
        seed_running_run(&pool, &workspace, Utc::now() - Duration::seconds(120), 5).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let result = repo
        .timeout_stuck_runs(TimeoutStuckRunsRecord {
            now: Utc::now(),
            limit: 50,
        })
        .await?;
    let run = result
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .expect("overdue run reaped");
    assert_eq!(run.status, PlaybookRunStatus::TimedOut);
    assert!(run.finished_at.is_some());
    let group = result
        .run_groups
        .iter()
        .find(|group| group.id == group_id)
        .expect("group rolled up");
    assert_eq!(group.timeout_runs, 1);
    assert_eq!(group.status, PlaybookRunStatus::TimedOut);
    assert!(group.finished_at.is_some());
    assert!(group.lease_owner.is_none());
    let second = repo
        .timeout_stuck_runs(TimeoutStuckRunsRecord {
            now: Utc::now(),
            limit: 50,
        })
        .await?;
    assert!(
        second.runs.iter().all(|run| run.id != run_id),
        "terminal run must not be reaped twice"
    );
    delete_group(&pool, group_id).await?;
    workspace.cleanup(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn timeout_stuck_runs_leaves_within_timeout_runs_alone() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let (group_id, run_id) = seed_running_run(&pool, &workspace, Utc::now(), 1800).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let result = repo
        .timeout_stuck_runs(TimeoutStuckRunsRecord {
            now: Utc::now(),
            limit: 50,
        })
        .await?;
    assert!(result.runs.iter().all(|run| run.id != run_id));
    let status: String = sqlx::query_scalar("SELECT status FROM playbook_run WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(status, "running");
    delete_group(&pool, group_id).await?;
    workspace.cleanup(&pool).await?;
    Ok(())
}

fn assignment_update() -> UpdateAssignmentInput {
    UpdateAssignmentInput {
        job_template_id: None,
        target_set_id: None,
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
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn assignment_update_and_delete_round_trip() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = Arc::new(SqlxPlaybookRepository::new(pool.clone()));
    let service = PlaybookService::new(repo.clone());
    let suffix = common::unique_suffix();
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let other = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let target_set_id = seed_static_target_set(&repo, &suffix, "assignment-set").await?;
    let other_target_set_id =
        seed_static_target_set(&repo, &suffix, "assignment-set-other").await?;
    let created = service
        .create_assignment(
            CreateAssignmentInput {
                job_template_id: workspace.template.id,
                target_set_id,
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
                name: Some(None),
                priority: Some(25),
                enabled: Some(false),
                quick_action_enabled: Some(true),
                default_input: Some(json!({})),
                default_timeout_seconds: Some(None),
                metadata: Some(json!({"owner": "updated"})),
                ..assignment_update()
            },
            None,
        )
        .await?
        .expect("assignment updates");
    assert_eq!(patched.name, None);
    assert_eq!(patched.priority, 25);
    assert!(!patched.enabled);
    assert!(patched.quick_action_enabled);
    assert_eq!(patched.job_template_id, workspace.template.id);
    assert_eq!(patched.default_timeout_seconds, None);
    let invalid_scope = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                scope_kind: Some(PlaybookAssignmentScopeKind::Global),
                scope_id: Some(Some(Uuid::now_v7())),
                ..assignment_update()
            },
            None,
        )
        .await;
    assert!(
        matches!(invalid_scope, Err(AppError::Validation(message)) if message.contains("must not set scope_id"))
    );
    let retargeted = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                job_template_id: Some(other.template.id),
                target_set_id: Some(other_target_set_id),
                name: Some(Some(format!("assignment-retargeted-{suffix}"))),
                enabled: Some(true),
                ..assignment_update()
            },
            None,
        )
        .await?
        .expect("assignment retargets");
    assert_eq!(retargeted.job_template_id, other.template.id);
    assert_eq!(retargeted.target_set_id, other_target_set_id);
    let missing_target = service
        .update_assignment(
            created.id,
            UpdateAssignmentInput {
                target_set_id: Some(Uuid::now_v7()),
                ..assignment_update()
            },
            None,
        )
        .await;
    assert!(
        matches!(missing_target, Err(AppError::NotFound(message)) if message.contains("target set"))
    );
    assert!(service.delete_assignment(created.id).await?);
    assert!(service.get_assignment(created.id).await?.is_none());
    for id in [target_set_id, other_target_set_id] {
        sqlx::query("DELETE FROM playbook_target_set WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await?;
    }
    workspace.cleanup(&pool).await?;
    other.cleanup(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn schedule_update_toggles_enabled_and_updates_configuration() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let suffix = common::unique_suffix();
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(user_id).bind(format!("schedule-{suffix}"))
        .bind(format!("schedule-{suffix}@test.local")).execute(&pool).await?;
    let schedule = repo
        .create_schedule(CreateScheduleRecord {
            overrides: Default::default(),
            template_id: workspace.template.id,
            targets: ScheduleTargets::Fixed {
                targets: vec![],
                target_set_id: None,
            },
            owner_user_id: user_id,
            name: format!("nightly-{suffix}"),
            cron_expression: "0 0 * * *".into(),
            timezone: "UTC".into(),
            misfire_policy: PlaybookScheduleMisfirePolicy::FireOnce,
            concurrency_policy: PlaybookScheduleConcurrencyPolicy::SkipIfRunning,
            enabled: true,
            next_run_at: Some(Utc::now() + Duration::hours(1)),
            metadata: json!({}),
            user_id: None,
        })
        .await?;
    assert!(schedule.enabled);
    let disabled = repo
        .update_schedule(UpdateScheduleRecord {
            overrides: None,
            id: schedule.id,
            targets: None,
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
        .expect("disable schedule");
    assert!(!disabled.enabled);
    let re_enabled = repo
        .update_schedule(UpdateScheduleRecord {
            overrides: None,
            id: schedule.id,
            targets: None,
            name: None,
            cron_expression: Some("*/15 * * * *".into()),
            timezone: Some("Europe/Berlin".into()),
            misfire_policy: None,
            concurrency_policy: Some(PlaybookScheduleConcurrencyPolicy::QueuePending),
            enabled: Some(true),
            next_run_at: Some(Utc::now() + Duration::hours(2)),
            metadata: Some(json!({"note": "re-enabled"})),
            user_id: None,
        })
        .await?
        .expect("re-enable schedule");
    assert!(re_enabled.enabled);
    assert_eq!(re_enabled.cron_expression, "*/15 * * * *");
    assert_eq!(re_enabled.timezone, "Europe/Berlin");
    assert_eq!(
        re_enabled.concurrency_policy,
        PlaybookScheduleConcurrencyPolicy::QueuePending
    );
    assert!(re_enabled.next_run_at.is_some());
    assert!(repo.delete_schedule(schedule.id).await?);
    assert!(repo.get_schedule(schedule.id).await?.is_none());
    assert!(!repo.delete_schedule(schedule.id).await?);
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await?;
    workspace.cleanup(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn project_revision_files_round_trip_and_escape_rejection() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let service = PlaybookService::new(Arc::new(SqlxPlaybookRepository::new(pool.clone())));
    let entry = ProjectEntry {
        path: "roles/common/tasks/main.yml".into(),
        content: ProjectEntryContent::File {
            bytes: b"---\n[]\n".to_vec(),
            executable: true,
        },
    };
    let revision = service
        .change_project_revision(
            workspace.project.id,
            workspace.revision.revision_id,
            vec![RevisionChange::Put {
                entry: entry.clone(),
            }],
            None,
        )
        .await?;
    let read_back = service
        .get_project_revision(workspace.project.id, revision.revision_id)
        .await?
        .expect("revision exists");
    assert!(read_back.entries.contains(&entry));
    let count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM playbook_project_revision WHERE project_id = $1")
            .bind(workspace.project.id)
            .fetch_one(&pool)
            .await?;
    let rejected = service
        .change_project_revision(
            workspace.project.id,
            revision.revision_id,
            vec![RevisionChange::Put {
                entry: ProjectEntry {
                    path: "../controller/ssh.key".into(),
                    content: ProjectEntryContent::File {
                        bytes: b"key-material".to_vec(),
                        executable: false,
                    },
                },
            }],
            None,
        )
        .await;
    let current_revision_id = service
        .get_project(workspace.project.id)
        .await?
        .expect("project exists")
        .current_revision_id;
    let count_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM playbook_project_revision WHERE project_id = $1")
            .bind(workspace.project.id)
            .fetch_one(&pool)
            .await?;
    let unchanged = service
        .get_project_revision(workspace.project.id, revision.revision_id)
        .await?
        .expect("revision exists");
    workspace.cleanup(&pool).await?;
    assert!(
        matches!(rejected, Err(AppError::Validation(_))),
        "path escaping the project must be rejected"
    );
    assert_eq!(current_revision_id, Some(revision.revision_id));
    assert_eq!(count_after, count_before);
    assert_eq!(unchanged.tree_sha256, read_back.tree_sha256);
    assert_eq!(unchanged.entries, read_back.entries);
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn target_set_update_replaces_members_and_rejects_empty_static() -> TestResult {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let suffix = Uuid::now_v7().to_string();
    let first = StaticTargetMemberRecord {
        target_kind: "connection_target".into(),
        target_id: Uuid::now_v7(),
        source_kind: "device".into(),
        source_id: Uuid::now_v7(),
        sort_order: 0,
    };
    let second = StaticTargetMemberRecord {
        target_kind: "connection_target".into(),
        target_id: Uuid::now_v7(),
        source_kind: "virtual_machine".into(),
        source_id: Uuid::now_v7(),
        sort_order: 1,
    };
    let created = repo
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("member-update-{suffix}"),
                mode: PlaybookTargetSetMode::Static,
                selector: json!({}),
                members: vec![first.clone()],
                user_id: None,
            },
            100,
        )
        .await?;
    let id = created.target_set.id;

    let updated = repo
        .update_target_set(UpdateTargetSetRecord {
            id,
            name: format!("member-update-{suffix}-renamed"),
            mode: PlaybookTargetSetMode::Static,
            entity_kind: "connection_target".into(),
            selector: json!({}),
            members: Some(vec![second.clone()]),
            user_id: None,
        })
        .await?
        .expect("target set updated");
    assert_eq!(updated.mode, PlaybookTargetSetMode::Static);
    let members = repo.list_target_set_members(id).await?;
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].target_id, second.target_id);

    let rejected = repo
        .update_target_set(UpdateTargetSetRecord {
            id,
            name: format!("member-update-{suffix}-empty"),
            mode: PlaybookTargetSetMode::Static,
            entity_kind: "connection_target".into(),
            selector: json!({}),
            members: Some(vec![]),
            user_id: None,
        })
        .await;
    assert!(matches!(rejected, Err(AppError::Validation(_))));
    assert_eq!(repo.list_target_set_members(id).await?.len(), 1);

    let dynamic = repo
        .update_target_set(UpdateTargetSetRecord {
            id,
            name: format!("member-update-{suffix}-dynamic"),
            mode: PlaybookTargetSetMode::Dynamic,
            entity_kind: "connection_target".into(),
            selector: json!({ "status": ["active"] }),
            members: None,
            user_id: None,
        })
        .await?
        .expect("target set switched to dynamic");
    assert_eq!(dynamic.mode, PlaybookTargetSetMode::Dynamic);
    assert_eq!(
        repo.list_target_set_members(id).await?.len(),
        1,
        "omitted members must be preserved across a mode switch"
    );

    repo.delete_target_set(id).await?;
    Ok(())
}
