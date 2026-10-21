use axum::http::StatusCode;
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use piping_backend::contexts::inventory::application::FullBackupSelection;
use redis::AsyncCommands;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_redis,
    login_and_get_cookie, request, unique_suffix,
};
use crate::support::{TestError, suite_guard};

const EXPORT_PATH: &str = "/api/v1/admin/backup/export";
const IMPORT_PATH: &str = "/api/v1/admin/backup/import";

fn tenant_selection() -> FullBackupSelection {
    FullBackupSelection {
        sections: Vec::new(),
        tables: vec!["tenancy_tenant".to_string()],
    }
}

async fn add_deny_policy(
    db: &sqlx::PgPool,
    actions: &[Action],
    resources: &[ResourceType],
) -> Result<Uuid, TestError> {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(db)
            .await?;
    let policy_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints,
            subjects, resources, enabled
        )
        VALUES ($1, $2, 'deny', $3, $4, NULL, $5, '[]'::jsonb, true)
        "#,
    )
    .bind(policy_id)
    .bind(format!("backup-route-deny-{}", unique_suffix()))
    .bind(serde_json::to_value(actions)?)
    .bind(serde_json::to_value(resources)?)
    .bind(json!([format!("role:{role_id}")]))
    .execute(db)
    .await?;
    bump_policy_cache().await?;
    Ok(policy_id)
}

async fn remove_deny_policy(db: &sqlx::PgPool, policy_id: Uuid) -> Result<(), TestError> {
    sqlx::query("DELETE FROM rbac_policy WHERE id = $1")
        .bind(policy_id)
        .execute(db)
        .await?;
    bump_policy_cache().await
}

async fn bump_policy_cache() -> Result<(), TestError> {
    let mut redis = build_test_redis().await?;
    let _: i64 = redis.incr("access_control:policies:version", 1).await?;
    Ok(())
}

async fn audit_count(db: &sqlx::PgPool, event_type: &str) -> Result<i64, TestError> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(db)
            .await?,
    )
}

/// Mutation caught: removing either endpoint's base `Backup` access check or
/// accidentally checking READ for import instead of UPDATE.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn backup_routes_enforce_export_and_import_permissions() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let state = build_test_app_state().await?;
    let archive = state
        .services
        .backup
        .export_full_backup(false, Some(tenant_selection()))
        .await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let export_body = json!({
        "includeSecrets": false,
        "selection": { "tables": ["tenancy_tenant"] }
    })
    .to_string();
    let read_deny = add_deny_policy(&db, &[Action::Read], &[ResourceType::Backup]).await?;
    let (export_status, _) =
        request(&app, "POST", EXPORT_PATH, Some(&export_body), Some(&cookie)).await?;
    assert_eq!(export_status, StatusCode::FORBIDDEN);
    remove_deny_policy(&db, read_deny).await?;

    let import_body = json!({
        "payload": archive,
        "confirmFullRestore": true,
        "selection": { "tables": ["tenancy_tenant"] }
    })
    .to_string();
    let update_deny = add_deny_policy(&db, &[Action::Update], &[ResourceType::Backup]).await?;
    let (import_status, _) =
        request(&app, "POST", IMPORT_PATH, Some(&import_body), Some(&cookie)).await?;
    assert_eq!(import_status, StatusCode::FORBIDDEN);

    remove_deny_policy(&db, update_deny).await?;
    Ok(())
}

/// Mutation caught: bypassing the second permission check when an import
/// archive contains secret-bearing tables or columns.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn secret_bearing_import_requires_secret_update_permission() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let state = build_test_app_state().await?;
    let selection = FullBackupSelection {
        sections: Vec::new(),
        tables: vec!["secrets_variable".to_string()],
    };
    let archive = state
        .services
        .backup
        .export_full_backup(true, Some(selection.clone()))
        .await?;
    let db = state.db_pool.clone();
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secrets_variable")
        .fetch_one(&db)
        .await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let policy_id =
        add_deny_policy(&db, &[Action::Update], &[ResourceType::SecretVariable]).await?;

    let body = json!({
        "payload": archive,
        "confirmFullRestore": true,
        "selection": selection
    })
    .to_string();
    let (status, _) = request(&app, "POST", IMPORT_PATH, Some(&body), Some(&cookie)).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secrets_variable")
        .fetch_one(&db)
        .await?;
    assert_eq!(after, before, "forbidden secret import mutated secret rows");

    remove_deny_policy(&db, policy_id).await?;
    Ok(())
}

/// Mutation caught: returning success without recording export/import audit
/// events, or forgetting to bump host-config cache generation after apply.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn successful_backup_routes_audit_and_bump_cache_generation() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let exported_before = audit_count(&db, "admin.backup.exported").await?;
    let restored_before = audit_count(&db, "admin.backup.restored").await?;

    let export_body = json!({
        "includeSecrets": false,
        "selection": { "tables": ["tenancy_tenant"] }
    })
    .to_string();
    let (status, body) =
        request(&app, "POST", EXPORT_PATH, Some(&export_body), Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let archive: Value = serde_json::from_slice(&body)?;
    assert_eq!(
        audit_count(&db, "admin.backup.exported").await?,
        exported_before + 1
    );

    let mut redis = build_test_redis().await?;
    let generation_before = redis
        .get::<_, Option<u64>>("host_config:version")
        .await?
        .unwrap_or(0);
    let import_body = json!({
        "payload": archive,
        "confirmFullRestore": true,
        "selection": { "tables": ["tenancy_tenant"] }
    })
    .to_string();
    let (status, body) =
        request(&app, "POST", IMPORT_PATH, Some(&import_body), Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let summary: Value = serde_json::from_slice(&body)?;
    assert_eq!(summary["applied"], true);
    assert_eq!(
        audit_count(&db, "admin.backup.restored").await?,
        restored_before + 1
    );
    let generation_after = redis
        .get::<_, Option<u64>>("host_config:version")
        .await?
        .unwrap_or(0);
    assert!(
        generation_after > generation_before,
        "applied import did not invalidate host-config cache"
    );
    Ok(())
}
