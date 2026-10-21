//! DB-backed regression tests for FK-blocked deletes outside the inventory
//! context.
//!
//! These paths used to fall through the generic FK classifier and answer a
//! blocked delete with 400 `Referenced resource does not exist or cannot be
//! used here` — the record exists and is in use, so both halves of that
//! message were wrong. They now route through the shared delete guard and
//! must answer [`AppError::DeleteBlocked`] naming the blocking rows.

mod common;

use piping_backend::contexts::connections::domain::ProxyNode;
use piping_backend::contexts::connections::infrastructure::ConnectionRepository;
use piping_backend::contexts::connections::infrastructure::persistence::repositories::sqlx_connection_repository::SqlxConnectionRepository;
use piping_backend::contexts::connection_config::infrastructure::persistence::record_repository::{
    ConnConfRecordRepository, SqlxConnConfRecordRepository,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::repositories::playbook_repository::{
    PlaybookRepository, SqlxPlaybookRepository,
};
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

#[tokio::test]
#[ignore = "requires database (set PIPING_DATABASE__URL explicitly)"]
async fn playbook_delete_blocked_by_run_history_reports_both_blockers_and_absent_is_false()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let run = Uuid::now_v7();
    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let run_group_id = Uuid::now_v7();
    let playbook_run_id = Uuid::now_v7();
    let suffix = &run.simple().to_string()[..12];
    let playbook_name = format!("Delete blocker playbook {suffix}");

    sqlx::query("INSERT INTO playbook (id, slug, name) VALUES ($1, $2, $3)")
        .bind(playbook_id)
        .bind(format!("delete-blocker-{suffix}"))
        .bind(&playbook_name)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO playbook_version \
         (id, playbook_id, version, content, content_sha256) \
         VALUES ($1, $2, 1, '---', $3)",
    )
    .bind(version_id)
    .bind(playbook_id)
    .bind("0".repeat(64))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO playbook_run_group \
         (id, playbook_id, playbook_version_id, trigger_kind) \
         VALUES ($1, $2, $3, 'manual')",
    )
    .bind(run_group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO playbook_run \
         (id, run_group_id, playbook_id, playbook_version_id, target_kind, target_id, \
          source_kind, source_id) \
         VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $6)",
    )
    .bind(playbook_run_id)
    .bind(run_group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await?;

    let error = repository
        .delete_playbook(playbook_id)
        .await
        .expect_err("run history must block hard deletion");
    let AppError::DeleteBlocked(details) = &error else {
        panic!("expected DeleteBlocked, got {error:?}");
    };
    assert_eq!(details.total_count, 2, "blocker details: {details:?}");
    for (table, entity_key) in [
        ("playbook_run_group", "Playbook Run Groups"),
        ("playbook_run", "Playbook Runs"),
    ] {
        let group = details
            .blockers
            .iter()
            .find(|group| group.table == table)
            .unwrap_or_else(|| panic!("missing {table} blocker: {details:?}"));
        assert_eq!(group.count, 1);
        assert_eq!(group.entity_key.as_deref(), Some(entity_key));
        assert!(
            group
                .examples
                .iter()
                .any(|example| example.label.ends_with(&format!(" @ {playbook_name}"))),
            "{table} must have a playbook-labelled example: {group:?}"
        );
    }
    let version_still_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM playbook_version WHERE id = $1)")
            .bind(version_id)
            .fetch_one(&pool)
            .await?;
    assert!(
        version_still_exists,
        "blocked delete must roll back the version cascade"
    );

    sqlx::query("DELETE FROM playbook_run WHERE id = $1")
        .bind(playbook_run_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(run_group_id)
        .execute(&pool)
        .await?;
    assert!(repository.delete_playbook(playbook_id).await?);
    assert!(!repository.delete_playbook(playbook_id).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (set PIPING_DATABASE__URL explicitly)"]
async fn target_set_delete_blocked_by_schedule_reports_blocker_and_absent_is_false()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxPlaybookRepository::new(pool.clone());
    let run = Uuid::now_v7();
    let suffix = &run.simple().to_string()[..12];
    let playbook_id = Uuid::now_v7();
    let target_set_id = Uuid::now_v7();
    let principal_id = Uuid::now_v7();
    let schedule_id = Uuid::now_v7();
    let schedule_name = format!("Delete blocker schedule {suffix}");

    sqlx::query("INSERT INTO playbook (id, slug, name) VALUES ($1, $2, $3)")
        .bind(playbook_id)
        .bind(format!("schedule-blocker-{suffix}"))
        .bind(format!("Schedule blocker playbook {suffix}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO playbook_target_set (id, name, mode) VALUES ($1, $2, 'dynamic')")
        .bind(target_set_id)
        .bind(format!("Delete blocker target set {suffix}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO playbook_service_principal (id, name, slug) VALUES ($1, $2, $3)")
        .bind(principal_id)
        .bind(format!("Delete blocker principal {suffix}"))
        .bind(format!("delete-blocker-{suffix}"))
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO playbook_schedule \
         (id, playbook_id, target_set_id, service_principal_id, name, cron_expression, enabled) \
         VALUES ($1, $2, $3, $4, $5, '0 * * * * *', false)",
    )
    .bind(schedule_id)
    .bind(playbook_id)
    .bind(target_set_id)
    .bind(principal_id)
    .bind(&schedule_name)
    .execute(&pool)
    .await?;

    let error = repository
        .delete_target_set(target_set_id)
        .await
        .expect_err("a schedule must block target-set deletion");
    let AppError::DeleteBlocked(details) = &error else {
        panic!("expected DeleteBlocked, got {error:?}");
    };
    assert_eq!(details.total_count, 1, "blocker details: {details:?}");
    let blocker = details
        .blockers
        .iter()
        .find(|group| group.table == "playbook_schedule")
        .unwrap_or_else(|| panic!("missing playbook_schedule blocker: {details:?}"));
    assert_eq!(blocker.entity_key.as_deref(), Some("Playbook Schedules"));
    assert_eq!(blocker.examples.len(), 1);
    assert_eq!(blocker.examples[0].label, schedule_name);

    sqlx::query("DELETE FROM playbook_schedule WHERE id = $1")
        .bind(schedule_id)
        .execute(&pool)
        .await?;
    assert!(repository.delete_target_set(target_set_id).await?);
    assert!(!repository.delete_target_set(target_set_id).await?);
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(playbook_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM playbook_service_principal WHERE id = $1")
        .bind(principal_id)
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
