//! Integration tests for `/api/v1/secret-fields`.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_secret_fields_api -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie, request,
};
use serde_json::Value;
use uuid::Uuid;

#[allow(
    clippy::expect_used,
    reason = "integration fixture helper outside the #[test] fns clippy.toml exempts; a rejected native-proxy insert means the isolated stack is unusable, so the calling test must abort"
)]
async fn seed_native_proxy(pool: &sqlx::PgPool, id: Uuid) {
    sqlx::query("INSERT INTO connections_proxy_node (id, name, slug, base_url) VALUES ($1, $2, $2, 'https://synthetic-proxy.invalid')")
        .bind(id).bind(format!("lifecycle-{id}")).execute(pool).await.expect("native proxy fixture");
}

#[tokio::test]
#[ignore] // Requires the isolated PostgreSQL and Redis integration stack.
async fn explicit_secret_id_rebinds_an_existing_field() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let owner_id = Uuid::now_v7();
    seed_native_proxy(&state.db_pool, owner_id).await;
    let consumers = [owner_id, owner_id];
    let fields = [
        "piping.connections.ssh.0.password",
        "piping.connections.ssh.1.password",
    ];
    let mut secret_ids = Vec::new();
    for (index, consumer_id) in consumers.into_iter().enumerate() {
        let payload = serde_json::json!({
            "consumer_type": "proxy_node:credential",
            "consumer_id": consumer_id,
            "owner_scope_type": "proxy_node",
            "owner_scope_id": owner_id,
            "field_key": fields[index],
            "intent": "set",
            "value": "synthetic-field-rebind-value",
        })
        .to_string();
        let (status, body) = request(
            &app,
            "PUT",
            "/api/v1/secret-fields",
            Some(&payload),
            Some(&cookie),
        )
        .await
        .expect("create managed field");
        assert_eq!(status, StatusCode::OK);
        let response: Value = serde_json::from_slice(&body).expect("field response");
        secret_ids.push(
            response["secret_id"]
                .as_str()
                .expect("secret id")
                .to_owned(),
        );
    }
    assert_ne!(secret_ids[0], secret_ids[1]);
    let payload = serde_json::json!({
        "consumer_type": "proxy_node:credential",
        "consumer_id": consumers[0],
        "owner_scope_type": "proxy_node",
        "owner_scope_id": owner_id,
        "field_key": fields[0],
        "intent": "set",
        "secret_id": secret_ids[1],
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("rebind field");
    assert_eq!(status, StatusCode::OK);
    let response: Value = serde_json::from_slice(&body).expect("rebind response");
    assert_eq!(
        response["secret_id"].as_str(),
        Some(secret_ids[1].as_str()),
        "an explicit replacement must not silently retain the old secret"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn secret_fields_apply_and_list_roundtrip_returns_metadata_only_state() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let consumer_id = Uuid::now_v7();
    let owner_scope_id = consumer_id;
    seed_native_proxy(&state.db_pool, consumer_id).await;
    let field_key = "piping.connections.ssh.0.password";
    let consumer_type = "proxy_node:credential";

    let set_payload = serde_json::json!({
        "consumer_type": consumer_type,
        "consumer_id": consumer_id,
        "owner_scope_type": "proxy_node",
        "owner_scope_id": owner_scope_id,
        "field_key": field_key,
        "intent": "set",
        "value": "integration-secret-value",
    })
    .to_string();

    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&set_payload),
        Some(&cookie),
    )
    .await
    .expect("apply set intent");
    assert_eq!(status, StatusCode::OK);
    let set_json: Value = serde_json::from_slice(&body).expect("set response json");
    assert_eq!(
        set_json.get("enabled").and_then(Value::as_bool),
        Some(true),
        "enabled should be true after set",
    );
    assert_eq!(
        set_json.get("has_value").and_then(Value::as_bool),
        Some(true),
        "has_value should be true after set",
    );
    let secret_id = set_json
        .get("secret_id")
        .and_then(Value::as_str)
        .expect("secret id should be present");
    assert!(
        Uuid::parse_str(secret_id).is_ok(),
        "secret_id must be UUID, got: {secret_id}",
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/secret-fields?consumer_type={}&consumer_id={}",
            consumer_type, consumer_id
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("list states");
    assert_eq!(status, StatusCode::OK);
    let states: Value = serde_json::from_slice(&body).expect("list response json");
    let first = states
        .as_array()
        .and_then(|items| items.first())
        .expect("at least one state");
    assert_eq!(
        first.get("field_key").and_then(Value::as_str),
        Some(field_key),
        "field_key mismatch",
    );
    assert_eq!(
        first.get("secret_id").and_then(Value::as_str),
        Some(secret_id),
        "secret_id mismatch",
    );

    let clear_payload = serde_json::json!({
        "consumer_type": consumer_type,
        "consumer_id": consumer_id,
        "owner_scope_type": "proxy_node",
        "owner_scope_id": owner_scope_id,
        "field_key": field_key,
        "intent": "clear",
        "secret_id": secret_id,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&clear_payload),
        Some(&cookie),
    )
    .await
    .expect("apply clear intent");
    assert_eq!(status, StatusCode::OK);
    let clear_json: Value = serde_json::from_slice(&body).expect("clear response json");
    assert_eq!(
        clear_json.get("enabled").and_then(Value::as_bool),
        Some(false),
        "enabled should be false after clear",
    );
    assert_eq!(
        clear_json.get("secret_id").and_then(Value::as_str),
        None,
        "secret_id should be cleared",
    );
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
                .bind(Uuid::parse_str(secret_id).expect("secret UUID"))
                .fetch_one(&state.db_pool)
                .await
                .expect("count retained secret rows");
        assert_eq!(remaining, 0, "cleared managed field remains in {table}");
    }
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn secret_fields_apply_rejects_unknown_intent_fail_closed() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let payload = serde_json::json!({
        "consumer_type": "connection_scope:connection_target",
        "consumer_id": Uuid::now_v7(),
        "owner_scope_type": "connection_target",
        "owner_scope_id": Uuid::now_v7(),
        "field_key": "piping.connections.ssh.0.password",
        "intent": "unknown-intent",
        "value": "must-not-pass",
    })
    .to_string();

    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply invalid intent");

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        String::from_utf8_lossy(&body).contains("intent must be one of"),
        "response must explain allowed intent values"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn user_connection_credentials_rejected_as_deprecated_consumer_type() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let payload = serde_json::json!({
        "consumer_type": "user_connection_credential",
        "consumer_id": Uuid::now_v7(),
        "owner_scope_type": "user",
        "owner_scope_id": Uuid::now_v7(),
        "field_key": "ssh_password",
        "intent": "set",
        "value": "must-not-pass",
    })
    .to_string();

    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply cross-user intent");

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn user_connection_credentials_self_scoped_write_rejected_as_deprecated() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(&app, "GET", "/api/v1/auth/me", None, Some(&cookie))
        .await
        .expect("auth me");
    assert_eq!(status, StatusCode::OK);
    let auth_me: Value = serde_json::from_slice(&body).expect("auth me json");
    let user_id = auth_me
        .get("user_id")
        .and_then(Value::as_str)
        .expect("user_id in auth/me");

    let payload = serde_json::json!({
        "consumer_type": "user_connection_credential",
        "consumer_id": user_id,
        "owner_scope_type": "user",
        "owner_scope_id": user_id,
        "field_key": "ssh_password",
        "intent": "set",
        "value": "self-scoped-secret",
    })
    .to_string();

    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply self-scoped intent");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        String::from_utf8_lossy(&body).contains("user_connection_credential_set"),
        "response must explain canonical consumer_type; got: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn user_connection_credential_set_allows_self_scoped_write() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(&app, "GET", "/api/v1/auth/me", None, Some(&cookie))
        .await
        .expect("auth me");
    assert_eq!(status, StatusCode::OK);
    let auth_me: Value = serde_json::from_slice(&body).expect("auth me json");
    let user_id = auth_me
        .get("user_id")
        .and_then(Value::as_str)
        .expect("user_id in auth/me");

    let create_set_payload = serde_json::json!({
        "protocol": "ssh",
        "name": "Self Set",
        "params": {},
        "is_default": true
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&create_set_payload),
        Some(&cookie),
    )
    .await
    .expect("create credential set");
    assert_eq!(status, StatusCode::CREATED);
    let created_set: Value = serde_json::from_slice(&body).expect("created set json");
    let set_id = created_set
        .get("id")
        .and_then(Value::as_str)
        .expect("set id");

    let payload = serde_json::json!({
        "consumer_type": "user_connection_credential_set",
        "consumer_id": set_id,
        "owner_scope_type": "user",
        "owner_scope_id": user_id,
        "field_key": "ssh_private_key",
        "intent": "set",
        "value": "self-scoped-key",
    })
    .to_string();

    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply set intent");
    assert_eq!(status, StatusCode::OK);

    let json: Value = serde_json::from_slice(&body).expect("set response json");
    assert_eq!(json.get("enabled").and_then(Value::as_bool), Some(true));
    assert_eq!(json.get("has_value").and_then(Value::as_bool), Some(true));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn user_connection_credential_set_rejects_foreign_owner_scope() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_set_payload = serde_json::json!({
        "protocol": "ssh",
        "name": "Owner protected set",
        "params": {},
        "is_default": true
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&create_set_payload),
        Some(&cookie),
    )
    .await
    .expect("create credential set");
    assert_eq!(status, StatusCode::CREATED);
    let created_set: Value = serde_json::from_slice(&body).expect("created set json");
    let set_id = created_set
        .get("id")
        .and_then(Value::as_str)
        .expect("set id");

    let payload = serde_json::json!({
        "consumer_type": "user_connection_credential_set",
        "consumer_id": set_id,
        "owner_scope_type": "user",
        "owner_scope_id": Uuid::now_v7(),
        "field_key": "ssh_password",
        "intent": "set",
        "value": "must-not-pass",
    })
    .to_string();

    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply foreign-owner intent");
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_secret_fields_api -- --ignored
async fn user_connection_credential_set_rejects_foreign_consumer_set_id() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(&app, "GET", "/api/v1/auth/me", None, Some(&cookie))
        .await
        .expect("auth me");
    assert_eq!(status, StatusCode::OK);
    let auth_me: Value = serde_json::from_slice(&body).expect("auth me json");
    let actor_user_id = auth_me
        .get("user_id")
        .and_then(Value::as_str)
        .expect("user_id in auth/me");

    let pool = build_test_db_pool().await.expect("db pool");
    let foreign_user_id = Uuid::now_v7();
    let foreign_set_id = Uuid::now_v7();
    let foreign_username = format!("foreign-secret-owner-{}", Uuid::now_v7());
    let foreign_email = format!("{foreign_username}@test.local");
    sqlx::query(
        r#"
        INSERT INTO identity_user (id, username, email, status, auth_source)
        VALUES ($1, $2, $3, 'active', 'local')
        "#,
    )
    .bind(foreign_user_id)
    .bind(&foreign_username)
    .bind(&foreign_email)
    .execute(&pool)
    .await
    .expect("insert foreign user");
    sqlx::query(
        r#"
        INSERT INTO connections_user_credential_set (id, user_id, protocol, name, is_default)
        VALUES ($1, $2, 'ssh', 'Foreign SSH Set', true)
        "#,
    )
    .bind(foreign_set_id)
    .bind(foreign_user_id)
    .execute(&pool)
    .await
    .expect("insert foreign set");

    let payload = serde_json::json!({
        "consumer_type": "user_connection_credential_set",
        "consumer_id": foreign_set_id,
        "owner_scope_type": "user",
        "owner_scope_id": actor_user_id,
        "field_key": "ssh_private_key",
        "intent": "set",
        "value": "must-not-pass",
    })
    .to_string();

    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply foreign set intent");
    assert_eq!(status, StatusCode::FORBIDDEN);
}
