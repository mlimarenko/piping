//! Integration tests for NetBox inbound/outbound source sync flow.
//!
//! Requires PostgreSQL and Redis. Run with:
//! `cargo test --test integration_netbox_source_flow -- --ignored`

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::Request,
    response::IntoResponse,
    routing::{get, patch},
};
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
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn signed_webhook_is_accepted_and_persisted_in_journal()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, secret).await?;
    let created_name = format!("site-created-{}", unique_suffix());
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some(Uuid::now_v7().to_string()),
        "request-flow-1",
        Some(&created_name),
    );

    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("applied"),
        "expected applied for supported model create"
    );

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
    .expect("webhook journal count");
    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(count, 1);

    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM audit_event
        WHERE target_id = $1
          AND event_type IN ('sync.webhook.inbound.accepted', 'sync.webhook.inbound.applied')
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("audit count");
    let audit_count: i64 = row.try_get("count").expect("count");
    assert_eq!(audit_count, 2, "expected accepted+applied audit events");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn duplicate_webhook_delivery_is_deduplicated()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, secret).await?;
    let site_id = Uuid::now_v7().to_string();
    let dedup_name = format!("site-dup-{}", unique_suffix());
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some(site_id),
        "request-flow-2",
        Some(&dedup_name),
    );

    let (status, _) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("deduplicated")
    );

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
    .expect("journal count");
    let count: i64 = row.try_get("count").expect("count");
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn unsupported_model_event_is_marked_ignored_without_inventory_mutation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, secret).await?;

    let before = inventory_site_count(&app.state.db_pool).await?;
    let payload = webhook_payload(
        "updated",
        "ipam.prefix",
        Some(Uuid::now_v7().to_string()),
        "request-flow-3",
        Some("ignored-resource"),
    );

    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ignored"));
    let after = inventory_site_count(&app.state.db_pool).await?;
    assert_eq!(before, after, "unsupported model must not mutate sites");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn supported_update_event_updates_local_site_entity()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, secret).await?;
    let old_name = format!("site-old-name-{}", unique_suffix());
    let new_name = format!("site-new-name-{}", unique_suffix());
    let site_id = create_site(&app.router, &app.cookie, &old_name).await?;

    let payload = webhook_payload(
        "updated",
        "dcim.site",
        Some(site_id.to_string()),
        "request-flow-4",
        Some(&new_name),
    );
    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body.get("status").and_then(Value::as_str), Some("applied"));

    let (status, body) = request(
        &app.router,
        "GET",
        &format!("/api/v1/inventory/sites/{site_id}"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("get site");
    assert_eq!(status, axum::http::StatusCode::OK);
    let site: Value = serde_json::from_slice(&body).expect("site json");
    assert_eq!(
        site.get("name").and_then(Value::as_str),
        Some(new_name.as_str())
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn delete_event_for_absent_local_entity_is_idempotent_ignored()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, secret).await?;

    let payload = webhook_payload(
        "deleted",
        "dcim.site",
        Some(Uuid::now_v7().to_string()),
        "request-flow-5",
        None,
    );
    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ignored"));
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn failed_apply_marks_journal_runtime_and_writes_failed_audit_event()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app.router, &app.cookie, secret).await?;
    let conflict_name = format!("site-conflict-{}", unique_suffix());
    let _ = create_site(&app.router, &app.cookie, &conflict_name).await?;

    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some(Uuid::now_v7().to_string()),
        "request-flow-6",
        Some(&conflict_name),
    );
    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    // A duplicate-name apply failure maps to AppError::Conflict -> 409 with the
    // canonical flat error envelope (error_mapping.rs), not a 500.
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("CONFLICT"),
        "expected canonical error payload, got {body}"
    );

    let row = sqlx::query(
        r#"
        SELECT status, last_error
        FROM sync_webhook_event
        WHERE source_id = $1 AND request_id = 'request-flow-6'
        ORDER BY received_at DESC
        LIMIT 1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("failed webhook event row");
    let webhook_status: String = row.try_get("status").expect("status");
    let last_error: Option<String> = row.try_get("last_error").expect("last_error");
    assert_eq!(webhook_status, "failed");
    assert!(
        last_error.is_some(),
        "expected failure details in webhook row"
    );

    let row = sqlx::query(
        r#"
        SELECT last_error_code
        FROM sync_source_runtime
        WHERE source_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("source runtime row");
    let runtime_error_code: Option<String> =
        row.try_get("last_error_code").expect("last_error_code");
    assert_eq!(runtime_error_code.as_deref(), Some("webhook_apply_failed"));

    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM audit_event
        WHERE target_id = $1
          AND event_type = 'sync.webhook.inbound.failed'
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("failed audit count");
    let failed_audit_count: i64 = row.try_get("count").expect("count");
    assert_eq!(failed_audit_count, 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn bidirectional_site_update_enqueues_outbound_event()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        "http://127.0.0.1:8000",
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;

    let (status, _) = request(
        &app.router,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(
            &json!({
                "name": format!("site-updated-{}", unique_suffix()),
                "status": "active"
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, axum::http::StatusCode::OK);

    let pending = outbox_count(&app.state.db_pool, provider_id, "update", "pending").await?;
    assert_eq!(pending, 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn pull_only_mode_does_not_enqueue_outbound_events()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        "http://127.0.0.1:8000",
        "pull_only",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;

    let (status, _) = request(
        &app.router,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(
            &json!({
                "name": format!("site-updated-{}", unique_suffix()),
                "status": "active"
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, axum::http::StatusCode::OK);

    let total = outbox_count_any_status(&app.state.db_pool, provider_id).await?;
    assert_eq!(total, 0);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn outbound_create_push_succeeds_in_bidirectional_mode()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;

    let _site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let processed = process_outbox(&app, 10).await?;
    assert!(processed >= 1);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);

    let status = latest_outbox_status(&app.state.db_pool, provider_id, "create").await?;
    assert_eq!(status.as_deref(), Some("succeeded"));
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn outbound_update_push_succeeds_in_bidirectional_mode()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;

    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let _ = process_outbox(&app, 10).await?; // process create

    let (status, _) = request(
        &app.router,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(
            &json!({
                "name": format!("site-updated-{}", unique_suffix()),
                "status": "active"
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, axum::http::StatusCode::OK);

    let _ = process_outbox(&app, 10).await?;
    assert!(netbox.state.patch_count.load(Ordering::SeqCst) >= 1);
    let status = latest_outbox_status(&app.state.db_pool, provider_id, "update").await?;
    assert_eq!(status.as_deref(), Some("succeeded"));
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn outbound_delete_push_succeeds_in_bidirectional_mode()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;

    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let _ = process_outbox(&app, 10).await?; // process create

    let (status, _) = request(
        &app.router,
        "DELETE",
        &format!("/api/v1/inventory/sites/{site_id}"),
        None,
        Some(&app.cookie),
    )
    .await
    .expect("delete site");
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);

    let _ = process_outbox(&app, 10).await?;
    assert!(netbox.state.delete_count.load(Ordering::SeqCst) >= 1);
    let status = latest_outbox_status(&app.state.db_pool, provider_id, "delete").await?;
    assert_eq!(status.as_deref(), Some("succeeded"));
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn outbound_transient_failure_retries_then_succeeds()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;

    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let _ = process_outbox(&app, 10).await?; // process create
    netbox.state.fail_patch_remaining.store(1, Ordering::SeqCst);

    let (status, _) = request(
        &app.router,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(
            &json!({
                "name": format!("site-updated-{}", unique_suffix()),
                "status": "active"
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, axum::http::StatusCode::OK);

    let _ = process_outbox(&app, 10).await?;
    let failed = latest_outbox_status(&app.state.db_pool, provider_id, "update").await?;
    assert_eq!(failed.as_deref(), Some("failed"));

    force_next_attempt_now(&app.state.db_pool, provider_id, "update").await?;
    let _ = process_outbox(&app, 10).await?;

    let succeeded = latest_outbox_status(&app.state.db_pool, provider_id, "update").await?;
    assert_eq!(succeeded.as_deref(), Some("succeeded"));
    assert!(netbox.state.patch_count.load(Ordering::SeqCst) >= 2);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn outbound_max_retry_exhaustion_transitions_to_dead_letter()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;
    let max_attempts = app.state.settings.sync_runtime.outbox_retry_max_attempts as i32;

    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let _ = process_outbox(&app, 10).await?; // process create
    netbox
        .state
        .fail_patch_remaining
        .store(max_attempts + 2, Ordering::SeqCst);

    let (status, _) = request(
        &app.router,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(
            &json!({
                "name": format!("site-updated-{}", unique_suffix()),
                "status": "active"
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, axum::http::StatusCode::OK);

    for attempt in 0..max_attempts {
        let _ = process_outbox(&app, 10).await?;
        if attempt + 1 < max_attempts {
            force_next_attempt_now(&app.state.db_pool, provider_id, "update").await?;
        }
    }

    let dead_letter = latest_outbox_status(&app.state.db_pool, provider_id, "update").await?;
    assert_eq!(dead_letter.as_deref(), Some("dead_letter"));

    let row = sqlx::query(
        r#"
        SELECT last_error_code
        FROM sync_source_runtime
        WHERE source_id = $1
        "#,
    )
    .bind(provider_id)
    .fetch_one(&app.state.db_pool)
    .await
    .expect("runtime row");
    let error_code: Option<String> = row.try_get("last_error_code").expect("last_error_code");
    assert_eq!(error_code.as_deref(), Some("outbound_retry_exhausted"));
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn replay_endpoint_requeues_dead_letter_events()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider_id = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        "netbox-secret",
    )
    .await?;
    isolate_provider(&app.state.db_pool, provider_id).await?;
    let max_attempts = app.state.settings.sync_runtime.outbox_retry_max_attempts as i32;

    let site_id = create_site(
        &app.router,
        &app.cookie,
        &format!("site-{}", unique_suffix()),
    )
    .await?;
    let _ = process_outbox(&app, 10).await?; // process create
    netbox
        .state
        .fail_patch_remaining
        .store(max_attempts + 2, Ordering::SeqCst);

    let (status, _) = request(
        &app.router,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(
            &json!({
                "name": format!("site-updated-{}", unique_suffix()),
                "status": "active"
            })
            .to_string(),
        ),
        Some(&app.cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, axum::http::StatusCode::OK);

    for attempt in 0..max_attempts {
        let _ = process_outbox(&app, 10).await?;
        if attempt + 1 < max_attempts {
            force_next_attempt_now(&app.state.db_pool, provider_id, "update").await?;
        }
    }
    let dead_letter = latest_outbox_status(&app.state.db_pool, provider_id, "update").await?;
    assert_eq!(dead_letter.as_deref(), Some("dead_letter"));

    let (status, body) = request(
        &app.router,
        "POST",
        &format!("/api/v1/providers/{provider_id}/outbox/replay"),
        Some(&json!({"limit": 10}).to_string()),
        Some(&app.cookie),
    )
    .await
    .expect("replay");
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    let replay: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(replay.get("accepted").and_then(Value::as_bool), Some(true));
    assert!(
        replay
            .get("replayed_events")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );

    netbox.state.fail_patch_remaining.store(0, Ordering::SeqCst);
    force_next_attempt_now(&app.state.db_pool, provider_id, "update").await?;
    let _ = process_outbox(&app, 10).await?;

    let succeeded = latest_outbox_status(&app.state.db_pool, provider_id, "update").await?;
    assert_eq!(succeeded.as_deref(), Some("succeeded"));
    Ok(())
}

struct AuthedApp {
    router: Router,
    state: piping_backend::app::AppState,
    cookie: String,
    _suite_guard: tokio::sync::OwnedMutexGuard<()>,
}

async fn build_authed_app() -> Result<AuthedApp, Box<dyn std::error::Error + Send + Sync>> {
    let suite_guard = netbox_flow_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&router, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "login failed".to_string())?;
    Ok(AuthedApp {
        router,
        state,
        cookie,
        _suite_guard: suite_guard,
    })
}

fn netbox_flow_test_lock() -> &'static Arc<tokio::sync::Mutex<()>> {
    static LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

async fn create_provider_with_secret(
    app: &Router,
    cookie: &str,
    secret: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    create_provider(app, cookie, "http://127.0.0.1:8000", "pull_only", secret).await
}

async fn create_provider(
    app: &Router,
    cookie: &str,
    base_url: &str,
    sync_mode: &str,
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
                    "base_url": base_url,
                    "api_token": "test-token",
                    "page_size": 100,
                    "sync_mode": sync_mode,
                    "webhook_secret": secret,
                    "webhook_object_types": [1]
                },
                "bootstrap": false
            })
            .to_string(),
        ),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let json: Value = serde_json::from_slice(&body)?;
    let id = json
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider id missing from response".to_string())?;
    Ok(Uuid::parse_str(id)?)
}

async fn process_outbox(
    app: &AuthedApp,
    limit: usize,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    app.state
        .services
        .outbound_service
        .process_batch(limit)
        .await
        .map_err(|e| format!("process outbox: {e}").into())
}

async fn isolate_provider(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        UPDATE sync_provider
        SET is_enabled = (id = $1)
        WHERE provider_type = 'netbox'
        "#,
    )
    .bind(provider_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_site(
    app: &Router,
    cookie: &str,
    name: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/inventory/sites",
        Some(
            &json!({
                "name": name,
                "status": "active"
            })
            .to_string(),
        ),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let json: Value = serde_json::from_slice(&body)?;
    let id = json
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "site id missing from response".to_string())?;
    Ok(Uuid::parse_str(id)?)
}

async fn send_signed_webhook(
    app: &Router,
    provider_id: Uuid,
    payload: &Value,
    secret: &str,
) -> Result<(axum::http::StatusCode, Value), Box<dyn std::error::Error + Send + Sync>> {
    let body = payload.to_string();
    let signature = sign_payload(body.as_bytes(), secret)?;
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/webhooks/netbox/{provider_id}"))
        .header("content-type", "application/json")
        .header("x-hook-signature", signature)
        .body(Body::from(body))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let body_json = serde_json::from_slice(&bytes)?;
    Ok((status, body_json))
}

fn webhook_payload(
    event: &str,
    model: &str,
    site_id: Option<String>,
    request_id: &str,
    name: Option<&str>,
) -> Value {
    let mut data = json!({
        "name": name.unwrap_or("site-from-webhook")
    });
    if let Some(site_id) = site_id {
        data["id"] = Value::String(site_id);
    }
    json!({
        "event": event,
        "timestamp": "2026-02-11T12:00:00Z",
        "model": model,
        "request_id": request_id,
        "data": data,
        "snapshots": {}
    })
}

async fn inventory_site_count(
    pool: &sqlx::PgPool,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM dcim_site")
        .fetch_one(pool)
        .await?;
    Ok(row.try_get("count")?)
}

async fn outbox_count(
    pool: &sqlx::PgPool,
    source_id: Uuid,
    operation: &str,
    status: &str,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM sync_outbound_event
        WHERE source_id = $1
          AND operation = $2
          AND status = $3
        "#,
    )
    .bind(source_id)
    .bind(operation)
    .bind(status)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get("count")?)
}

async fn outbox_count_any_status(
    pool: &sqlx::PgPool,
    source_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS count
        FROM sync_outbound_event
        WHERE source_id = $1
        "#,
    )
    .bind(source_id)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get("count")?)
}

async fn latest_outbox_status(
    pool: &sqlx::PgPool,
    source_id: Uuid,
    operation: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        r#"
        SELECT status
        FROM sync_outbound_event
        WHERE source_id = $1
          AND operation = $2
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(source_id)
    .bind(operation)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|row| row.try_get("status").ok()))
}

async fn force_next_attempt_now(
    pool: &sqlx::PgPool,
    source_id: Uuid,
    operation: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        UPDATE sync_outbound_event
        SET next_attempt_at = NOW()
        WHERE source_id = $1
          AND operation = $2
          AND status = 'failed'
        "#,
    )
    .bind(source_id)
    .bind(operation)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Clone, Default)]
struct MockNetboxState {
    sites: Arc<tokio::sync::Mutex<Vec<Value>>>,
    next_id: Arc<AtomicUsize>,
    post_count: Arc<AtomicUsize>,
    patch_count: Arc<AtomicUsize>,
    delete_count: Arc<AtomicUsize>,
    fail_post_remaining: Arc<AtomicI32>,
    fail_patch_remaining: Arc<AtomicI32>,
    fail_delete_remaining: Arc<AtomicI32>,
}

struct MockNetboxHandle {
    base_url: String,
    state: MockNetboxState,
}

async fn spawn_mock_netbox() -> Result<MockNetboxHandle, Box<dyn std::error::Error + Send + Sync>> {
    let state = MockNetboxState::default();
    let app = Router::new()
        .route("/api/schema/", get(mock_schema))
        .route("/dcim/sites/", get(mock_list_sites).post(mock_create_site))
        .route(
            "/dcim/sites/{id}/",
            patch(mock_update_site).delete(mock_delete_site),
        )
        .route(
            "/api/dcim/sites/",
            get(mock_list_sites).post(mock_create_site),
        )
        .route(
            "/api/dcim/sites/{id}/",
            patch(mock_update_site).delete(mock_delete_site),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        #[allow(
            clippy::expect_used,
            reason = "detached background task backing a test-only mock server; a serve failure has no caller left to propagate to, so panicking the task is the correct failure mode"
        )]
        axum::serve(listener, app).await.expect("serve mock netbox");
    });

    Ok(MockNetboxHandle {
        base_url: format!("http://{}", addr),
        state,
    })
}

async fn mock_schema() -> impl IntoResponse {
    Json(json!({
        "openapi": "3.0.0",
        "paths": {
            "/api/dcim/sites/": {
                "get": {
                    "parameters": [
                        { "name": "limit", "in": "query", "schema": { "type": "integer" } },
                        { "name": "offset", "in": "query", "schema": { "type": "integer" } },
                        { "name": "ordering", "in": "query", "schema": { "type": "string" } },
                        { "name": "name", "in": "query", "schema": { "type": "string" } },
                        { "name": "slug", "in": "query", "schema": { "type": "string" } }
                    ],
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PaginatedSiteList" } } } }
                    }
                },
                "post": {
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SiteRequest" }
                            }
                        }
                    },
                    "responses": {
                        "201": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Site" } } } }
                    }
                }
            },
            "/api/dcim/sites/{id}/": {
                "patch": {
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SiteRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Site" } } } }
                    }
                },
                "delete": {
                    "responses": { "204": { "description": "No content" } }
                }
            }
        },
        "components": {
            "schemas": {
                "Site": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" },
                        "slug": { "type": "string" }
                    }
                },
                "SiteRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "slug": { "type": "string" }
                    }
                },
                "PaginatedSiteList": {
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer" },
                        "results": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/Site" }
                        }
                    }
                }
            }
        }
    }))
}

async fn mock_list_sites(
    State(state): State<MockNetboxState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let sites = state.sites.lock().await;
    let mut results = sites.clone();
    if let Some(slug) = params.get("slug") {
        results.retain(|site| site.get("slug").and_then(Value::as_str) == Some(slug.as_str()));
    }
    if let Some(name) = params.get("name") {
        results.retain(|site| site.get("name").and_then(Value::as_str) == Some(name.as_str()));
    }
    Json(json!({
        "count": results.len(),
        "results": results,
        "next": null,
        "previous": null
    }))
}

async fn mock_create_site(
    State(state): State<MockNetboxState>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if consume_failure(&state.fail_post_remaining) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "temporary create failure"})),
        )
            .into_response();
    }
    state.post_count.fetch_add(1, Ordering::SeqCst);

    let mut site = payload.as_object().cloned().unwrap_or_default();
    let id = state.next_id.fetch_add(1, Ordering::SeqCst) as i64 + 1;
    site.insert("id".to_string(), json!(id));
    if !site.contains_key("slug") {
        let slug = site
            .get("name")
            .and_then(Value::as_str)
            .map(|name| name.to_ascii_lowercase().replace(' ', "-"))
            .unwrap_or_else(|| format!("site-{id}"));
        site.insert("slug".to_string(), json!(slug));
    }
    let value = Value::Object(site);
    state.sites.lock().await.push(value.clone());
    (axum::http::StatusCode::CREATED, Json(value)).into_response()
}

async fn mock_update_site(
    State(state): State<MockNetboxState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    state.patch_count.fetch_add(1, Ordering::SeqCst);
    if consume_failure(&state.fail_patch_remaining) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "temporary update failure"})),
        )
            .into_response();
    }

    let target_id: i64 = id.parse().unwrap_or_default();
    let mut sites = state.sites.lock().await;
    let Some(index) = sites
        .iter()
        .position(|site| site.get("id").and_then(Value::as_i64) == Some(target_id))
    else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": "site not found"})),
        )
            .into_response();
    };

    let mut merged = sites[index].clone();
    if let (Some(base), Some(incoming)) = (merged.as_object_mut(), payload.as_object()) {
        for (key, value) in incoming {
            base.insert(key.clone(), value.clone());
        }
    }
    sites[index] = merged.clone();
    (axum::http::StatusCode::OK, Json(merged)).into_response()
}

async fn mock_delete_site(
    State(state): State<MockNetboxState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if consume_failure(&state.fail_delete_remaining) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "temporary delete failure"})),
        )
            .into_response();
    }
    state.delete_count.fetch_add(1, Ordering::SeqCst);
    let target_id: i64 = id.parse().unwrap_or_default();
    let mut sites = state.sites.lock().await;
    let before = sites.len();
    sites.retain(|site| site.get("id").and_then(Value::as_i64) != Some(target_id));
    if sites.len() == before {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": "site not found"})),
        )
            .into_response();
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

fn consume_failure(counter: &Arc<AtomicI32>) -> bool {
    loop {
        let current = counter.load(Ordering::SeqCst);
        if current <= 0 {
            return false;
        }
        if counter
            .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return true;
        }
    }
}

fn sign_payload(
    payload: &[u8],
    secret: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key = PKey::hmac(secret.as_bytes())?;
    let mut signer = Signer::new(MessageDigest::sha512(), &key)?;
    signer.update(payload)?;
    let signature = signer.sign_to_vec()?;
    Ok(signature
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}
