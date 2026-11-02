//! Integration test for the delete-time sweep of the author-time runtime-secret
//! usage projection.
//!
//! Reproduces the data-integrity bug where `DELETE /playbooks/{id}` removed the
//! playbook and its versions but left `secret_usage_binding` rows with
//! `consumer_type = 'playbook_version:runtime_env'` dangling (`consumer_id` is
//! polymorphic — no FK onto `playbook_version`), permanently blocking
//! `DELETE /automation-secrets/{id}` with a 409 and inflating `usage_count`.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_playbook_secret_usage_sweep -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    build_test_redis, login_and_get_cookie, request, unique_suffix,
};
use serde_json::Value;
use uuid::Uuid;

const RUNTIME_ENV_CONSUMER_TYPE: &str = "playbook_version:runtime_env";

/// The shared `test-admin-policy` does not cover playbook resources; grant the
/// test-admin role playbook CRUD for this suite (idempotent).
async fn grant_playbook_admin(
    db: &sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(db)
            .await
            .map_err(|e| format!("test-admin role: {e}"))?;
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints, subjects, resources, enabled
        )
        VALUES ($1, 'test-admin-playbooks-policy', 'allow', $2, $3, NULL, $4, '[]'::jsonb, true)
        ON CONFLICT (name)
        DO UPDATE SET actions = EXCLUDED.actions,
                      resource_types = EXCLUDED.resource_types,
                      subjects = EXCLUDED.subjects,
                      enabled = true
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(serde_json::json!([
        "CREATE", "READ", "UPDATE", "DELETE", "EXECUTE"
    ]))
    .bind(serde_json::json!(["PLAYBOOK", "PLAYBOOK_VERSION"]))
    .bind(serde_json::json!([format!("role:{role_id}")]))
    .execute(db)
    .await
    .map_err(|e| format!("seed playbook admin policy: {e}"))?;

    // The policy set is cached in Redis behind a generation key; a direct SQL
    // seed bypasses the admin API's invalidation, so bump the generation here.
    let mut redis = build_test_redis()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await
        .map_err(|e| format!("bump policy cache generation: {e}"))?;
    Ok(())
}

async fn binding_count(
    db: &sqlx::PgPool,
    playbook_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM secret_usage_binding \
         WHERE consumer_type = $1 AND owner_scope_type = 'playbook' AND owner_scope_id = $2",
    )
    .bind(RUNTIME_ENV_CONSUMER_TYPE)
    .bind(playbook_id)
    .fetch_one(db)
    .await
    .map_err(|e| format!("count usage bindings: {e}"))?)
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_playbook_secret_usage_sweep -- --ignored
async fn deleting_playbook_sweeps_secret_usage_and_unblocks_secret_delete()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await.expect("db pool");
    grant_playbook_admin(&db).await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // Named automation secret the playbook will declare.
    let suffix = unique_suffix();
    let secret_key = format!("itest-usage-sweep-{suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/automation-secrets",
        Some(&serde_json::json!({ "key": secret_key }).to_string()),
        Some(&cookie),
    )
    .await
    .expect("create named secret");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create secret: {}",
        String::from_utf8_lossy(&body)
    );
    let secret: Value = serde_json::from_slice(&body).expect("secret json");
    let secret_id = secret["id"].as_str().expect("secret id").to_string();

    // Playbook whose initial version declares the secret via
    // `token_policy.runtime.secrets.env` — the author-time projection
    // upserts one usage binding per declaring version.
    let token_policy = serde_json::json!({
        "runtime": { "secrets": { "env": { "SWEEP_TOKEN": secret_key } } }
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks",
        Some(
            &serde_json::json!({
                "name": format!("itest usage sweep {suffix}"),
                "content": "- hosts: all\n  tasks: []\n",
                "token_policy": token_policy,
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await
    .expect("create playbook");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create playbook: {}",
        String::from_utf8_lossy(&body)
    );
    let item: Value = serde_json::from_slice(&body).expect("playbook json");
    let playbook_id: Uuid = item["playbook"]["id"]
        .as_str()
        .expect("playbook id")
        .parse()
        .expect("playbook uuid");

    assert_eq!(
        binding_count(&db, playbook_id).await?,
        1,
        "initial version must project one usage binding"
    );

    // Publishing another declaring version keeps the superseded version's
    // binding: usage is counted across ALL versions of a live playbook.
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/playbooks/{playbook_id}/versions"),
        Some(
            &serde_json::json!({
                "content": "- hosts: all\n  tasks: []\n# v2\n",
                "token_policy": token_policy,
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await
    .expect("create second version");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create version: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        binding_count(&db, playbook_id).await?,
        2,
        "each declaring version projects its own binding"
    );

    // While the playbook exists the secret is in use: delete must 409.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete in-use secret");
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "in-use named secret must be delete-protected"
    );

    // Duplicating the playbook copies the token_policy, so the copy's version 1
    // must project its own usage binding.
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/playbooks/{playbook_id}/duplicate"),
        Some(&serde_json::json!({}).to_string()),
        Some(&cookie),
    )
    .await
    .expect("duplicate playbook");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "duplicate playbook: {}",
        String::from_utf8_lossy(&body)
    );
    let copy: Value = serde_json::from_slice(&body).expect("duplicate json");
    let copy_id: Uuid = copy["playbook"]["id"]
        .as_str()
        .expect("copy id")
        .parse()
        .expect("copy uuid");
    assert_eq!(
        binding_count(&db, copy_id).await?,
        1,
        "duplicated playbook must project its inherited secret declaration"
    );

    // Deleting the original must sweep ITS projection in the same transaction…
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/{playbook_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete playbook");
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        binding_count(&db, playbook_id).await?,
        0,
        "playbook delete must not leave dangling usage bindings"
    );

    // …but only its own: the copy still declares the secret, so it stays
    // delete-protected.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete secret still used by the copy");
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "secret declared by the surviving duplicate must stay delete-protected"
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/{copy_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete duplicate playbook");
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        binding_count(&db, copy_id).await?,
        0,
        "duplicate delete must sweep its projection too"
    );

    // …and the admin surface must report the secret as unused.
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/automation-secrets",
        None,
        Some(&cookie),
    )
    .await
    .expect("list secrets");
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&body).expect("list json");
    let usage_count = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|entry| entry["id"].as_str() == Some(secret_id.as_str()))
        .expect("created secret in list")["usage_count"]
        .as_i64();
    assert_eq!(
        usage_count,
        Some(0),
        "usage_count must be 0 after the playbook is gone"
    );

    // The secret is deletable again.
    let (status, body) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unblocked secret");
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "delete secret: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(())
}
