//! Integration tests for NetBox webhook signature and security behavior.
//!
//! Requires PostgreSQL and Redis. Run with:
//! `cargo test --test integration_netbox_webhook_security -- --ignored`

mod common;

use axum::Router;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_webhook_security -- --ignored
async fn webhook_requires_valid_signature_header()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, "secret-a").await?;
    let payload = webhook_payload();
    let payload_text = payload.to_string();

    let (status, _) = request(
        &app.router,
        "POST",
        &format!("/api/v1/webhooks/netbox/{provider_id}"),
        Some(&payload_text),
        None,
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    let signature = sign_payload(payload_text.as_bytes(), "secret-a")?;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/v1/webhooks/netbox/{provider_id}"))
        .header("content-type", "application/json")
        .header("x-hook-signature", signature)
        .body(axum::body::Body::from(payload_text.clone()))
        .expect("request");
    let response = app.router.clone().oneshot(req).await.expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_webhook_security -- --ignored
async fn webhook_rejects_invalid_signature_without_side_effects()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, "secret-b").await?;
    let payload = webhook_payload();
    let payload_text = payload.to_string();

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/v1/webhooks/netbox/{provider_id}"))
        .header("content-type", "application/json")
        .header("x-hook-signature", "invalid-signature")
        .body(axum::body::Body::from(payload_text))
        .expect("request");
    let response = app.router.clone().oneshot(req).await.expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);

    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM sync_webhook_event
        WHERE source_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("count row");
    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_webhook_security -- --ignored
async fn webhook_unknown_provider_returns_404()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let payload = webhook_payload().to_string();
    let signature = sign_payload(payload.as_bytes(), "secret-c")?;

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/v1/webhooks/netbox/{}", Uuid::now_v7()))
        .header("content-type", "application/json")
        .header("x-hook-signature", signature)
        .body(axum::body::Body::from(payload))
        .expect("request");
    let response = app.router.clone().oneshot(req).await.expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    Ok(())
}

struct AuthedApp {
    router: Router,
    state: piping_backend::app::AppState,
    cookie: String,
}

async fn build_authed_app() -> Result<AuthedApp, Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state()
        .await
        .map_err(|e| format!("app state: {e}"))?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&router, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "login failed".to_string())?;
    Ok(AuthedApp {
        router,
        state,
        cookie,
    })
}

async fn create_provider_with_secret(
    app: &Router,
    cookie: &str,
    secret: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": format!("provider-{}", unique_suffix()),
                "provider_type": "netbox",
                "config": {
                    "base_url": "http://127.0.0.1:8000",
                    "api_token": "test-token",
                    "page_size": 100,
                    "sync_mode": "pull_only",
                    "webhook_secret": secret,
                    "webhook_object_types": [1]
                },
                "bootstrap": false
            })
            .to_string(),
        ),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create provider: {e}"))?;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let json: Value = serde_json::from_slice(&body).map_err(|e| format!("json: {e}"))?;
    let id = json
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider id".to_string())?;
    Ok(Uuid::parse_str(id).map_err(|e| format!("uuid: {e}"))?)
}

fn webhook_payload() -> Value {
    json!({
        "event": "updated",
        "timestamp": "2026-02-11T12:00:00Z",
        "model": "dcim.site",
        "request_id": "request-1",
        "data": {
            "id": 101,
            "name": "site-a"
        },
        "snapshots": {}
    })
}

fn sign_payload(
    payload: &[u8],
    secret: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key = PKey::hmac(secret.as_bytes()).map_err(|e| format!("hmac key: {e}"))?;
    let mut signer =
        Signer::new(MessageDigest::sha512(), &key).map_err(|e| format!("signer: {e}"))?;
    signer.update(payload).map_err(|e| format!("update: {e}"))?;
    let signature = signer.sign_to_vec().map_err(|e| format!("sign: {e}"))?;
    Ok(signature
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}
