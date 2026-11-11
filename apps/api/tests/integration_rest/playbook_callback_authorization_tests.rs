//! Router-level authorization for the supervisor's group console callback.
//!
//! The repository-level console suite proves durability and sealing but calls the
//! service directly, so it cannot observe a central-auth regression that consumes
//! the callback bearer before the handler runs. This module drives the real
//! router with a token minted by the owning playbook service and asserts both the
//! accepted callback and every refused neighbour.
//!
//! Run against isolated PostgreSQL and Redis fixtures:
//! `cargo test --test integration_rest playbook_callback_authorization -- --ignored --test-threads=1`.

use super::common::{build_test_app_state, request_with_auth, unique_suffix};
use super::project_workspace::{WorkspaceFixture, seed_workspace};
use axum::Router;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use piping_backend::app::AppState;
use piping_backend::contexts::playbooks::domain::{InventorySelection, PlaybookRunTriggerKind};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateRunGroupRecord, PlaybookRepository, SqlxPlaybookRepository,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Exact bytes the simulated supervisor ships in its first console chunk.
const CONSOLE_MARKER: &[u8] = b"synthetic native console line\n";

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn boxed(error: impl std::fmt::Display) -> Box<dyn std::error::Error + Send + Sync> {
    Box::from(error.to_string())
}

/// One run group with a live callback token, exactly as the worker mints it.
struct LiveGroup {
    id: Uuid,
    dispatch_id: Uuid,
    dispatch_attempt: i32,
    lease_owner: String,
    token: String,
}

/// Persist a native launch, move it through the worker's queue transition, and
/// let the owning service mint the group callback token.
async fn start_native_group(
    pool: &PgPool,
    state: &AppState,
    workspace: &WorkspaceFixture,
    lease_owner: &str,
) -> Result<LiveGroup, Box<dyn std::error::Error + Send + Sync>> {
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let created = repo
        .create_run_group(CreateRunGroupRecord {
            launch_spec: workspace.launch_spec(Vec::new()),
            schedule_id: None,
            scheduled_for: None,
            trigger_kind: PlaybookRunTriggerKind::Manual,
            requested_by_user_id: None,
            service_principal_id: None,
            metadata: json!({}),
            secret_env_authorization: None,
            ssh_record_contexts: Vec::new(),
        })
        .await?;
    // Fixture seeding of the worker's queue transition; scoped to this group so
    // an unrelated queued group in a shared database is never touched.
    let now = Utc::now();
    let leased = sqlx::query(
        "UPDATE playbook_run_group \
         SET status = 'leased', lease_owner = $2, lease_expires_at = $3, \
             leased_at = $4, started_at = COALESCE(started_at, $4), updated_at = $4 \
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(created.run_group.id)
    .bind(lease_owner)
    .bind(now + Duration::seconds(600))
    .bind(now)
    .execute(pool)
    .await?;
    if leased.rows_affected() != 1 {
        return Err("fixture group was not leased".into());
    }
    let execution = state
        .services
        .playbooks
        .start_leased_run_group(created.run_group.id, lease_owner)
        .await?;
    let dispatch_id = execution
        .run_group
        .dispatch_id
        .ok_or("dispatch identity missing")?;
    Ok(LiveGroup {
        id: execution.run_group.id,
        dispatch_id,
        dispatch_attempt: execution.run_group.dispatch_attempt,
        lease_owner: lease_owner.to_string(),
        token: execution.group_callback_token,
    })
}

fn console_body(group: &LiveGroup, dispatch_id: Uuid) -> Value {
    json!({
        "dispatch_id": dispatch_id,
        "dispatch_attempt": group.dispatch_attempt,
        "chunks": [{
            "sequence": 0,
            "stream": "stdout",
            "bytes": CONSOLE_MARKER,
        }],
    })
}

async fn post_console(
    app: &Router,
    group_id: Uuid,
    token: Option<&str>,
    body: &Value,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error + Send + Sync>> {
    send_json(
        app,
        "POST",
        &format!("/api/v1/playbooks/run-groups/{group_id}/console/batch"),
        token,
        body,
    )
    .await
}

async fn send_json(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: &Value,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error + Send + Sync>> {
    let encoded = serde_json::to_vec(body)?;
    request_with_auth(
        app,
        method,
        uri,
        Some(std::str::from_utf8(&encoded)?),
        None,
        token,
    )
    .await
    .map(|(status, bytes)| {
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    })
}

/// Persisted console prefix, independent of any HTTP surface.
async fn stored_console(
    pool: &PgPool,
    group_id: Uuid,
) -> Result<Vec<(i64, String, Vec<u8>)>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query(
        "SELECT sequence, stream, bytes FROM playbook_run_console_chunk \
         WHERE run_group_id = $1 ORDER BY sequence",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get("sequence"), row.get("stream"), row.get("bytes")))
        .collect())
}

async fn terminal_audit_count(
    pool: &PgPool,
    group_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        "SELECT count(*) AS count FROM audit_event \
         WHERE target_id = $1 AND event_type = 'playbook.run_group.completed'",
    )
    .bind(group_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get("count"))
}

async fn cleanup(
    pool: &PgPool,
    workspace: &WorkspaceFixture,
    groups: &[Uuid],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for group in groups {
        sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
            .bind(group)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM audit_event WHERE target_id = $1")
            .bind(group)
            .execute(pool)
            .await?;
        // Route-emitted callback audits target the appended event, so they are
        // reached through the run-group correlation instead.
        sqlx::query(
            "DELETE FROM audit_event \
             WHERE metadata #>> '{audit_context,correlation,run_group_id}' = $1",
        )
        .bind(group.to_string())
        .execute(pool)
        .await?;
    }
    workspace.cleanup(pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis fixtures"]
async fn group_callback_token_is_accepted_only_for_its_own_console_batch() -> TestResult {
    let state = build_test_app_state().await.map_err(boxed)?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state.clone());
    let suffix = unique_suffix();
    let workspace = seed_workspace(
        &pool,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let group =
        start_native_group(&pool, &state, &workspace, &format!("console-auth-{suffix}")).await?;
    let other = start_native_group(
        &pool,
        &state,
        &workspace,
        &format!("console-auth-peer-{suffix}"),
    )
    .await?;

    let exact = console_body(&group, group.dispatch_id);
    let baseline = stored_console(&pool, group.id).await?;
    assert!(
        baseline.is_empty(),
        "fixture group must start with no console"
    );

    // The supervisor's own callback is accepted and acknowledged durably.
    let (status, ack) = post_console(&app, group.id, Some(&group.token), &exact).await?;
    assert_eq!(status, StatusCode::OK, "callback must be authorized: {ack}");
    assert_eq!(ack["dispatch_id"], json!(group.dispatch_id));
    assert_eq!(ack["dispatch_attempt"], json!(group.dispatch_attempt));
    assert_eq!(ack["next_sequence"], json!(1));
    let after_ack = stored_console(&pool, group.id).await?;
    assert_eq!(after_ack.len(), 1, "one chunk must be durable");
    assert_eq!(after_ack[0].2, CONSOLE_MARKER.to_vec());

    // An identical retry is idempotent: same acknowledgement, same bytes.
    let (status, retry_ack) = post_console(&app, group.id, Some(&group.token), &exact).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "identical retry must be idempotent: {retry_ack}"
    );
    assert_eq!(retry_ack, ack, "retry must return the same acknowledgement");
    assert_eq!(
        stored_console(&pool, group.id).await?,
        after_ack,
        "retry must not rewrite the prefix"
    );

    // Refusals must not write anything.
    let altered = console_body(&group, Uuid::now_v7());
    let foreign_token = format!("pjt_{}_{}", Uuid::now_v7().simple(), "a".repeat(32));
    let cases: Vec<(&str, Option<String>, Value, StatusCode)> = vec![
        (
            "altered dispatch identity",
            Some(group.token.clone()),
            altered,
            StatusCode::CONFLICT,
        ),
        (
            "wrong token",
            Some(foreign_token),
            exact.clone(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "missing token",
            None,
            exact.clone(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "peer group token",
            Some(other.token.clone()),
            exact.clone(),
            StatusCode::UNAUTHORIZED,
        ),
    ];
    for (label, token, body, expected) in cases {
        let (status, payload) = post_console(&app, group.id, token.as_deref(), &body).await?;
        assert_eq!(status, expected, "{label} must be {expected}: {payload}");
        assert_eq!(
            stored_console(&pool, group.id).await?,
            after_ack,
            "{label} must not persist console bytes"
        );
    }
    assert_eq!(
        stored_console(&pool, other.id).await?,
        Vec::new(),
        "a peer token must not write the peer group either"
    );

    // The callback token is not a session: ordinary reads stay refused.
    for uri in [
        format!(
            "/api/v1/playbooks/run-groups/{}/console?dispatch_id={}",
            group.id, group.dispatch_id
        ),
        format!("/api/v1/playbooks/run-groups/{}", group.id),
        "/api/v1/auth/me".to_string(),
    ] {
        let (status, payload) =
            request_with_auth(&app, "GET", &uri, None, None, Some(&group.token))
                .await
                .map(|(status, bytes)| {
                    (
                        status,
                        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                    )
                })?;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "callback token must not read {uri}: {payload}"
        );
    }

    // Adjacent method on the read-only path stays closed to the callback token.
    let (status, _) = send_json(
        &app,
        "POST",
        &format!("/api/v1/playbooks/run-groups/{}/console", group.id),
        Some(&group.token),
        &exact,
    )
    .await?;
    assert!(
        !status.is_success(),
        "callback token must not turn the console read route into a write: {status}"
    );

    // Sealing through the real completion callback keeps one terminal audit.
    let completion = json!({
        "dispatch_id": group.dispatch_id,
        "dispatch_attempt": group.dispatch_attempt,
        "lease_owner": group.lease_owner,
        "console_next_sequence": 1,
        "group_status": "succeeded",
        "exit_code": 0,
        "error_message": null,
        "runs": [],
    });
    let completion_uri = format!("/api/v1/playbooks/run-groups/{}/completion-batch", group.id);
    let (status, sealed) = send_json(
        &app,
        "POST",
        &completion_uri,
        Some(&group.token),
        &completion,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "completion callback must be authorized: {sealed}"
    );
    let audit_after_seal = terminal_audit_count(&pool, group.id).await?;
    assert_eq!(
        audit_after_seal, 1,
        "terminal completion must audit exactly once"
    );

    // Exact replay is idempotent and adds no second terminal audit.
    let (status, replayed) = send_json(
        &app,
        "POST",
        &completion_uri,
        Some(&group.token),
        &completion,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "exact completion replay must be idempotent: {replayed}"
    );
    assert_eq!(
        terminal_audit_count(&pool, group.id).await?,
        audit_after_seal,
        "replay must not duplicate the terminal audit"
    );

    // A conflicting completion is refused without touching stored state.
    let mut conflicting = completion.clone();
    conflicting["group_status"] = json!("failed");
    conflicting["exit_code"] = json!(1);
    let (status, refused) = send_json(
        &app,
        "POST",
        &completion_uri,
        Some(&group.token),
        &conflicting,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "conflicting completion must be refused: {refused}"
    );
    assert_eq!(
        terminal_audit_count(&pool, group.id).await?,
        audit_after_seal
    );
    assert_eq!(stored_console(&pool, group.id).await?, after_ack);

    // A sealed group still accepts the exact console prefix, once.
    let (status, _) = post_console(&app, group.id, Some(&group.token), &exact).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "sealed prefix retry must remain idempotent"
    );
    assert_eq!(stored_console(&pool, group.id).await?, after_ack);

    cleanup(&pool, &workspace, &[group.id, other.id]).await?;
    Ok(())
}

/// The accepted group event batch must persist exactly one audit row for the
/// callback actor class, bound to its own run group, and a refused neighbour
/// must persist none.
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis fixtures"]
async fn group_event_batch_audits_its_callback_token_and_run_group() -> TestResult {
    let state = build_test_app_state().await.map_err(boxed)?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state.clone());
    let suffix = unique_suffix();
    let workspace = seed_workspace(
        &pool,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let group =
        start_native_group(&pool, &state, &workspace, &format!("audit-group-{suffix}")).await?;
    let other = start_native_group(
        &pool,
        &state,
        &workspace,
        &format!("audit-group-peer-{suffix}"),
    )
    .await?;

    // The run the runner reports events for; the group owns no run otherwise.
    let run_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_run (id, run_group_id, hostname, status) \
         VALUES ($1, $2, 'audit-test-host', 'running')",
    )
    .bind(run_id)
    .bind(group.id)
    .execute(&pool)
    .await?;

    let batch_id = Uuid::now_v7();
    let body = json!({
        "batch_id": batch_id,
        "events": [{
            "run_id": run_id,
            "host": "audit-test-host",
            "event_kind": "stdout",
            "event_type": "console_line",
            "message": "audited group callback",
            "payload": {},
        }],
    });

    let (status, events) = send_json(
        &app,
        "POST",
        &format!("/api/v1/playbooks/run-groups/{}/events/batch", group.id),
        Some(&group.token),
        &body,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "batch must be accepted: {events}"
    );

    let (token_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM playbook_run_group_callback_token WHERE run_group_id = $1")
            .bind(group.id)
            .fetch_one(&pool)
            .await?;
    let audit_rows = callback_audit_rows(&pool, group.id).await?;
    assert_eq!(
        audit_rows.len(),
        1,
        "an accepted group batch must persist exactly one audit row: {audit_rows:?}"
    );
    let (actor_id, actor_kind, target_type, target_id, metadata) = &audit_rows[0];
    assert_eq!(
        *actor_id, None,
        "the callback identity is the delegated token, not a user"
    );
    assert_eq!(actor_kind, "playbook_job_token");
    assert_eq!(target_type.as_deref(), Some("playbook_run_event"));
    assert_eq!(
        *target_id,
        events[0]["id"].as_str().map(Uuid::parse_str).transpose()?,
        "the audit target is the first appended event"
    );
    assert_eq!(
        metadata.pointer("/audit_context/delegated_principal"),
        Some(&json!({"kind": "playbook_job_token", "id": token_id})),
        "the persisted row must name the run-group callback token"
    );
    assert_eq!(
        metadata.pointer("/audit_context/correlation/run_group_id"),
        Some(&json!(group.id))
    );
    assert_eq!(
        metadata.pointer("/audit_context/emitter/kind"),
        Some(&json!("api"))
    );
    assert!(
        !metadata.to_string().contains(&group.token),
        "the raw callback token must never reach the audit row"
    );

    // A peer group's token is refused and writes neither an event nor an
    // audit row for this group.
    let (status, refused) = send_json(
        &app,
        "POST",
        &format!("/api/v1/playbooks/run-groups/{}/events/batch", group.id),
        Some(&other.token),
        &json!({"batch_id": Uuid::now_v7(), "events": body["events"].clone()}),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a peer group token must be refused: {refused}"
    );
    assert_eq!(
        callback_audit_rows(&pool, group.id).await?.len(),
        1,
        "a refused callback must not add an audit row"
    );
    assert_eq!(
        callback_audit_rows(&pool, other.id).await?.len(),
        0,
        "a refused callback must not attribute the event to the peer group"
    );

    cleanup(&pool, &workspace, &[group.id, other.id]).await?;
    Ok(())
}

/// Persisted audit rows for accepted run-group callback batches, correlated to
/// one run group.
type CallbackAuditRow = (Option<Uuid>, String, Option<String>, Option<Uuid>, Value);

async fn callback_audit_rows(
    pool: &PgPool,
    group_id: Uuid,
) -> Result<Vec<CallbackAuditRow>, Box<dyn std::error::Error + Send + Sync>> {
    let rows = sqlx::query(
        "SELECT actor_id, actor_kind, target_type, target_id, metadata FROM audit_event \
         WHERE event_type = 'playbook.run_group_events.appended' \
           AND metadata #>> '{audit_context,correlation,run_group_id}' = $1",
    )
    .bind(group_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                row.get("actor_id"),
                row.get("actor_kind"),
                row.get("target_type"),
                row.get("target_id"),
                row.get("metadata"),
            )
        })
        .collect())
}
