//! DB-backed regression tests for FK-blocked deletes outside the inventory
//! context.
//!
//! These paths used to fall through the generic FK classifier and answer a
//! blocked delete with 400 `Referenced resource does not exist or cannot be
//! used here` — the record exists and is in use, so both halves of that
//! message were wrong. They now route through the shared delete guard and
//! must answer [`AppError::DeleteBlocked`] naming the blocking rows.

mod common;
#[path = "common/project_workspace.rs"]
mod project_workspace;

use piping_backend::contexts::connections::domain::ProxyNode;
use piping_backend::contexts::connections::infrastructure::ConnectionRepository;
use piping_backend::contexts::connections::infrastructure::persistence::repositories::sqlx_connection_repository::SqlxConnectionRepository;
use piping_backend::contexts::connection_config::infrastructure::persistence::record_repository::{
    ConnConfRecordRepository, SqlxConnConfRecordRepository,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::repositories::playbook_repository::{
    PlaybookRepository, SqlxPlaybookRepository,
};
use piping_backend::contexts::playbooks::domain::{InventorySelection, PlaybookScheduleConcurrencyPolicy, PlaybookScheduleMisfirePolicy, ScheduleTargets};
use piping_backend::contexts::playbooks::infrastructure::persistence::CreateScheduleRecord;
use piping_backend::errors::AppError;
use sqlx::PgPool;
use uuid::Uuid;

async fn migrated_pool() -> Result<PgPool, Box<dyn std::error::Error + Send + Sync>> {
    std::env::var("PIPING_DATABASE__URL").map_err(
        |_| "PIPING_DATABASE__URL must be set for ignored delete-blocked integration tests",
    )?;
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

fn native_inventory() -> InventorySelection {
    InventorySelection::Native {
        sources: vec!["inventory.yml".into()],
        ssh_record_id: None,
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn template_delete_blocked_by_run_history_preserves_history_and_revision()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let group_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    let spec = serde_json::to_value(workspace.launch_spec(vec![]))?;
    sqlx::query("INSERT INTO playbook_run_group (id, template_id, launch_spec, trigger_kind, status, total_runs) VALUES ($1, $2, $3, 'manual', 'running', 1)")
        .bind(group_id).bind(workspace.template.id).bind(&spec).execute(&pool).await?;
    sqlx::query("INSERT INTO playbook_run (id, run_group_id, hostname, status) VALUES ($1, $2, 'test-host', 'running')")
        .bind(run_id).bind(group_id).execute(&pool).await?;
    let error = repository
        .delete_job_template(workspace.template.id)
        .await
        .expect_err("run group must block deletion");
    let AppError::DeleteBlocked(details) = &error else {
        panic!("expected DeleteBlocked, got {error:?}");
    };
    assert_eq!(details.total_count, 1, "{details:?}");
    let blocker = details
        .blockers
        .iter()
        .find(|item| item.table == "playbook_run_group")
        .expect("run group blocker");
    assert_eq!(blocker.count, 1);
    let saved_spec: serde_json::Value =
        sqlx::query_scalar("SELECT launch_spec FROM playbook_run_group WHERE id = $1")
            .bind(group_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved_spec, spec);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM playbook_run WHERE id = $1)")
            .bind(run_id)
            .fetch_one(&pool)
            .await?
    );
    assert!(
        repository
            .get_job_template(workspace.template.id)
            .await?
            .is_some()
    );
    assert!(
        repository
            .get_project_revision(workspace.project.id, workspace.revision.revision_id)
            .await?
            .is_some()
    );
    sqlx::query("DELETE FROM playbook_run WHERE id = $1")
        .bind(run_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(&pool)
        .await?;
    assert!(
        repository
            .delete_job_template(workspace.template.id)
            .await?
    );
    assert!(
        !repository
            .delete_job_template(workspace.template.id)
            .await?
    );
    workspace.cleanup(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn template_delete_blocked_by_schedule_and_absent_is_false()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let workspace = project_workspace::seed_workspace(&pool, native_inventory()).await?;
    let suffix = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(user_id).bind(format!("schedule-blocker-{suffix}")).bind(format!("schedule-blocker-{suffix}@test.local")).execute(&pool).await?;
        let schedule = repository
        .create_schedule(CreateScheduleRecord {
            overrides: Default::default(),
            template_id: workspace.template.id,
            targets: ScheduleTargets::Fixed {
                targets: vec![],
                target_set_id: None,
            },
            owner_user_id: user_id,
            name: format!("schedule-blocker-{suffix}"),
            cron_expression: "0 0 * * *".into(),
            timezone: "UTC".into(),
            misfire_policy: PlaybookScheduleMisfirePolicy::FireOnce,
            concurrency_policy: PlaybookScheduleConcurrencyPolicy::SkipIfRunning,
            enabled: false,
            next_run_at: None,
            metadata: serde_json::json!({}),
            user_id: None,
        })
        .await?;
    let error = repository
        .delete_job_template(workspace.template.id)
        .await
        .expect_err("schedule must block deletion");
    let AppError::DeleteBlocked(details) = &error else {
        panic!("expected DeleteBlocked, got {error:?}");
    };
    assert_eq!(details.total_count, 1, "{details:?}");
    let blocker = details
        .blockers
        .iter()
        .find(|item| item.table == "playbook_schedule")
        .expect("schedule blocker");
    assert_eq!(blocker.count, 1);
    assert!(
        repository
            .get_job_template(workspace.template.id)
            .await?
            .is_some()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM playbook_schedule WHERE id = $1)"
        )
        .bind(schedule.id)
        .fetch_one(&pool)
        .await?
    );
    assert!(
        repository
            .get_project_revision(workspace.project.id, workspace.revision.revision_id)
            .await?
            .is_some()
    );
    assert!(repository.delete_schedule(schedule.id).await?);
    assert!(
        repository
            .delete_job_template(workspace.template.id)
            .await?
    );
    assert!(
        !repository
            .delete_job_template(workspace.template.id)
            .await?
    );
    workspace.cleanup(&pool).await?;
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (set PIPING_DATABASE__URL explicitly)"]
async fn connconf_record_delete_blocked_by_child_reports_blocker_and_absent_is_false()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxConnConfRecordRepository::new(pool.clone());
    let run = Uuid::now_v7();
    let suffix = &run.simple().to_string()[..12];
    let base_id = Uuid::now_v7();
    let child_id = Uuid::now_v7();
    let child_name = format!("Delete blocker child {suffix}");

    sqlx::query("INSERT INTO connconf_record (id, name, protocol) VALUES ($1, $2, 'ssh')")
        .bind(base_id)
        .bind(format!("Delete blocker base {suffix}"))
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO connconf_record (id, name, protocol, base_record_id) \
         VALUES ($1, $2, 'ssh', $3)",
    )
    .bind(child_id)
    .bind(&child_name)
    .bind(base_id)
    .execute(&pool)
    .await?;

    let error = repository
        .delete(base_id)
        .await
        .expect_err("a derived record must block base-record deletion");
    let AppError::DeleteBlocked(details) = &error else {
        panic!("expected DeleteBlocked, got {error:?}");
    };
    assert_eq!(details.total_count, 1, "blocker details: {details:?}");
    let blocker = details
        .blockers
        .iter()
        .find(|group| group.table == "connconf_record")
        .unwrap_or_else(|| panic!("missing connconf_record blocker: {details:?}"));
    assert_eq!(
        blocker.entity_key.as_deref(),
        Some("Connection Config Records")
    );
    assert_eq!(blocker.examples.len(), 1);
    assert_eq!(blocker.examples[0].label, child_name);

    assert!(repository.delete(child_id).await?);
    assert!(repository.delete(base_id).await?);
    assert!(!repository.delete(base_id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (set PIPING_DATABASE__URL explicitly)"]
async fn choice_set_delete_blocked_by_custom_field_reports_blockers()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let run = Uuid::now_v7();
    let choice_set_id = Uuid::now_v7();
    let field_id = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO custom_field_choice_set (id, name, description, extra_choices, order_alphabetically) \
         VALUES ($1, $2, '', '[]'::jsonb, false)",
    )
    .bind(choice_set_id)
    .bind(format!("del-blk-{}", &run.simple().to_string()[..12]))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO custom_field (id, name, label, field_type, choice_set_id) \
         VALUES ($1, $2, $2, 'select', $3)",
    )
    .bind(field_id)
    .bind(format!("del_blk_{}", &run.simple().to_string()[..12]))
    .bind(choice_set_id)
    .execute(&pool)
    .await?;

    let error = piping_backend::contexts::shared::delete_guard::delete_record_guarded(
        &pool,
        "custom_field_choice_set",
        choice_set_id,
        piping_backend::contexts::shared::delete_guard::no_blocker_metadata,
    )
    .await
    .expect_err("delete of a referenced choice set must fail");

    match &error {
        AppError::DeleteBlocked(details) => {
            assert!(details.total_count >= 1, "blocker count: {details:?}");
            assert!(
                details
                    .blockers
                    .iter()
                    .any(|group| group.table == "custom_field"),
                "custom_field must be reported as the blocker: {details:?}"
            );
        }
        other => panic!("expected DeleteBlocked, got {other:?}"),
    }

    // Cleanup: field first, then the set.
    sqlx::query("DELETE FROM custom_field WHERE id = $1")
        .bind(field_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM custom_field_choice_set WHERE id = $1")
        .bind(choice_set_id)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (set PIPING_DATABASE__URL explicitly)"]
async fn proxy_node_delete_blocked_by_assignment_and_absent_node_is_false()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxConnectionRepository::new(pool.clone());
    let run = Uuid::now_v7();
    let node_id = Uuid::now_v7();

    let node = ProxyNode {
        id: node_id,
        name: format!("del-blk-node-{}", &run.simple().to_string()[..12]),
        slug: format!("del-blk-node-{}", &run.simple().to_string()[..12]),
        base_url: "https://proxy.invalid:8443".to_string(),
        enabled: true,
        secret_version: 1,
        secret_rotated_at: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    repository.create_proxy_node(&node).await?;
    sqlx::query(
        "INSERT INTO connections_proxy_assignment (proxy_node_id, scope_type, scope_id, weight, enabled) \
         VALUES ($1, 'global', NULL, 100, true)",
    )
    .bind(node_id)
    .execute(&pool)
    .await?;

    let error = repository
        .delete_proxy_node(node_id)
        .await
        .expect_err("delete of an assigned proxy node must fail");
    match &error {
        AppError::DeleteBlocked(details) => {
            assert!(
                details
                    .blockers
                    .iter()
                    .any(|group| group.table == "connections_proxy_assignment"),
                "assignments must be reported as blockers: {details:?}"
            );
            // Sessions reference proxy nodes with ON DELETE SET NULL; they
            // resolve themselves and must never be listed as blockers.
            assert!(
                details
                    .blockers
                    .iter()
                    .all(|group| group.table != "connections_session"),
                "SET NULL references must not be counted as blockers: {details:?}"
            );
        }
        other => panic!("expected DeleteBlocked, got {other:?}"),
    }

    // Remove the assignment; the delete must now succeed and report true.
    sqlx::query("DELETE FROM connections_proxy_assignment WHERE proxy_node_id = $1")
        .bind(node_id)
        .execute(&pool)
        .await?;
    assert!(repository.delete_proxy_node(node_id).await?);
    // Deleting a node that is already gone must report false (404 material),
    // not silently claim success.
    assert!(!repository.delete_proxy_node(node_id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (set PIPING_DATABASE__URL explicitly)"]
async fn rack_delete_of_absent_row_is_not_found()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use piping_backend::contexts::inventory::infrastructure::persistence::repositories::dcim::rack_repository::{
        RackRepository, SqlxRackRepository,
    };
    let pool = migrated_pool().await?;
    let repository = SqlxRackRepository::new(pool);
    // The rack DELETE contract advertises 404: deleting a row that does not
    // exist must be NotFound, not a silent 204-material Ok.
    let error = repository
        .delete(Uuid::now_v7())
        .await
        .expect_err("deleting an absent rack must fail");
    assert!(
        matches!(&error, AppError::NotFound(_)),
        "expected NotFound, got {error:?}"
    );
    Ok(())
}
