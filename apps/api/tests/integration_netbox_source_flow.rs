//! Integration tests for NetBox inbound/outbound source sync flow.
//!
//! Requires PostgreSQL and Redis. Run with:
//! `cargo test --test integration_netbox_source_flow -- --ignored`

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::Request,
    response::IntoResponse,
    routing::get,
};
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use piping_backend::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, InventoryEntity, MappingConfiguration,
    MappingConfigurationStatus, MappingDirection, MappingTarget, OutboundEvent,
    OutboundEventStatus,
};
use piping_backend::contexts::sync::infrastructure::persistence::MappingConfigurationRepository;
use serde_json::{Value, json};
use sqlx::{Connection, Row};
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
    let provider_id = create_provider_with_secret(&app, secret).await?;
    let created_name = format!("site-created-{}", unique_suffix());
    app.inbound_mock.put_site(8104, &created_name).await;
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some("8104".to_string()),
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
    let provider_id = create_provider_with_secret(&app, secret).await?;
    let site_id = "8105".to_string();
    let dedup_name = format!("site-dup-{}", unique_suffix());
    app.inbound_mock.put_site(8105, &dedup_name).await;
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
async fn unmapped_model_event_is_marked_ignored_without_inventory_mutation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app, secret).await?;

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
    let unknown = webhook_payload(
        "updated",
        "dcim.unknown_model",
        Some("123".into()),
        "unknown-model",
        None,
    );
    let (status, body) = send_signed_webhook(&app.router, provider_id, &unknown, secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "ignored");
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn provider_without_active_mapping_ignores_webhook()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = format!("no-mapping-{}", unique_suffix());
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &app.inbound_mock.base_url,
        "bidirectional",
        &secret,
    )
    .await?;
    let name = format!("unmapped-{}", unique_suffix());
    app.inbound_mock.put_site(8120, &name).await;
    let before = inventory_site_count(&app.state.db_pool).await?;
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some("8120".into()),
        "no-active-mapping",
        Some(&name),
    );
    let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "ignored");
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn supported_update_event_updates_local_site_entity()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app, secret).await?;
    let old_name = format!("site-old-name-{}", unique_suffix());
    let new_name = format!("site-new-name-{}", unique_suffix());
    app.inbound_mock.put_site(41, &new_name).await;
    {
        let mut sites = app.inbound_mock.state.sites.lock().await;
        let record = sites
            .iter_mut()
            .find(|record| record["id"] == 41)
            .ok_or("mock site missing")?;
        record["description"] = json!("retained description");
        record["comments"] = json!("retained comment");
    }
    let site_id = create_site(&app.router, &app.cookie, &old_name).await?;
    let other_name = format!("unrelated-site-{}", unique_suffix());
    let other_id = create_site(&app.router, &app.cookie, &other_name).await?;
    let job_id = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_job(id,provider_id,mapping_version,status) VALUES ($1,$2,'identity-fixture','completed')")
        .bind(job_id).bind(provider_id).execute(&app.state.db_pool).await?;
    seed_site_binding(&app.state.db_pool, provider_id, "41", site_id, Some(job_id)).await?;
    sqlx::query("UPDATE dcim_site SET description='retained description',comments='retained comment' WHERE id=$1")
        .bind(site_id).execute(&app.state.db_pool).await?;

    let mut payload = webhook_payload(
        "updated",
        "dcim.site",
        Some("41".to_string()),
        "request-flow-4",
        Some("payload-name-must-not-be-used"),
    );
    payload["data"]["local_id"] = json!(other_id);
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
    assert_eq!(site["description"], "retained description");
    assert_eq!(site["comments"], "retained comment");
    let binding = sqlx::query("SELECT local_id,last_seen_job_id FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity='dcim.sites' AND external_id='41' AND target_entity='piping.inventory.site'")
        .bind(provider_id).fetch_one(&app.state.db_pool).await?;
    assert_eq!(binding.try_get::<Uuid, _>("local_id")?, site_id);
    let last_seen = binding.try_get::<Option<Uuid>, _>("last_seen_job_id")?;
    assert!(last_seen.is_some());
    assert_ne!(
        last_seen,
        Some(job_id),
        "live commit records its own sync job"
    );
    let other: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id=$1")
        .bind(other_id)
        .fetch_one(&app.state.db_pool)
        .await?;
    assert_eq!(
        other, other_name,
        "payload local_id is not an identity authority"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn unbound_webhook_cannot_adopt_a_local_site_by_uuid_or_name()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = Uuid::now_v7().to_string();
    let provider = create_provider_with_secret(&app, &secret).await?;
    let name = format!("local-only-{}", unique_suffix());
    let site = create_site(&app.router, &app.cookie, &name).await?;
    let remote_name = format!("remote-only-{}", unique_suffix());
    app.inbound_mock.put_site(8103, &remote_name).await;
    for (remote_id, expected) in [
        ("8103".to_string(), axum::http::StatusCode::ACCEPTED),
        (site.to_string(), axum::http::StatusCode::BAD_REQUEST),
    ] {
        let mut payload = webhook_payload(
            "updated",
            "dcim.site",
            Some(remote_id),
            "unbound-identity",
            Some("must-not-overwrite"),
        );
        payload["data"]["local_id"] = json!(site);
        let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
        assert_eq!(status, expected);
        if expected == axum::http::StatusCode::ACCEPTED {
            assert_eq!(body["status"], "applied");
        }
    }
    let original_slug: Option<String> =
        sqlx::query_scalar("SELECT slug FROM dcim_site WHERE id=$1")
            .bind(site)
            .fetch_one(&app.state.db_pool)
            .await?;
    let mut payload = webhook_payload(
        "updated",
        "dcim.site",
        Some("8108".to_string()),
        "natural-key-adoption",
        Some(&name),
    );
    app.inbound_mock.put_site(8108, &name).await;
    payload["data"]["slug"] = json!("must-not-adopt-by-name");
    let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
    let retained_slug: Option<String> =
        sqlx::query_scalar("SELECT slug FROM dcim_site WHERE id=$1")
            .bind(site)
            .fetch_one(&app.state.db_pool)
            .await?;
    assert_eq!(retained_slug, original_slug);
    let retained: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id=$1")
        .bind(site)
        .fetch_one(&app.state.db_pool)
        .await?;
    assert_eq!(retained, name);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn webhook_equal_remote_ids_are_provider_qualified_and_delete_only_their_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = Uuid::now_v7().to_string();
    let first = create_provider_with_secret(&app, &secret).await?;
    let second = create_provider_with_secret(&app, &secret).await?;
    let mut ids = Vec::new();
    for (provider, prefix) in [(first, "first"), (second, "second")] {
        let name = format!("{prefix}-{}", unique_suffix());
        app.inbound_mock.put_site(8100, &name).await;
        let payload = webhook_payload(
            "created",
            "dcim.site",
            Some("8100".to_string()),
            "same-remote-id",
            Some(&name),
        );
        let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
        assert_eq!(status, axum::http::StatusCode::ACCEPTED);
        assert_eq!(body["status"], "applied");
        let id: Uuid = sqlx::query_scalar("SELECT local_id FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity='dcim.sites' AND external_id='8100' AND target_entity='piping.inventory.site'")
            .bind(provider).fetch_one(&app.state.db_pool).await?;
        ids.push(id);
    }
    assert_ne!(ids[0], ids[1]);
    let payload = webhook_payload(
        "deleted",
        "dcim.site",
        Some("8100".to_string()),
        "delete-first-binding",
        None,
    );
    let (status, body) = send_signed_webhook(&app.router, first, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "applied");
    assert_eq!(site_binding_count(&app.state.db_pool, first).await?, 0);
    assert_eq!(site_binding_count(&app.state.db_pool, second).await?, 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_site WHERE id=ANY($1)")
        .bind(&ids)
        .fetch_one(&app.state.db_pool)
        .await?;
    assert_eq!(count, 1);
    let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT FROM dcim_site WHERE id=$1)")
        .bind(ids[1])
        .fetch_one(&app.state.db_pool)
        .await?;
    assert!(retained);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn guarded_webhook_delete_rolls_back_row_and_binding_together()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = Uuid::now_v7().to_string();
    let provider = create_provider_with_secret(&app, &secret).await?;
    let name = format!("protected-site-{}", unique_suffix());
    let site = create_site(&app.router, &app.cookie, &name).await?;
    seed_site_binding(&app.state.db_pool, provider, "8101", site, None).await?;
    let location = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_location(id,custom_field_data,name,slug,description,lft,rght,tree_id,level,site_id,status,facility,comments) VALUES ($1,'{}',$3,$3,'',0,0,0,0,$2,'active','','')")
        .bind(location).bind(site).bind(format!("location-{location}")).execute(&app.state.db_pool).await?;
    let payload = webhook_payload(
        "deleted",
        "dcim.site",
        Some("8101".to_string()),
        "protected-delete",
        None,
    );
    let (status, _) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 1);
    let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT FROM dcim_site WHERE id=$1)")
        .bind(site)
        .fetch_one(&app.state.db_pool)
        .await?;
    assert!(retained);
    sqlx::query("DELETE FROM dcim_location WHERE id=$1")
        .bind(location)
        .execute(&app.state.db_pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn provider_lease_contention_does_not_poison_webhook_deduplication()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = Uuid::now_v7().to_string();
    let provider = create_provider_with_secret(&app, &secret).await?;
    let owner = hold_provider_lease(&app, provider).await?;
    let name = format!("lease-site-{}", unique_suffix());
    app.inbound_mock.put_site(8102, &name).await;
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some("8102".to_string()),
        "held-provider-lease",
        Some(&name),
    );
    let before = inventory_site_count(&app.state.db_pool).await?;
    let (status, _) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before);
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_webhook_event WHERE source_id=$1 AND request_id='held-provider-lease'")
        .bind(provider).fetch_one(&app.state.db_pool).await?;
    assert_eq!(
        receipts, 0,
        "contention must not persist a failed/deduplicating receipt"
    );
    release_provider_lease(&app, provider, &owner).await?;
    let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "applied");
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn accepted_webhook_receipt_without_application_is_retryable()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use piping_backend::contexts::sync::infrastructure::persistence::repositories::repository::WebhookEventRepository;

    let app = build_authed_app().await?;
    let secret = Uuid::now_v7().to_string();
    let provider = create_provider_with_secret(&app, &secret).await?;
    let name = format!("accepted-recovery-{}", unique_suffix());
    app.inbound_mock.put_site(8113, &name).await;
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some("8113".to_string()),
        "accepted-but-not-applied",
        Some(&name),
    );
    let bytes = serde_json::to_vec(&payload)?;
    let event = app
        .state
        .services
        .webhook_service
        .normalize_payload(provider, &bytes, &payload)?;
    app.state
        .repositories
        .sync_webhook_events
        .create(&event)
        .await?;
    let before = inventory_site_count(&app.state.db_pool).await?;
    let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "applied");
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before + 1);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 1);
    let receipt: Uuid = sqlx::query_scalar("SELECT id FROM sync_webhook_event WHERE source_id=$1 AND request_id='accepted-but-not-applied' AND status='applied'")
        .bind(provider).fetch_one(&app.state.db_pool).await?;
    assert_eq!(receipt, event.id);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn webhook_lost_lease_after_journaling_is_applied_on_redelivery()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = Uuid::now_v7().to_string();
    let provider = create_provider_with_secret(&app, &secret).await?;
    let name = format!("lease-recovery-{}", unique_suffix());
    app.inbound_mock.put_site(8112, &name).await;
    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some("8112".to_string()),
        "lost-lease-after-journal",
        Some(&name),
    );
    let before = inventory_site_count(&app.state.db_pool).await?;

    // Block the inventory transaction, not the earlier durable journal insert.
    let mut connection = sqlx::PgConnection::connect(&common::database_url()).await?;
    let mut blocker = connection.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(provider.to_string())
        .execute(&mut *blocker)
        .await?;
    let successor = Uuid::now_v7().to_string();
    let interrupt_after_receipt = async {
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(10));
            loop {
                ticks.tick().await;
                let receipt: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM sync_webhook_event WHERE source_id=$1 AND request_id='lost-lease-after-journal' AND status='accepted'",
                )
                .bind(provider)
                .fetch_optional(&app.state.db_pool)
                .await?;
                if let Some(receipt) = receipt {
                    break Ok::<Uuid, sqlx::Error>(receipt);
                }
            }
        })
        .await??;
        let mut redis = app.state.redis.clone();
        let replaced: String = redis::cmd("SET")
            .arg(format!("sync:provider:{provider}"))
            .arg(&successor)
            .arg("EX")
            .arg(120)
            .query_async(&mut redis)
            .await?;
        assert_eq!(replaced, "OK");
        blocker.commit().await?;
        Ok::<Uuid, Box<dyn std::error::Error + Send + Sync>>(receipt)
    };
    let (response, receipt) = tokio::join!(
        send_signed_webhook(&app.router, provider, &payload, &secret),
        interrupt_after_receipt,
    );
    let receipt = receipt?;
    assert_eq!(response?.0, axum::http::StatusCode::CONFLICT);
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    release_provider_lease(&app, provider, &successor).await?;

    // A receipt with another body must never authorize a retry of this delivery.
    let mut different_payload = payload.clone();
    different_payload["data"]["name"] = json!("must-not-replace-original-delivery");
    let (status, _) =
        send_signed_webhook(&app.router, provider, &different_payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);

    let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "applied");
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before + 1);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 1);
    let row = sqlx::query(
        "SELECT id,status,last_error FROM sync_webhook_event WHERE source_id=$1 AND request_id='lost-lease-after-journal'",
    )
    .bind(provider)
    .fetch_one(&app.state.db_pool)
    .await?;
    assert_eq!(row.try_get::<Uuid, _>("id")?, receipt);
    assert_eq!(row.try_get::<String, _>("status")?, "applied");
    assert_eq!(row.try_get::<Option<String>, _>("last_error")?, None);
    let (status, body) = send_signed_webhook(&app.router, provider, &payload, &secret).await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(body["status"], "deduplicated");
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before + 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_netbox_source_flow -- --ignored
async fn delete_event_for_absent_local_entity_is_idempotent_ignored()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let secret = "netbox-secret";
    let provider_id = create_provider_with_secret(&app, secret).await?;

    let payload = webhook_payload(
        "deleted",
        "dcim.site",
        Some("8106".to_string()),
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
    let provider_id = create_provider_with_secret(&app, secret).await?;
    let conflict_name = format!("site-conflict-{}", unique_suffix());
    let _ = create_site(&app.router, &app.cookie, &conflict_name).await?;
    app.inbound_mock.put_site(8107, &conflict_name).await;

    let payload = webhook_payload(
        "created",
        "dcim.site",
        Some("8107".to_string()),
        "request-flow-6",
        Some(&conflict_name),
    );
    let (status, body) = send_signed_webhook(&app.router, provider_id, &payload, secret).await?;
    // The live planner reports target natural-key collisions as validation findings.
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("VALIDATION_ERROR"),
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

    seed_site_binding(&app.state.db_pool, provider_id, "8111", site_id, None).await?;

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

    let site_id = create_site(
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
    let (bound_local, remote): (Uuid, String) = sqlx::query_as("SELECT local_id,external_id FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity='dcim.sites' AND target_entity='piping.inventory.site'")
        .bind(provider_id).fetch_one(&app.state.db_pool).await?;
    assert_eq!(bound_local, site_id);
    assert_eq!(
        remote, "1",
        "the returned remote identity must be committed with success"
    );
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
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn outbound_delete_of_absent_remote_finalizes_the_exact_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let site = create_site(
        &app.router,
        &app.cookie,
        &format!("remote-already-absent-{}", unique_suffix()),
    )
    .await?;
    let netbox = spawn_mock_netbox().await?;
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        &Uuid::now_v7().to_string(),
    )
    .await?;
    seed_site_binding(&app.state.db_pool, provider, "9999", site, None).await?;
    let (status, _) = request(
        &app.router,
        "DELETE",
        &format!("/api/v1/inventory/sites/{site}"),
        None,
        Some(&app.cookie),
    )
    .await?;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
    assert_eq!(process_outbox(&app, 10).await?, 1);
    assert_eq!(netbox.state.delete_count.load(Ordering::SeqCst), 1);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 0);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    assert_eq!(
        latest_outbox_status(&app.state.db_pool, provider, "delete")
            .await?
            .as_deref(),
        Some("succeeded")
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn local_create_rejects_ambiguous_destinations_before_persistence()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    for _ in 0..2 {
        create_provider(
            &app.router,
            &app.cookie,
            "http://127.0.0.1:8000",
            "bidirectional",
            &Uuid::now_v7().to_string(),
        )
        .await?;
    }
    let before = inventory_site_count(&app.state.db_pool).await?;
    let (status, _) = request(
        &app.router,
        "POST",
        "/api/v1/inventory/sites",
        Some(
            &json!({"name":format!("ambiguous-{}",unique_suffix()),"status":"active"}).to_string(),
        ),
        Some(&app.cookie),
    )
    .await?;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(inventory_site_count(&app.state.db_pool).await?, before);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn bound_site_updates_route_to_each_authoritative_provider()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let mut sites = Vec::new();
    for prefix in ["bound-first", "bound-second"] {
        sites.push(
            create_site(
                &app.router,
                &app.cookie,
                &format!("{prefix}-{}", unique_suffix()),
            )
            .await?,
        );
    }
    let mut providers = Vec::new();
    for _ in 0..2 {
        providers.push(
            create_provider(
                &app.router,
                &app.cookie,
                "http://127.0.0.1:8000",
                "bidirectional",
                &Uuid::now_v7().to_string(),
            )
            .await?,
        );
    }
    for index in 0..2 {
        seed_site_binding(
            &app.state.db_pool,
            providers[index],
            &format!("{}", index + 1),
            sites[index],
            None,
        )
        .await?;
        let (status, _) = request(
            &app.router,
            "PUT",
            &format!("/api/v1/inventory/sites/{}", sites[index]),
            Some(
                &json!({"name":format!("renamed-{index}-{}",unique_suffix()),"status":"active"})
                    .to_string(),
            ),
            Some(&app.cookie),
        )
        .await?;
        assert_eq!(status, axum::http::StatusCode::OK);
    }
    for provider in providers {
        assert_eq!(
            outbox_count(&app.state.db_pool, provider, "update", "pending").await?,
            1
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn missing_outbound_binding_never_uses_payload_remote_id_or_fallback_post()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let site = create_site(
        &app.router,
        &app.cookie,
        &format!("unbound-outbound-{}", unique_suffix()),
    )
    .await?;
    let netbox = spawn_mock_netbox().await?;
    netbox
        .state
        .sites
        .lock()
        .await
        .push(json!({"id":1,"name":"remote victim","slug":"remote-victim"}));
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        &Uuid::now_v7().to_string(),
    )
    .await?;
    for operation in ["update", "delete"] {
        queue_site_event(
            &app,
            provider,
            operation,
            site,
            json!({"remote_id":1,"id":1,"name":"must-not-adopt","slug":"remote-victim"}),
        )
        .await?;
        assert!(process_outbox(&app, 10).await? >= 1);
        let status = latest_outbox_status(&app.state.db_pool, provider, operation).await?;
        assert!(
            matches!(status.as_deref(), Some("failed" | "dead_letter")),
            "missing binding must fail, got {status:?}"
        );
    }
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 0);
    assert_eq!(netbox.state.patch_count.load(Ordering::SeqCst), 0);
    assert_eq!(netbox.state.delete_count.load(Ordering::SeqCst), 0);
    assert_eq!(netbox.state.sites.lock().await[0]["name"], "remote victim");
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn concurrent_workers_and_duplicate_create_do_not_repeat_post()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        &Uuid::now_v7().to_string(),
    )
    .await?;
    let site = create_site(
        &app.router,
        &app.cookie,
        &format!("one-post-{}", unique_suffix()),
    )
    .await?;
    let (first, second) = tokio::join!(process_outbox(&app, 10), process_outbox(&app, 10));
    assert_eq!(
        first? + second?,
        1,
        "only a successfully claimed pending event counts as processed"
    );
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    queue_site_event(
        &app,
        provider,
        "create",
        site,
        json!({"name":"duplicate-producer","slug":"duplicate-producer"}),
    )
    .await?;
    assert_eq!(process_outbox(&app, 10).await?, 1);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    assert_eq!(netbox.state.sites.lock().await.len(), 1);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 1);
    assert_eq!(
        latest_outbox_status(&app.state.db_pool, provider, "create")
            .await?
            .as_deref(),
        Some("succeeded")
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn provider_lease_contention_keeps_outbound_pending_without_http()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        &Uuid::now_v7().to_string(),
    )
    .await?;
    create_site(
        &app.router,
        &app.cookie,
        &format!("held-outbound-{}", unique_suffix()),
    )
    .await?;
    let owner = hold_provider_lease(&app, provider).await?;
    assert_eq!(process_outbox(&app, 10).await?, 0);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        latest_outbox_status(&app.state.db_pool, provider, "create")
            .await?
            .as_deref(),
        Some("pending")
    );
    let attempts: i32 = sqlx::query_scalar(
        "SELECT attempts FROM sync_outbound_event WHERE source_id=$1 AND operation='create'",
    )
    .bind(provider)
    .fetch_one(&app.state.db_pool)
    .await?;
    assert_eq!(attempts, 0);
    release_provider_lease(&app, provider, &owner).await?;
    assert_eq!(process_outbox(&app, 10).await?, 1);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn uncertain_remote_create_cannot_repeat_post_after_replay_or_duplicate_producer()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        &Uuid::now_v7().to_string(),
    )
    .await?;
    let site = create_site(
        &app.router,
        &app.cookie,
        &format!("uncertain-create-{}", unique_suffix()),
    )
    .await?;
    netbox
        .state
        .fail_after_create_remaining
        .store(1, Ordering::SeqCst);
    assert_eq!(process_outbox(&app, 10).await?, 1);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        netbox.state.sites.lock().await.len(),
        1,
        "the remote write happened before its error response"
    );
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    assert_eq!(
        latest_outbox_status(&app.state.db_pool, provider, "create")
            .await?
            .as_deref(),
        Some("dead_letter")
    );
    let attempts: i32 = sqlx::query_scalar(
        "SELECT attempts FROM sync_outbound_event WHERE source_id=$1 AND operation='create'",
    )
    .bind(provider)
    .fetch_one(&app.state.db_pool)
    .await?;
    assert!(attempts > 0, "uncertainty must be durable before replay");
    let (status, _) = request(
        &app.router,
        "POST",
        &format!("/api/v1/providers/{provider}/outbox/replay"),
        Some(&json!({"limit":10}).to_string()),
        Some(&app.cookie),
    )
    .await?;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    let _ = process_outbox(&app, 10).await?;
    queue_site_event(
        &app,
        provider,
        "create",
        site,
        json!({"name":"must-not-create-twice","slug":"must-not-create-twice"}),
    )
    .await?;
    assert!(process_outbox(&app, 10).await? >= 1);
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    assert_eq!(netbox.state.sites.lock().await.len(), 1);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    assert_eq!(
        latest_outbox_status(&app.state.db_pool, provider, "create")
            .await?
            .as_deref(),
        Some("dead_letter")
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn lost_provider_lease_prevents_outbound_binding_finalization()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_authed_app().await?;
    let netbox = spawn_mock_netbox().await?;
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &netbox.base_url,
        "bidirectional",
        &Uuid::now_v7().to_string(),
    )
    .await?;
    create_site(
        &app.router,
        &app.cookie,
        &format!("lost-lease-{}", unique_suffix()),
    )
    .await?;
    netbox
        .state
        .pause_create_response
        .store(true, Ordering::SeqCst);
    let replacement_owner = Uuid::now_v7().to_string();
    let replace_lease = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            netbox.state.create_started.notified(),
        )
        .await?;
        let mut redis = app.state.redis.clone();
        let replaced: String = redis::cmd("SET")
            .arg(format!("sync:provider:{provider}"))
            .arg(&replacement_owner)
            .arg("EX")
            .arg(120)
            .query_async(&mut redis)
            .await?;
        assert_eq!(replaced, "OK");
        netbox.state.allow_create_response.notify_one();
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (processed, replaced) = tokio::join!(process_outbox(&app, 10), replace_lease);
    replaced?;
    processed?;
    assert_eq!(netbox.state.post_count.load(Ordering::SeqCst), 1);
    assert_eq!(site_binding_count(&app.state.db_pool, provider).await?, 0);
    assert_ne!(
        latest_outbox_status(&app.state.db_pool, provider, "create")
            .await?
            .as_deref(),
        Some("succeeded")
    );
    let mut redis = app.state.redis.clone();
    let retained: String = redis::cmd("GET")
        .arg(format!("sync:provider:{provider}"))
        .query_async(&mut redis)
        .await?;
    assert_eq!(
        retained, replacement_owner,
        "the stale worker cannot release its successor's lease"
    );
    release_provider_lease(&app, provider, &replacement_owner).await?;
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
    inbound_mock: MockNetboxHandle,
    _suite_guard: tokio::sync::OwnedMutexGuard<()>,
}

async fn build_authed_app() -> Result<AuthedApp, Box<dyn std::error::Error + Send + Sync>> {
    let suite_guard = netbox_flow_test_lock().clone().lock_owned().await;
    let state = build_test_app_state().await?;
    // This serialized flow fixture owns its disposable queue, including claim counts.
    sqlx::query("DELETE FROM sync_outbound_event")
        .execute(&state.db_pool)
        .await?;
    sqlx::query("UPDATE sync_provider SET is_enabled=false WHERE provider_type='netbox'")
        .execute(&state.db_pool)
        .await?;
    let router = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&router, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "login failed".to_string())?;
    let inbound_mock = spawn_mock_netbox().await?;
    Ok(AuthedApp {
        router,
        state,
        cookie,
        inbound_mock,
        _suite_guard: suite_guard,
    })
}

fn netbox_flow_test_lock() -> &'static Arc<tokio::sync::Mutex<()>> {
    static LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

async fn create_provider_with_secret(
    app: &AuthedApp,
    secret: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let provider = create_provider(
        &app.router,
        &app.cookie,
        &app.inbound_mock.base_url,
        "bidirectional",
        secret,
    )
    .await?;
    let logical_id = Uuid::now_v7();
    let mapping_id = Uuid::now_v7();
    let entity = InventoryEntity::Site;
    let config = MappingConfiguration {
        absence_policy: Default::default(),
        logical_id,
        version_id: 1,
        name: format!("live-site-{}", unique_suffix()),
        description: None,
        source_provider_id: provider,
        target: MappingTarget::LocalInventory,
        status: MappingConfigurationStatus::Active,
        direction: MappingDirection::Bidirectional,
        conflict_mode: ConflictMode::NetboxWins,
        created_by: None,
        created_at: chrono::Utc::now(),
        retired_at: None,
    };
    let mappings = [EntityMapping {
        id: mapping_id,
        mapping_config_logical_id: logical_id,
        mapping_config_version_id: 1,
        source_entity: entity.source_key().to_string(),
        target_entity: entity.target_key(),
        enabled: true,
        created_at: chrono::Utc::now(),
    }];
    let fields: Vec<_> = entity
        .field_iter()
        .map(|field| {
            let name = field.name();
            FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: mapping_id,
                source_field: name.to_string(),
                target_field: name.to_string(),
                forward_transform_json: json!({"op": "identity", "args": [{"field": name}]}),
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: chrono::Utc::now(),
            }
        })
        .collect();
    app.state
        .repositories
        .sync_mapping_configurations
        .insert_new_version(&config, &mappings, &fields)
        .await?;
    Ok(provider)
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

async fn hold_provider_lease(
    app: &AuthedApp,
    provider: Uuid,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let owner = Uuid::now_v7().to_string();
    let mut redis = app.state.redis.clone();
    let acquired: Option<String> = redis::cmd("SET")
        .arg(format!("sync:provider:{provider}"))
        .arg(&owner)
        .arg("NX")
        .arg("EX")
        .arg(120)
        .query_async(&mut redis)
        .await?;
    assert_eq!(acquired.as_deref(), Some("OK"));
    Ok(owner)
}

async fn release_provider_lease(
    app: &AuthedApp,
    provider: Uuid,
    owner: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut redis = app.state.redis.clone();
    let released: i32 = redis::Script::new("if redis.call('GET',KEYS[1]) == ARGV[1] then return redis.call('DEL',KEYS[1]) else return 0 end")
        .key(format!("sync:provider:{provider}")).arg(owner).invoke_async(&mut redis).await?;
    assert_eq!(released, 1, "release only the lease this fixture owns");
    Ok(())
}

async fn queue_site_event(
    app: &AuthedApp,
    provider: Uuid,
    operation: &str,
    local: Uuid,
    mut payload: Value,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    payload["local_id"] = json!(local);
    let event = OutboundEvent {
        id: Uuid::now_v7(),
        source_id: provider,
        resource_type: "dcim.sites".to_string(),
        operation: operation.to_string(),
        payload,
        status: OutboundEventStatus::Pending,
        attempts: 0,
        next_attempt_at: None,
        processed_at: None,
        correlation_id: None,
        last_error: None,
        created_at: chrono::Utc::now(),
    };
    app.state.services.outbound_service.enqueue(&event).await?;
    Ok(event.id)
}

async fn seed_site_binding(
    pool: &sqlx::PgPool,
    provider: Uuid,
    external: &str,
    local: Uuid,
    last_seen: Option<Uuid>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id,last_seen_job_id) VALUES ($1,'dcim.sites',$2,'piping.inventory.site',$3,$4)")
        .bind(provider).bind(external).bind(local).bind(last_seen).execute(pool).await?;
    Ok(())
}

async fn site_binding_count(
    pool: &sqlx::PgPool,
    provider: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM sync_inventory_binding WHERE source_provider_id=$1 AND source_entity='dcim.sites' AND target_entity='piping.inventory.site'")
        .bind(provider).fetch_one(pool).await?)
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
    fail_after_create_remaining: Arc<AtomicI32>,
    pause_create_response: Arc<AtomicBool>,
    create_started: Arc<tokio::sync::Notify>,
    allow_create_response: Arc<tokio::sync::Notify>,
    fail_patch_remaining: Arc<AtomicI32>,
    fail_delete_remaining: Arc<AtomicI32>,
}

struct MockNetboxHandle {
    base_url: String,
    state: MockNetboxState,
}

impl MockNetboxHandle {
    async fn put_site(&self, id: i64, name: &str) {
        let record = json!({
            "id": id, "name": name, "slug": name,
            "status": {"value": "active", "label": "Active"},
            "facility": "", "description": "", "physical_address": "",
            "shipping_address": "", "comments": "", "tenant": null,
            "time_zone": null, "tags": []
        });
        let mut sites = self.state.sites.lock().await;
        sites.retain(|site| site.get("id").and_then(Value::as_i64) != Some(id));
        sites.push(record);
    }
}

async fn spawn_mock_netbox() -> Result<MockNetboxHandle, Box<dyn std::error::Error + Send + Sync>> {
    let state = MockNetboxState::default();
    let app = Router::new()
        .route("/api/schema/", get(mock_schema))
        .route("/dcim/sites/", get(mock_list_sites).post(mock_create_site))
        .route(
            "/dcim/sites/{id}/",
            get(mock_get_site)
                .patch(mock_update_site)
                .delete(mock_delete_site),
        )
        .route(
            "/api/dcim/sites/",
            get(mock_list_sites).post(mock_create_site),
        )
        .route(
            "/api/dcim/sites/{id}/",
            get(mock_get_site)
                .patch(mock_update_site)
                .delete(mock_delete_site),
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
                "get": {
                    "responses": { "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Site" } } } } }
                },
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

async fn mock_get_site(
    State(state): State<MockNetboxState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let sites = state.sites.lock().await;
    match sites
        .iter()
        .find(|site| site.get("id").and_then(Value::as_i64) == Some(id))
    {
        Some(site) => Json(site.clone()).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"detail": "site not found"})),
        )
            .into_response(),
    }
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
    state.create_started.notify_one();
    if state.pause_create_response.load(Ordering::SeqCst) {
        state.allow_create_response.notified().await;
    }
    if consume_failure(&state.fail_after_create_remaining) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"remote create committed before this response failure"})),
        )
            .into_response();
    }
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
