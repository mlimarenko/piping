//! Protective tests for the bulk access-control endpoints.
//!
//! `admin_access_control/bulk.rs` had ZERO coverage, and it is a control-plane
//! surface: one request can create, update and delete users, groups, roles and
//! policies. It is also reachable from MCP, not only the admin UI.
//!
//! The property under test is the one that makes the endpoint safe:
//! `ensure_bulk_permissions` demands a SEPARATE action per section, so a
//! caller holding only CREATE must not be able to smuggle an update or a
//! delete into the same payload. Nothing pinned that — and the failure mode is
//! silent, because a bulk response reports per-item errors rather than
//! refusing the request.

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    build_test_redis, login_and_get_cookie, request, unique_suffix,
};
use serde_json::json;
use uuid::Uuid;

const BULK_USERS: &str = "/api/v1/admin/access-control/users/bulk";

/// Replace the shared test-admin policy with a create-only grant over USER,
/// then restore full access. Returns the policy id so the caller can drop it.
async fn grant_create_only_over_users(
    db: &sqlx::PgPool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(db)
            .await?;

    // Deny wins over allow, so a narrow DENY on UPDATE/DELETE × USER is what
    // actually constrains the admin role — adding another allow would not.
    // Unique per call: `rbac_policy.name` is UNIQUE, and a test that fails
    // before its cleanup would otherwise poison every later run.
    let policy_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints, subjects, resources, enabled
        )
        VALUES ($1, $5, 'deny', $2, $3, NULL, $4, '[]'::jsonb, true)
        "#,
    )
    .bind(policy_id)
    .bind(json!(["UPDATE", "DELETE"]))
    .bind(json!(["USER"]))
    .bind(json!([format!("role:{role_id}")]))
    .bind(format!("bulk-rbac-test-deny-{}", unique_suffix()))
    .execute(db)
    .await?;

    bump_policy_cache().await?;
    Ok(policy_id)
}

async fn drop_policy(
    db: &sqlx::PgPool,
    policy_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("DELETE FROM rbac_policy WHERE id = $1")
        .bind(policy_id)
        .execute(db)
        .await?;
    bump_policy_cache().await
}

/// Policies are cached in Redis behind a generation key; a direct SQL seed
/// bypasses the admin API's invalidation.
async fn bump_policy_cache() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut redis = build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn bulk_users_refuses_an_update_the_caller_may_not_perform()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    // Seed a victim through the fully-privileged path first.
    let victim = format!("bulk-rbac-victim-{}", unique_suffix());
    let create_body = json!({
        "username": victim,
        "email": format!("{victim}@test.local"),
        "status": "active",
        "auth_source": "local",
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let victim_id = serde_json::from_slice::<serde_json::Value>(&body)?
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("victim id")?
        .to_string();

    let policy_id = grant_create_only_over_users(&db).await?;

    // A single payload mixing an allowed CREATE with a forbidden UPDATE. The
    // whole request must be refused — not partially applied with the update
    // reported as a per-item error.
    let mixed = json!({
        "create": [{
            "username": format!("bulk-rbac-new-{}", unique_suffix()),
            "email": "bulk-rbac-new@test.local",
            "status": "active",
            "auth_source": "local",
        }],
        "update": [{
            "id": victim_id,
            "data": {
                "username": victim,
                "email": format!("{victim}@test.local"),
                "status": "disabled",
                "auth_source": "local",
            },
        }],
    })
    .to_string();
    let (status, _) = request(&app, "POST", BULK_USERS, Some(&mixed), Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a payload containing a forbidden section must be refused outright"
    );

    // And nothing from it may have landed.
    let victim_status: String =
        sqlx::query_scalar("SELECT status FROM identity_user WHERE id = $1")
            .bind(Uuid::parse_str(&victim_id)?)
            .fetch_one(&db)
            .await?;
    assert_eq!(
        victim_status, "active",
        "the refused update must not have been applied"
    );

    drop_policy(&db, policy_id).await?;
    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{victim_id}"),
        None,
        Some(&cookie),
    )
    .await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn bulk_users_refuses_a_delete_the_caller_may_not_perform()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    let victim = format!("bulk-rbac-del-{}", unique_suffix());
    let create_body = json!({
        "username": victim,
        "email": format!("{victim}@test.local"),
        "status": "active",
        "auth_source": "local",
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let victim_id = serde_json::from_slice::<serde_json::Value>(&body)?
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("victim id")?
        .to_string();

    let policy_id = grant_create_only_over_users(&db).await?;

    let payload = json!({ "delete": [victim_id] }).to_string();
    let (status, _) = request(&app, "POST", BULK_USERS, Some(&payload), Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a bulk delete must require the delete action"
    );

    let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identity_user WHERE id = $1")
        .bind(Uuid::parse_str(&victim_id)?)
        .fetch_one(&db)
        .await?;
    assert_eq!(still_there, 1, "the refused delete must not have run");

    drop_policy(&db, policy_id).await?;
    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{victim_id}"),
        None,
        Some(&cookie),
    )
    .await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn bulk_users_rejects_an_anonymous_caller()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);

    let payload = json!({ "delete": [Uuid::now_v7().to_string()] }).to_string();
    let (status, _) = request(&app, "POST", BULK_USERS, Some(&payload), None).await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the control-plane bulk endpoint must reject anonymous callers"
    );
    Ok(())
}
