//! Integration tests for OIDC secret lifecycle via unified secret engine.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_oidc_secret_engine_unification -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};
use serde_json::Value;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_oidc_secret_engine_unification -- --ignored
async fn oidc_settings_update_and_read_return_metadata_only_secret_state() {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let raw_secret = format!("oidc-secret-{}", unique_suffix());
    let update_payload = serde_json::json!({
        "enabled": false,
        "issuer_url": "https://issuer.example.test/realms/main",
        "client_id": "piping-client",
        "client_secret_enabled": true,
        "client_secret_value": raw_secret,
        "redirect_url": "https://piping.example.test/api/v1/auth/sso/callback",
        "scopes": ["openid", "profile", "email"],
        "group_claim": "groups",
        "provisioning_mode": "manual",
    })
    .to_string();

    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/admin/auth/oidc/settings",
        Some(&update_payload),
        Some(&cookie),
    )
    .await
    .expect("update oidc settings");
    assert_eq!(status, StatusCode::OK);
    let updated: Value = serde_json::from_slice(&body).expect("update response json");
    assert!(updated.get("client_secret").is_none());
    assert!(updated.get("client_secret_value").is_none());
    assert_eq!(
        updated
            .get("client_secret_state")
            .and_then(|value| value.get("enabled"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        updated
            .get("client_secret_state")
            .and_then(|value| value.get("has_value"))
            .and_then(Value::as_bool),
        Some(true)
    );

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/admin/auth/oidc/settings",
        None,
        Some(&cookie),
    )
    .await
    .expect("get oidc settings");
    assert_eq!(status, StatusCode::OK);
    let fetched: Value = serde_json::from_slice(&body).expect("get response json");
    assert!(fetched.get("client_secret").is_none());
    assert!(fetched.get("client_secret_value").is_none());
    assert_eq!(
        fetched
            .get("client_secret_state")
            .and_then(|value| value.get("enabled"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        fetched
            .get("client_secret_state")
            .and_then(|value| value.get("has_value"))
            .and_then(Value::as_bool),
        Some(true)
    );

    let has_legacy_column: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.columns
            WHERE table_name = 'auth_oidc_settings'
              AND column_name = 'client_secret_encrypted'
        )
        "#,
    )
    .fetch_one(&db_pool)
    .await
    .expect("query information_schema");
    assert!(
        !has_legacy_column,
        "auth_oidc_settings must not contain legacy client_secret_encrypted column",
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_oidc_secret_engine_unification -- --ignored
async fn oidc_test_connection_fails_closed_when_disabled() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let update_payload = serde_json::json!({
        "enabled": false,
        "issuer_url": "https://issuer.example.test/realms/main",
        "client_id": "piping-client",
        "client_secret_enabled": false,
        "client_secret_value": null,
        "redirect_url": "https://piping.example.test/api/v1/auth/sso/callback",
        "scopes": ["openid"],
        "group_claim": "groups",
        "provisioning_mode": "manual",
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/admin/auth/oidc/settings",
        Some(&update_payload),
        Some(&cookie),
    )
    .await
    .expect("update oidc settings");
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/auth/oidc/test-connection",
        None,
        Some(&cookie),
    )
    .await
    .expect("test oidc connection");

    assert_eq!(status, StatusCode::BAD_REQUEST);
    let response: Value = serde_json::from_slice(&body).expect("error response json");
    let detail = response
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !detail.contains("client_secret"),
        "fail-closed response must not leak secret-related internals",
    );
}
