//! Integration test for run-group event-batch ingest idempotency.
//!
//! The runner retries a batch POST up to three times, so a lost ACK (the
//! backend appended the events but the response never arrived) would re-append
//! the same events without dedup. The backend claims the runner-assigned
//! `batch_id` and its events in one PostgreSQL transaction; a re-POST of an
//! already-ingested `(run_group_id, batch_id)` must be a 200 no-op that appends
//! nothing, while a fresh `batch_id` must append normally.
//!
//! Requires PostgreSQL and Redis to construct the application state (e.g.
//! `make run-up`). Run with:
//! `cargo test --test integration_playbook_event_batch_idempotency -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{build_test_app_state, build_test_db_pool, request_with_auth, unique_suffix};
use piping_backend::contexts::playbooks::domain::PlaybookRunEventKind;
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    AppendRunGroupEventBatchOutcome, CreateRunGroupEventRecord, PlaybookRepository,
    SqlxPlaybookRepository,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Seed a minimal playbook + version + running run group + running run, and
/// mint an `event:append` job token bound to that run, mirroring what dispatch
/// does. Returns `(run_group_id, run_id, raw_job_token)`.
async fn seed_running_group_with_callback_token(
    db: &sqlx::PgPool,
) -> Result<(Uuid, Uuid, String), Box<dyn std::error::Error + Send + Sync>> {
    let suffix = unique_suffix();
    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook (id, slug, name, status, runtime, risk, category)
        VALUES ($1, $2, 'Event Batch Idempotency Test', 'active', 'ansible', 'low', 'general')
        "#,
    )
    .bind(playbook_id)
    .bind(format!("event-batch-idem-{suffix}"))
    .execute(db)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_version (
            id, playbook_id, version, language, runtime, content, content_sha256
        )
        VALUES ($1, $2, 1, 'yaml', 'ansible', '---\n- hosts: all\n', $3)
        "#,
    )
    .bind(version_id)
    .bind(playbook_id)
    .bind("0000000000000000000000000000000000000000000000000000000000000000")
    .execute(db)
    .await?;
    sqlx::query("UPDATE playbook SET current_version_id = $2 WHERE id = $1")
        .bind(playbook_id)
        .bind(version_id)
        .execute(db)
        .await?;

    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, playbook_id, playbook_version_id, trigger_kind, status, total_runs
        )
        VALUES ($1, $2, $3, 'manual', 'running', 1)
        "#,
    )
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(db)
    .await?;

    let run_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let source_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run (
            id, run_group_id, playbook_id, playbook_version_id,
            target_kind, target_id, source_kind, source_id, status
        )
        VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $6, 'running')
        "#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(target_id)
    .bind(source_id)
    .execute(db)
    .await?;

    // Mint a GROUP callback token, the way `PlaybookService` does: raw is
    // `pjt_<token_id>_<secret>`, stored as its SHA-256 hex.
    //
    // Not a per-run job token. The batch endpoint authorizes against
    // `playbook_run_group_callback_token` precisely so a run token cannot
    // inherit its group's authority; seeding the run token here made this
    // fixture assert the old model and the endpoint answered 401.
    let token_id = Uuid::now_v7();
    let secret = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
    let raw_token = format!("pjt_{token_id}_{secret}");
    let token_hash = format!("{:x}", Sha256::digest(raw_token.as_bytes()));
    let token_prefix = format!("pjt_{}", &secret[..8]);
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group_callback_token (
            id, run_group_id, token_hash, token_prefix, status, expires_at
        )
        VALUES ($1, $2, $3, $4, 'active', now() + interval '1 hour')
        "#,
    )
    .bind(token_id)
    .bind(group_id)
    .bind(token_hash)
    .bind(token_prefix)
    .execute(db)
    .await?;

    Ok((group_id, run_id, raw_token))
}

async fn event_count(db: &sqlx::PgPool, group_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM playbook_run_event WHERE run_group_id = $1")
        .bind(group_id)
        .fetch_one(db)
        .await
}

fn batch_body(batch_id: Uuid, run_id: Uuid, message: &str) -> String {
    serde_json::json!({
        "batch_id": batch_id,
        "events": [{
            "run_id": run_id,
            "host": "idem-test-host",
            "event_kind": "stdout",
            "event_type": "console_line",
            "message": message,
            "payload": {}
        }]
    })
    .to_string()
}

/// Remove everything `seed_running_group_with_callback_token` created.
///
/// Without this the suite leaked `playbook_run` rows in a worker state into
/// the shared development database — four of them sat there for days and were
/// initially mistaken for a product defect (a run stuck in `running`). The
/// fixture state is not reachable through the product: runs are created
/// `queued` and every transition into a worker state sets `started_at`.
async fn cleanup_seeded_group(
    db: &sqlx::PgPool,
    group_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // playbook_run_event and playbook_job_token cascade from the group/run.
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_playbook_event_batch_idempotency -- --ignored
async fn retried_event_batch_is_ingested_at_most_once() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await.expect("db pool");
    let (group_id, run_id, token) = seed_running_group_with_callback_token(&db)
        .await
        .expect("seed run group");
    let uri = format!("/api/v1/playbooks/run-groups/{group_id}/events/batch");

    // First delivery appends the batch.
    let batch_id = Uuid::now_v7();
    let body = batch_body(batch_id, run_id, "first delivery");
    let (status, response) = request_with_auth(&app, "POST", &uri, Some(&body), None, Some(&token))
        .await
        .expect("first delivery");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "first delivery: {}",
        String::from_utf8_lossy(&response)
    );
    let events: Value = serde_json::from_slice(&response).expect("events json");
    assert_eq!(events.as_array().map(Vec::len), Some(1));
    assert_eq!(event_count(&db, group_id).await.expect("count events"), 1);

    // A retried delivery of the same batch (lost ACK) is a 200 no-op.
    let (status, response) = request_with_auth(&app, "POST", &uri, Some(&body), None, Some(&token))
        .await
        .expect("retried delivery");
    assert_eq!(
        status,
        StatusCode::OK,
        "retried delivery must dedup, not re-append: {}",
        String::from_utf8_lossy(&response)
    );
    let events: Value = serde_json::from_slice(&response).expect("events json");
    assert_eq!(events.as_array().map(Vec::len), Some(0));
    assert_eq!(
        event_count(&db, group_id).await.expect("count events"),
        1,
        "duplicate batch_id must not append events"
    );

    // A fresh batch_id from the same runner appends normally.
    let next_body = batch_body(Uuid::now_v7(), run_id, "second batch");
    let (status, response) =
        request_with_auth(&app, "POST", &uri, Some(&next_body), None, Some(&token))
            .await
            .expect("next batch");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "fresh batch_id must append: {}",
        String::from_utf8_lossy(&response)
    );
    assert_eq!(event_count(&db, group_id).await.expect("count events"), 2);

    cleanup_seeded_group(&db, group_id)
        .await
        .expect("cleanup seeded run group");
}

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_playbook_event_batch_idempotency -- --ignored
async fn failed_append_does_not_commit_the_idempotency_claim() {
    let db = build_test_db_pool().await.expect("db pool");
    piping_backend::storage::database::migrations::run_migrations(&db)
        .await
        .expect("run migrations");
    let (group_id, run_id, _) = seed_running_group_with_callback_token(&db)
        .await
        .expect("seed run group");
    let repository = SqlxPlaybookRepository::new(db.clone());

    // Force the event insert to fail after the idempotency INSERT by using a
    // nonexistent run ID. Both writes must roll back together.
    let batch_id = Uuid::now_v7();
    let invalid_record = CreateRunGroupEventRecord {
        run_group_id: group_id,
        run_id: Some(Uuid::now_v7()),
        host: Some("idem-test-host".to_string()),
        event_kind: PlaybookRunEventKind::Stdout,
        event_type: "console_line".to_string(),
        message: Some("failed delivery".to_string()),
        payload: serde_json::json!({}),
    };
    repository
        .append_run_group_event_batch(batch_id, vec![invalid_record])
        .await
        .expect_err("invalid event insert must fail");
    let claim_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM playbook_run_group_event_batch
        WHERE run_group_id = $1 AND batch_id = $2
        "#,
    )
    .bind(group_id)
    .bind(batch_id)
    .fetch_one(&db)
    .await
    .expect("count batch claims");
    assert_eq!(
        claim_count, 0,
        "failed event insert must roll back its claim"
    );

    let valid_record = CreateRunGroupEventRecord {
        run_group_id: group_id,
        run_id: Some(run_id),
        host: Some("idem-test-host".to_string()),
        event_kind: PlaybookRunEventKind::Stdout,
        event_type: "console_line".to_string(),
        message: Some("retry after failure".to_string()),
        payload: serde_json::json!({}),
    };
    let outcome = repository
        .append_run_group_event_batch(batch_id, vec![valid_record])
        .await
        .expect("retry after failed transaction");
    assert!(
        matches!(outcome, AppendRunGroupEventBatchOutcome::Created(ref events) if events.len() == 1),
        "retry must create the event after the failed transaction"
    );
    assert_eq!(event_count(&db, group_id).await.expect("count events"), 1);

    cleanup_seeded_group(&db, group_id)
        .await
        .expect("cleanup seeded run group");
}
