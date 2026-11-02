//! Integration tests for user local password secret lifecycle.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_user_password_secret_lifecycle -- --ignored`

mod common;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie,
};
use piping_backend::contexts::secrets::application::SecretStore;
use piping_backend::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService,
};
use piping_backend::contexts::secrets::infrastructure::DbSecretStore;
use piping_backend::contexts::secrets::infrastructure::SqlxSecretRepository;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

const USER_PASSWORD_SECRET_CONSUMER_TYPE: &str = "user_auth_password";
const USER_PASSWORD_SECRET_FIELD_KEY: &str = "local_password";

async fn load_user_password_secret_id(
    db: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        r#"
        SELECT secret_id, owner_scope_type, owner_scope_id
        FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
          AND field_key = $3
        "#,
    )
    .bind(USER_PASSWORD_SECRET_CONSUMER_TYPE)
    .bind(user_id)
    .bind(USER_PASSWORD_SECRET_FIELD_KEY)
    .fetch_one(db)
    .await
    .map_err(|e| format!("secret_usage_binding row: {e}"))?;

    let owner_scope_type: String = row
        .try_get("owner_scope_type")
        .map_err(|e| format!("owner_scope_type: {e}"))?;
    let owner_scope_id: Uuid = row
        .try_get("owner_scope_id")
        .map_err(|e| format!("owner_scope_id: {e}"))?;
    assert_eq!(owner_scope_type, "user");
    assert_eq!(owner_scope_id, user_id);

    Ok(row
        .try_get("secret_id")
        .map_err(|e| format!("secret_id: {e}"))?)
}

async fn load_secret_value(
    db: &sqlx::PgPool,
    secret_id: Uuid,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let store = DbSecretStore::new(db.clone());
    Ok(store
        .get_secret(secret_id)
        .await
        .map_err(|e| format!("secret store get_secret: {e}"))?)
}

#[tokio::test]
#[ignore] // Requires PostgreSQL + Redis; run with: cargo test --test integration_user_password_secret_lifecycle -- --ignored
async fn manual_and_generated_password_rotation_writes_through_secret_engine_and_auth_works()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);

    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");

    let suffix = common::unique_suffix();
    let username = format!("user-pass-lifecycle-{}", suffix);
    let initial_password = "Abcd1234";

    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{}@test.local", username),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "manual",
            "password": initial_password
        }
    })
    .to_string();
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&admin_cookie),
    )
    .await
    .expect("create user");
    assert_eq!(status, axum::http::StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user_id: Uuid = json
        .get("id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .expect("user id");

    // Manual password should be stored via secret engine binding.
    let secret_id = load_user_password_secret_id(&db_pool, user_id).await?;
    let stored = load_secret_value(&db_pool, secret_id).await?;
    assert_eq!(stored, initial_password);

    let user_cookie = login_and_get_cookie(&app, &username, initial_password).await;
    assert!(user_cookie.is_some(), "manual password login must succeed");

    // Rotate password via generated mode and ensure secret store matches returned value.
    let rotate_body = serde_json::json!({
        "mode": "generate"
    })
    .to_string();
    let (status, body) = common::request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/users/{}/password", user_id),
        Some(&rotate_body),
        Some(&admin_cookie),
    )
    .await
    .expect("rotate password");
    assert_eq!(status, axum::http::StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let generated_password = json
        .get("generated_password")
        .and_then(|value| value.as_str())
        .expect("generated_password");

    // Secret id should remain stable; value should update.
    let secret_id_after = load_user_password_secret_id(&db_pool, user_id).await?;
    assert_eq!(secret_id_after, secret_id);
    let rotated = load_secret_value(&db_pool, secret_id_after).await?;
    assert_eq!(rotated, generated_password);

    let old_cookie = login_and_get_cookie(&app, &username, initial_password).await;
    assert!(
        old_cookie.is_none(),
        "old password should no longer authenticate after rotation"
    );
    let new_cookie = login_and_get_cookie(&app, &username, generated_password).await;
    assert!(
        new_cookie.is_some(),
        "generated rotated password login must succeed"
    );

    // User deletion cascades native personal records/credential sets. Capture
    // and retire their fields in the same transaction, not after FK deletion.
    let set_id = Uuid::now_v7();
    let record_id = Uuid::now_v7();
    sqlx::query("INSERT INTO connections_user_credential_set (id, user_id, protocol, name) VALUES ($1, $2, 'ssh', 'synthetic-owned-set')")
        .bind(set_id).bind(user_id).execute(&db_pool).await?;
    sqlx::query("INSERT INTO connconf_record (id, owner_user_id, protocol, name) VALUES ($1, $2, 'ssh', 'synthetic-owned-record')")
        .bind(record_id).bind(user_id).execute(&db_pool).await?;
    let fields =
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(db_pool.clone())));
    let mut child_secrets = Vec::new();
    for (consumer_type, consumer_id, field_key) in [
        ("user_connection_credential_set", set_id, "ssh_password"),
        (
            "connection_config:record",
            record_id,
            "credentials.password",
        ),
    ] {
        let field = fields
            .apply_intent(ApplySecretFieldIntentInput {
                consumer_type: consumer_type.into(),
                consumer_id,
                field_key: field_key.into(),
                owner_scope_type: "user".into(),
                owner_scope_id: user_id,
                intent: SecretFieldIntent::Set,
                value: Some("synthetic-child-value".into()),
                secret_id: None,
            })
            .await?;
        child_secrets.push(field.secret_id.expect("owned child secret"));
    }

    let (status, _) = common::request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("delete user");
    assert_eq!(status, axum::http::StatusCode::OK);
    for (table, id) in [
        ("connections_user_credential_set", set_id),
        ("connconf_record", record_id),
    ] {
        let remaining: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
                .bind(id)
                .fetch_one(&db_pool)
                .await?;
        assert_eq!(remaining, 0, "owned child resource remains in {table}");
    }
    child_secrets.push(secret_id);
    for secret_id in child_secrets {
        for table in [
            "secrets_variable",
            "secrets_store",
            "secrets_binding",
            "secret_usage_binding",
        ] {
            let column = if table == "secrets_variable" {
                "id"
            } else {
                "secret_id"
            };
            let remaining: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = $1"))
                    .bind(secret_id)
                    .fetch_one(&db_pool)
                    .await?;
            assert_eq!(remaining, 0, "deleted user's password remains in {table}");
        }
    }
    Ok(())
}
