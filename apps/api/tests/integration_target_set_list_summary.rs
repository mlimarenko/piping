//! DB-backed tests for the target-set list summary.
//!
//! The list must answer "how many hosts" and "what uses this set" in the page
//! request itself. These tests pin the three ownership references (job
//! template, assignment, schedule), the equality between a list count and the
//! preview total, and the difference between a known zero and an unknown
//! count.

mod common;
#[path = "common/project_workspace.rs"]
mod project_workspace;

use piping_backend::contexts::playbooks::domain::{
    InventorySelection, PlaybookAssignmentScopeKind, PlaybookScheduleConcurrencyPolicy,
    PlaybookScheduleMisfirePolicy, PlaybookTargetSetMode, ScheduleTargets,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateAssignmentRecord, CreateScheduleRecord,
    CreateTargetSetRecord, PlaybookRepository, PlaybookTargetSetListFilter, SqlxPlaybookRepository,
    StaticTargetMemberRecord,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn native_inventory() -> InventorySelection {
    InventorySelection::Native {
        sources: vec!["inventory.yml".into()],
        ssh_record_id: None,
    }
}

async fn migrated_pool() -> TestResult<PgPool> {
    std::env::var("PIPING_DATABASE__URL")
        .map_err(|_| "PIPING_DATABASE__URL must be set for ignored target-set list tests")?;
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|error| format!("run migrations: {error}"))?;
    Ok(pool)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn list_target_sets_reports_counts_and_usage_in_one_page() -> TestResult {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let suffix = Uuid::now_v7();

    let template_set = repository
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("list-summary-template-{suffix}"),
                mode: PlaybookTargetSetMode::Static,
                selector: json!({}),
                members: vec![],
                user_id: None,
            },
            50,
        )
        .await?
        .target_set;
    let assignment_set = repository
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("list-summary-assignment-{suffix}"),
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
            50,
        )
        .await?
        .target_set;
    // A stored selector the predicate builder cannot read makes the count
    // uncomputable; the list must report that as unknown, not as zero. The set
    // is inserted directly because a normal create would refuse to preview it.
    let schedule_set_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_target_set (id, name, mode, entity_kind, selector) \
         VALUES ($1, $2, 'dynamic', 'connection_target', $3)",
    )
    .bind(schedule_set_id)
    .bind(format!("list-summary-schedule-{suffix}"))
    .bind(json!({ "site_ids": "not-an-array" }))
    .execute(&pool)
    .await?;

    // Usage by managed job template.
    sqlx::query("UPDATE playbook_job_template SET inventory = $2 WHERE id = $1")
        .bind(workspace.template.id)
        .bind(serde_json::to_value(InventorySelection::Managed {
            target_set_id: template_set.id,
        })?)
        .execute(&pool)
        .await?;

    // Usage by assignment.
    let assignment_name = format!("list-summary-assignment-{suffix}");
    let assignment = repository
        .create_assignment(CreateAssignmentRecord {
            job_template_id: workspace.template.id,
            target_set_id: assignment_set.id,
            name: Some(assignment_name.clone()),
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

    // Usage by schedule target selection.
    let user_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source) \
         VALUES ($1, $2, $3, 'active', 'local')",
    )
    .bind(user_id)
    .bind(format!("list-summary-{suffix}"))
    .bind(format!("list-summary-{suffix}@test.local"))
    .execute(&pool)
    .await?;
    let schedule = repository
        .create_schedule(CreateScheduleRecord {
            overrides: Default::default(),
            template_id: workspace.template.id,
            targets: ScheduleTargets::Dynamic {
                selector: json!({}),
                target_set_id: Some(schedule_set_id),
            },
            owner_user_id: user_id,
            name: format!("list-summary-schedule-{suffix}"),
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

    let (items, total) = repository
        .list_target_sets(&PlaybookTargetSetListFilter::default(), 500, 0)
        .await?;
    assert!(
        total >= 3,
        "list must report the matched total, got {total}"
    );
    let item = |id: Uuid| {
        items
            .iter()
            .find(|item| item.target_set.id == id)
            .unwrap_or_else(|| panic!("target set {id} missing from the list page"))
    };

    let template_item = item(template_set.id);
    assert_eq!(template_item.usage.job_templates.len(), 1);
    assert_eq!(
        template_item.usage.job_templates[0].id,
        workspace.template.id
    );
    assert!(template_item.usage.assignments.is_empty());
    assert!(template_item.usage.schedules.is_empty());
    assert_eq!(template_item.resolved_count, Some(0));

    let assignment_item = item(assignment_set.id);
    assert!(assignment_item.usage.job_templates.is_empty());
    assert_eq!(assignment_item.usage.assignments.len(), 1);
    assert_eq!(assignment_item.usage.assignments[0].id, assignment.id);
    assert_eq!(
        assignment_item.usage.assignments[0].name.as_deref(),
        Some(assignment_name.as_str())
    );
    assert!(assignment_item.usage.schedules.is_empty());
    assert_eq!(assignment_item.resolved_count, Some(0));

    let schedule_item = item(schedule_set_id);
    assert!(schedule_item.usage.job_templates.is_empty());
    assert!(schedule_item.usage.assignments.is_empty());
    assert_eq!(schedule_item.usage.schedules.len(), 1);
    assert_eq!(schedule_item.usage.schedules[0].id, schedule.id);
    assert_eq!(schedule_item.resolved_count, None);

    // A list count is the preview total, not a separate estimate.
    let preview = repository
        .preview_target_set(template_set.id, 50)
        .await?
        .expect("template set preview");
    assert_eq!(Some(preview.total), template_item.resolved_count);

    repository.delete_schedule(schedule.id).await?;
    repository.delete_assignment(assignment.id).await?;
    repository.delete_target_set(template_set.id).await?;
    repository.delete_target_set(assignment_set.id).await?;
    repository.delete_target_set(schedule_set_id).await?;
    workspace.cleanup(&pool).await?;
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn list_target_sets_searches_names_beyond_the_first_page() -> TestResult {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let suffix = Uuid::now_v7();
    let needle = format!("needle-{suffix}");
    let mut ids = Vec::with_capacity(121);
    for index in 0..120 {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO playbook_target_set (id, name, mode, entity_kind, selector) \
             VALUES ($1, $2, 'dynamic', 'connection_target', '{}'::jsonb)",
        )
        .bind(id)
        .bind(format!("bulk-{suffix}-{index:03}"))
        .execute(&pool)
        .await?;
        ids.push(id);
    }
    let needle_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_target_set (id, name, mode, entity_kind, selector) \
         VALUES ($1, $2, 'dynamic', 'connection_target', '{}'::jsonb)",
    )
    .bind(needle_id)
    .bind(&needle)
    .execute(&pool)
    .await?;
    ids.push(needle_id);

    let (items, total) = repository
        .list_target_sets(
            &PlaybookTargetSetListFilter {
                search: Some(needle.clone()),
                ..PlaybookTargetSetListFilter::default()
            },
            100,
            0,
        )
        .await?;
    assert_eq!(
        total, 1,
        "search must count every matching row, not the page"
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].target_set.id, needle_id);

    // The name search composes with the mode filter instead of replacing it.
    let (_, wrong_mode_total) = repository
        .list_target_sets(
            &PlaybookTargetSetListFilter {
                search: Some(needle.clone()),
                mode: vec![PlaybookTargetSetMode::Static],
                ..PlaybookTargetSetListFilter::default()
            },
            100,
            0,
        )
        .await?;
    assert_eq!(wrong_mode_total, 0);

    sqlx::query("DELETE FROM playbook_target_set WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&pool)
        .await?;
    Ok(())
}
