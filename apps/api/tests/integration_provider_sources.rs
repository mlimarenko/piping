//! Integration tests for provider/source lifecycle endpoints.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_provider_sources -- --ignored`

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::{get, patch},
};
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};

#[derive(Clone, Default)]
struct MockNetboxState {
    webhooks: Arc<tokio::sync::Mutex<Vec<Value>>>,
    event_rules: Arc<tokio::sync::Mutex<Vec<Value>>>,
    post_count: Arc<AtomicUsize>,
    patch_count: Arc<AtomicUsize>,
}

struct MockNetboxHandle {
    base_url: String,
    state: MockNetboxState,
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn post_providers_validates_payload_schema_and_allowed_provider_types() {
    let app = build_authed_app().await.expect("authed app");

    let (status, _) = request(
        &app.router,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": format!("provider-{}", unique_suffix()),
                "config": netbox_provider_config("http://127.0.0.1:8000"),
                "bootstrap": false
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert!(
        status.is_client_error(),
        "missing provider_type must fail with 4xx, got {status}"
    );

    let (status, _) = request(
        &app.router,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": format!("provider-{}", unique_suffix()),
                "provider_type": "unsupported",
                "config": netbox_provider_config("http://127.0.0.1:8000"),
                "bootstrap": false
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn patch_provider_supports_partial_update_and_validation_errors() {
    let app = build_authed_app().await.expect("authed app");
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &format!("provider-{}", unique_suffix()),
        netbox_provider_config("http://127.0.0.1:8000"),
        false,
    )
    .await
    .expect("provider");

    let (status, body) = request(
        &app.router,
        "PATCH",
        &format!("/api/v1/providers/{provider_id}"),
        Some(&json!({"is_enabled": false}).to_string()),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::OK);
    let json: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json.get("is_enabled").and_then(Value::as_bool), Some(false));

    let (status, _) = request(
        &app.router,
        "PATCH",
        &format!("/api/v1/providers/{provider_id}"),
        Some(&json!({"config": "invalid"}).to_string()),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn delete_provider_returns_204_and_404() {
    let app = build_authed_app().await.expect("authed app");
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &format!("provider-{}", unique_suffix()),
        netbox_provider_config("http://127.0.0.1:8000"),
        false,
    )
    .await
    .expect("provider");

    let (status, _) = request(
        &app.router,
        "DELETE",
        &format!("/api/v1/providers/{provider_id}"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);

    let (status, _) = request(
        &app.router,
        "DELETE",
        &format!("/api/v1/providers/{provider_id}"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn replay_outbox_unknown_provider_returns_404() {
    let app = build_authed_app().await.expect("authed app");
    let missing_id = Uuid::now_v7();

    let (status, _body) = request(
        &app.router,
        "POST",
        &format!("/api/v1/providers/{missing_id}/outbox/replay"),
        Some(&json!({ "limit": 10 }).to_string()),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn create_provider_with_bootstrap_false_persists_pending_runtime() {
    let app = build_authed_app().await.expect("authed app");
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &format!("provider-{}", unique_suffix()),
        netbox_provider_config("http://127.0.0.1:8000"),
        false,
    )
    .await
    .expect("provider");

    let row = sqlx::query(
        r#"
        SELECT bootstrap_status, external_webhook_id
        FROM sync_source_runtime
        WHERE source_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("runtime row");

    let bootstrap_status: String = row.try_get("bootstrap_status").expect("status");
    let external_webhook_id: Option<String> = row.try_get("external_webhook_id").expect("id");
    assert_eq!(bootstrap_status, "pending");
    assert!(external_webhook_id.is_none());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn bidirectional_provider_with_bootstrap_true_sets_ready_and_webhook_id() {
    let app = build_authed_app().await.expect("authed app");
    let netbox = spawn_mock_netbox().await.expect("mock netbox");

    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &format!("provider-{}", unique_suffix()),
        netbox_provider_config_custom(&netbox.base_url, "test-token", "bidirectional", vec![1]),
        true,
    )
    .await
    .expect("provider");

    let row = sqlx::query(
        r#"
        SELECT bootstrap_status, external_webhook_id
        FROM sync_source_runtime
        WHERE source_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("runtime row");

    let bootstrap_status: String = row.try_get("bootstrap_status").expect("status");
    let external_webhook_id: Option<String> = row.try_get("external_webhook_id").expect("id");
    assert_eq!(bootstrap_status, "ready");
    assert!(external_webhook_id.is_some());
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn create_provider_rejects_duplicate_name_with_conflict() {
    let app = build_authed_app().await.expect("authed app");
    let provider_name = format!("provider-{}", unique_suffix());
    let _ = create_provider(
        &app.router,
        &app.cookie,
        &provider_name,
        netbox_provider_config("http://127.0.0.1:8000"),
        false,
    )
    .await
    .expect("provider");

    let (status, _) = request(
        &app.router,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": provider_name,
                "provider_type": "netbox",
                "config": netbox_provider_config("http://127.0.0.1:8000"),
                "bootstrap": false
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn bidirectional_bootstrap_invalid_credentials_sets_failed_runtime() {
    let app = build_authed_app().await.expect("authed app");

    let (status, body) = request(
        &app.router,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": format!("provider-{}", unique_suffix()),
                "provider_type": "netbox",
                "config": netbox_provider_config_custom("http://127.0.0.1:9", "test-token", "bidirectional", vec![1]),
                "bootstrap": true
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::BAD_GATEWAY);

    let body_json: Value = serde_json::from_slice(&body).expect("json");
    let message = body_json
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(!message.is_empty());

    let row = sqlx::query(
        r#"
        SELECT bootstrap_status, operational_status, last_error_code
        FROM sync_source_runtime
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_one(&app.state.db_pool)
    .await
    .expect("runtime row");
    let bootstrap_status: String = row.try_get("bootstrap_status").expect("bootstrap_status");
    let operational_status: String = row
        .try_get("operational_status")
        .expect("operational_status");
    let last_error_code: Option<String> = row.try_get("last_error_code").expect("last_error_code");
    assert_eq!(bootstrap_status, "failed");
    assert_eq!(operational_status, "bootstrap_failed");
    assert_eq!(last_error_code.as_deref(), Some("bootstrap_request_failed"));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_provider_sources -- --ignored
async fn bidirectional_bootstrap_endpoint_is_idempotent() {
    let app = build_authed_app().await.expect("authed app");
    let netbox = spawn_mock_netbox().await.expect("mock netbox");

    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &format!("provider-{}", unique_suffix()),
        netbox_provider_config_custom(&netbox.base_url, "test-token", "bidirectional", vec![1]),
        true,
    )
    .await
    .expect("provider");

    let (status, _) = request(
        &app.router,
        "POST",
        &format!("/api/v1/providers/{provider_id}/bootstrap"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, _) = request(
        &app.router,
        "POST",
        &format!("/api/v1/providers/{provider_id}/bootstrap"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, axum::http::StatusCode::OK);

    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    assert!(
        netbox.state.patch_count.load(Ordering::SeqCst) >= 1,
        "second bootstrap must patch existing webhook"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL, Redis, and running NetBox; set NETBOX_REAL_E2E=1 to execute.
async fn bootstrap_with_real_netbox_creates_remote_webhook() {
    if std::env::var("NETBOX_REAL_E2E").as_deref() != Ok("1") {
        return;
    }
    let app = build_authed_app().await.expect("authed app");
    let base_url = std::env::var("NETBOX_REAL_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:28080".to_string());
    let api_token =
        std::env::var("NETBOX_REAL_API_TOKEN").unwrap_or_else(|_| "netbox-admin-token".to_string());
    let client = Client::new();

    let status = client
        .get(format!("{}/api/status/", base_url.trim_end_matches('/')))
        .header("Authorization", format!("Token {}", api_token))
        .send()
        .await
        .expect("netbox status request")
        .status();
    assert!(
        status.is_success(),
        "real netbox status check must be successful, got {status}"
    );

    let object_types: Value = client
        .get(format!(
            "{}/api/extras/object-types/?app_label=dcim&model=site",
            base_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Token {}", api_token))
        .send()
        .await
        .expect("object types request")
        .json()
        .await
        .expect("object types json");
    let site_object_type_id = object_types
        .get("results")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|row| row.get("id"))
        .and_then(Value::as_i64)
        .expect("dcim.site object type id") as i32;

    let config = netbox_provider_config_custom(
        &base_url,
        &api_token,
        "bidirectional",
        vec![site_object_type_id],
    );
    let provider_name = format!("provider-real-{}", unique_suffix());
    let provider_id = create_provider(&app.router, &app.cookie, &provider_name, config, true)
        .await
        .expect("provider");

    let row = sqlx::query(
        r#"
        SELECT bootstrap_status, external_webhook_id
        FROM sync_source_runtime
        WHERE source_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("runtime row");
    let bootstrap_status: String = row.try_get("bootstrap_status").expect("bootstrap_status");
    let external_webhook_id: Option<String> =
        row.try_get("external_webhook_id").expect("webhook id");
    assert_eq!(bootstrap_status, "ready");
    assert!(external_webhook_id.is_some());

    let webhooks: Value = client
        .get(format!(
            "{}/api/extras/webhooks/?name=piping-source-{}",
            base_url.trim_end_matches('/'),
            provider_id
        ))
        .header("Authorization", format!("Token {}", api_token))
        .send()
        .await
        .expect("webhook list request")
        .json()
        .await
        .expect("webhook list json");
    assert_eq!(webhooks.get("count").and_then(Value::as_u64), Some(1));

    let event_rules: Value = client
        .get(format!(
            "{}/api/extras/event-rules/?name=piping-source-{}-events",
            base_url.trim_end_matches('/'),
            provider_id
        ))
        .header("Authorization", format!("Token {}", api_token))
        .send()
        .await
        .expect("event rule list request")
        .json()
        .await
        .expect("event rule list json");
    assert_eq!(event_rules.get("count").and_then(Value::as_u64), Some(1));

    let (status, _) = request(
        &app.router,
        "DELETE",
        &format!("/api/v1/providers/{provider_id}"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("provider delete");
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn pull_only_provider_bootstrap_never_creates_remote_resources() {
    let app = build_authed_app().await.expect("authed app");
    let netbox = spawn_mock_netbox().await.expect("mock source");
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &format!("provider-{}", unique_suffix()),
        netbox_provider_config(&netbox.base_url),
        true,
    )
    .await
    .expect("provider");
    for _ in 0..2 {
        let (status, _) = request(
            &app.router,
            "POST",
            &format!("/api/v1/providers/{provider_id}/bootstrap"),
            None,
            Some(&app.cookie),
        )
        .await
        .expect("bootstrap request");
        assert_eq!(status, axum::http::StatusCode::OK);
    }
    let row = sqlx::query(
        "SELECT bootstrap_status, external_webhook_id FROM sync_source_runtime WHERE source_id=$1",
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("runtime row");
    assert_eq!(
        row.try_get::<String, _>("bootstrap_status")
            .expect("status"),
        "ready"
    );
    assert!(
        row.try_get::<Option<String>, _>("external_webhook_id")
            .expect("webhook id")
            .is_none()
    );
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 0);
    assert_eq!(netbox.state.patch_count.load(Ordering::SeqCst), 0);
    assert!(netbox.state.webhooks.lock().await.is_empty());
    assert!(netbox.state.event_rules.lock().await.is_empty());
}

struct AuthedApp {
    router: Router,
    state: piping_backend::app::AppState,
    cookie: String,
}

async fn build_authed_app() -> Result<AuthedApp, Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&router, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login failed: no session cookie")?;
    Ok(AuthedApp {
        router,
        state,
        cookie,
    })
}

async fn create_provider(
    app: &Router,
    cookie: &str,
    name: &str,
    config: Value,
    bootstrap: bool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/providers",
        Some(
            &json!({
                "name": name,
                "provider_type": "netbox",
                "config": config,
                "bootstrap": bootstrap
            })
            .to_string(),
        ),
        Some(cookie),
    )
    .await?;
    assert_eq!(
        status,
        axum::http::StatusCode::CREATED,
        "provider creation should succeed, body={}",
        String::from_utf8_lossy(&body)
    );
    let json: Value = serde_json::from_slice(&body)?;
    let id = json
        .get("id")
        .and_then(Value::as_str)
        .ok_or("provider id missing in response")?;
    Ok(Uuid::parse_str(id)?)
}

fn netbox_provider_config(base_url: &str) -> Value {
    netbox_provider_config_custom(base_url, "test-token", "pull_only", vec![1])
}

fn netbox_provider_config_custom(
    base_url: &str,
    api_token: &str,
    sync_mode: &str,
    webhook_object_types: Vec<i32>,
) -> Value {
    json!({
        "base_url": base_url,
        "api_token": api_token,
        "page_size": 100,
        "sync_mode": sync_mode,
        "webhook_secret": "webhook-secret",
        "webhook_events": ["created", "updated", "deleted"],
        "webhook_object_types": webhook_object_types
    })
}

async fn spawn_mock_netbox() -> Result<MockNetboxHandle, Box<dyn std::error::Error + Send + Sync>> {
    let state = MockNetboxState::default();
    let app = Router::new()
        .route(
            "/api/extras/webhooks/",
            get(list_webhooks).post(create_webhook),
        )
        .route("/api/extras/webhooks/{id}/", patch(update_webhook))
        .route(
            "/api/extras/event-rules/",
            get(list_event_rules).post(create_event_rule),
        )
        .route("/api/extras/event-rules/{id}/", patch(update_event_rule))
        .route("/api/extras/object-types/{id}/", get(get_object_type))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    tokio::spawn(async move {
        let served = axum::serve(listener, app).await;
        assert!(served.is_ok(), "mock netbox server failed: {served:?}");
    });

    Ok(MockNetboxHandle {
        base_url: format!("http://{}", addr),
        state,
    })
}

async fn list_webhooks(
    State(state): State<MockNetboxState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let name_filter = params.get("name").cloned();
    let data = state.webhooks.lock().await;
    let results = data
        .iter()
        .filter(|item| {
            if let Some(name_filter) = &name_filter {
                item.get("name").and_then(Value::as_str) == Some(name_filter)
            } else {
                true
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    Json(json!({
        "count": results.len(),
        "results": results,
        "next": null,
        "previous": null
    }))
}

async fn create_webhook(
    State(state): State<MockNetboxState>,
    Json(mut payload): Json<Value>,
) -> impl IntoResponse {
    state.post_count.fetch_add(1, Ordering::SeqCst);
    let mut data = state.webhooks.lock().await;
    let id = data.len() as i64 + 1;
    payload["id"] = json!(id);
    data.push(payload.clone());
    (axum::http::StatusCode::CREATED, Json(payload))
}

async fn update_webhook(
    State(state): State<MockNetboxState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    state.patch_count.fetch_add(1, Ordering::SeqCst);
    let mut data = state.webhooks.lock().await;
    if let Some(index) = data.iter().position(|item| {
        item.get("id")
            .map(|value| value == &json!(id.parse::<i64>().unwrap_or_default()))
            .unwrap_or(false)
    }) {
        let mut merged = data[index].clone();
        if let (Some(base), Some(incoming)) = (merged.as_object_mut(), payload.as_object()) {
            for (key, value) in incoming {
                base.insert(key.clone(), value.clone());
            }
        }
        data[index] = merged.clone();
        return (axum::http::StatusCode::OK, Json(merged)).into_response();
    }
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({"error": "not found"})),
    )
        .into_response()
}

async fn list_event_rules(
    State(state): State<MockNetboxState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let name_filter = params.get("name").cloned();
    let data = state.event_rules.lock().await;
    let results = data
        .iter()
        .filter(|item| {
            if let Some(name_filter) = &name_filter {
                item.get("name").and_then(Value::as_str) == Some(name_filter)
            } else {
                true
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    Json(json!({
        "count": results.len(),
        "results": results,
        "next": null,
        "previous": null
    }))
}

async fn create_event_rule(
    State(state): State<MockNetboxState>,
    Json(mut payload): Json<Value>,
) -> impl IntoResponse {
    let mut data = state.event_rules.lock().await;
    let id = data.len() as i64 + 1;
    payload["id"] = json!(id);
    data.push(payload.clone());
    (axum::http::StatusCode::CREATED, Json(payload))
}

async fn update_event_rule(
    State(state): State<MockNetboxState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let mut data = state.event_rules.lock().await;
    if let Some(index) = data.iter().position(|item| {
        item.get("id")
            .map(|value| value == &json!(id.parse::<i64>().unwrap_or_default()))
            .unwrap_or(false)
    }) {
        let mut merged = data[index].clone();
        if let (Some(base), Some(incoming)) = (merged.as_object_mut(), payload.as_object()) {
            for (key, value) in incoming {
                base.insert(key.clone(), value.clone());
            }
        }
        data[index] = merged.clone();
        return (axum::http::StatusCode::OK, Json(merged)).into_response();
    }
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({"error": "not found"})),
    )
        .into_response()
}

async fn get_object_type(Path(id): Path<String>) -> impl IntoResponse {
    let id = id.parse::<i64>().unwrap_or_default();
    if id == 1 {
        return Json(json!({
            "id": 1,
            "app_label": "dcim",
            "model": "site",
            "value": "dcim.site"
        }))
        .into_response();
    }
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({"error": "not found"})),
    )
        .into_response()
}
