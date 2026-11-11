//! Integration tests for sync provider secret unification.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_sync_provider_secret_engine_unification -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_sync_provider_secret_engine_unification -- --ignored
async fn provider_create_and_update_extracts_sensitive_fields_into_secret_engine_bindings() {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_payload = serde_json::json!({
        "name": format!("provider-{}", unique_suffix()),
        "provider_type": "netbox",
        "is_enabled": true,
        "bootstrap": false,
        "config": {
            "base_url": "http://127.0.0.1:8000",
            "sync_mode": "pull_only",
            "page_size": 100,
            "webhook_events": ["created"],
            "webhook_object_types": ["dcim.device"],
            "api_token": format!("api-token-{}", unique_suffix()),
            "webhook_secret": format!("webhook-secret-{}", unique_suffix()),
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/providers",
        Some(&create_payload),
        Some(&cookie),
    )
    .await
    .expect("create provider");
    assert_eq!(status, StatusCode::CREATED);
    let created: Value = serde_json::from_slice(&body).expect("create response json");
    let provider_id = created
        .get("id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .expect("provider id");

    let binding_count_row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM secret_usage_binding
        WHERE consumer_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&db_pool)
    .await
    .expect("count provider secret bindings");
    let binding_count: i64 = binding_count_row.try_get("count").expect("count");
    assert!(
        binding_count >= 2,
        "provider secrets should be extracted into usage bindings",
    );

    let update_payload = serde_json::json!({
        "config": {
            "api_token": format!("rotated-api-token-{}", unique_suffix()),
            "webhook_secret": format!("rotated-webhook-secret-{}", unique_suffix()),
        },
        "bootstrap": false
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/providers/{provider_id}"),
        Some(&update_payload),
        Some(&cookie),
    )
    .await
    .expect("update provider");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_sync_provider_secret_engine_unification -- --ignored
async fn provider_get_response_hides_plaintext_sensitive_config_fields() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_payload = serde_json::json!({
        "name": format!("provider-{}", unique_suffix()),
        "provider_type": "netbox",
        "is_enabled": true,
        "bootstrap": false,
        "config": {
            "base_url": "http://127.0.0.1:8000",
            "sync_mode": "pull_only",
            "page_size": 100,
            "webhook_events": ["created"],
            "webhook_object_types": ["dcim.device"],
            "api_token": format!("api-token-{}", unique_suffix()),
            "webhook_secret": format!("webhook-secret-{}", unique_suffix()),
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/providers",
        Some(&create_payload),
        Some(&cookie),
    )
    .await
    .expect("create provider");
    assert_eq!(status, StatusCode::CREATED);
    let created: Value = serde_json::from_slice(&body).expect("create response json");
    let provider_id = created
        .get("id")
        .and_then(Value::as_str)
        .expect("provider id");

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/providers/{provider_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get provider");
    assert_eq!(status, StatusCode::OK);
    let provider: Value = serde_json::from_slice(&body).expect("provider response json");
    let config = provider.get("config").and_then(Value::as_object);

    let api_token = config.and_then(|value| value.get("api_token"));
    let webhook_secret = config.and_then(|value| value.get("webhook_secret"));
    assert!(
        api_token.is_none() || api_token == Some(&Value::Null),
        "provider payload must not expose plaintext api_token",
    );
    assert!(
        webhook_secret.is_none() || webhook_secret == Some(&Value::Null),
        "provider payload must not expose plaintext webhook_secret",
    );
}
