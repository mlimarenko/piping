//! Integration coverage for the CCTV audit trail (PWA-63).
//!
//! Properties that the in-crate unit test cannot prove:
//!
//! 1. **RBAC precedes the device call.** Every CCTV mutating handler authorizes
//!    `cctv:*` for the target BEFORE it resolves or dials the device. This suite
//!    seeds a device whose management endpoint is deliberately unreachable, so a
//!    control run (admin cookie, no deny policy) answers with a device error
//!    (502), while a run under a DENY policy answers 403. A 403 from an
//!    unreachable device is only possible if authorization ran first.
//!
//! 2. **A successful PTZ handler persists actor/action/target with
//!    credential-free metadata.** The emitter writes only after device success.
//!
//! 3. **A partial device-user write remains visible and audited.** The first
//!    acknowledgement survives a broken second response; the third account is
//!    not attempted. Both single-target and fleet audit rows contain counts,
//!    not account names or device error messages.
//!
//! A local HTTP device stand-in drives a successful PTZ request through the
//! router and checks the durable row. The crate-internal CCTV tests exercise
//! all five emitter variants; the HTTP suite also drives partial user writes.
//! The handler gate lives in `compliance/cctv_audit_coverage.rs`.
//!
//! Requires PostgreSQL and Redis. CI's coverage lane includes ignored tests;
//! locally run with:
//! `cargo test -p piping-backend --test integration_cctv_audit_trail -- --ignored --test-threads=1`

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Router, http::StatusCode, routing::get};
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    build_test_redis, login_and_get_cookie, request, seed_device, unique_suffix,
};
use serde_json::json;
use uuid::Uuid;

/// One `audit_event` row as selected by these checks:
/// `(event_type, actor_id, target_type, target_id, metadata)`.
type AuditEventRow = (
    String,
    Option<Uuid>,
    String,
    Option<Uuid>,
    serde_json::Value,
);

/// A CCTV audit row without `target_type` (the query filters on it):
/// `(event_type, actor_id, target_id, metadata)`.
type AuditEventCountRow = (String, Option<Uuid>, Option<Uuid>, serde_json::Value);

/// `(event_type, http_status)` pairs collected while a deny policy is active.
type DeniedCctvRun = Result<Vec<(String, StatusCode)>, Box<dyn std::error::Error + Send + Sync>>;

/// The five mutating CCTV endpoints under test, paired with the audit event type
/// their handler emits and the action/resource the RBAC gate evaluates.
///
/// Payloads are minimal but VALID: request extractors parse before RBAC, so an
/// invalid body would answer 400 and never exercise the authorization gate.
fn cctv_write_requests(target: &str) -> Vec<(&'static str, String, Option<String>)> {
    // A window that is well within the export cap and strictly increasing:
    // `parse_export_request` rejects an inverted or over-long window as 400.
    let export_uri = format!(
        "/api/v1/cctv/{target}/export?channel=1&start=2026-08-11T00:00:00Z&end=2026-08-11T00:01:00Z"
    );
    vec![
        (
            "cctv.ptz.commanded",
            format!("/api/v1/cctv/{target}/ptz"),
            // A non-zero `move`: it dials the device and propagates the transport
            // error. (A `stop` is deliberately error-swallowing in the Dahua
            // driver, so it would report success against an unreachable device.)
            Some(json!({"kind": "move", "pan": 1.0}).to_string()),
        ),
        ("cctv.recording.exported", export_uri, None),
        (
            "device.users.created",
            format!("/api/v1/device-fleet/{target}/users:create"),
            Some(json!({"users": [{"name": "audit-probe"}]}).to_string()),
        ),
        (
            "device.users.updated",
            format!("/api/v1/device-fleet/{target}/users"),
            Some(json!({"users": [{"name": "audit-probe", "group": "operators"}]}).to_string()),
        ),
        (
            "device.users.deleted",
            format!("/api/v1/device-fleet/{target}/users:delete"),
            Some(json!({"usernames": ["audit-probe"]}).to_string()),
        ),
    ]
}

/// Spawn a stand-in Dahua management plane for user writes.
///
/// `ok` selects the device's completed-write body (`OK`) or a device-reported
/// rejection, so one server drives the success path and another a wholly failed
/// one. Returns the bound port and a request counter — a denied target points at
/// a live server whose counter must stay zero, proving authorization ran before
/// any dial.
async fn spawn_cctv_user_device(
    ok: bool,
) -> Result<(u16, Arc<AtomicUsize>), Box<dyn std::error::Error + Send + Sync>> {
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    let device = Router::new().fallback(get(move |_uri: axum::http::Uri| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            if ok {
                "OK\r\n"
            } else {
                "Error: mock rejected the write\r\n"
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move { axum::serve(listener, device).await });
    Ok((port, requests))
}

/// Acknowledge the first CGI write, break the second response body, and expose
/// the call count so the third requested account must remain unattempted.
async fn spawn_partial_cctv_user_device()
-> Result<(u16, Arc<AtomicUsize>), Box<dyn std::error::Error + Send + Sync>> {
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    let device = Router::new().fallback(get(move || {
        let counter = counter.clone();
        async move {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                axum::body::Body::from("OK\r\n")
            } else {
                axum::body::Body::from_stream(futures_util::stream::once(async {
                    Err::<bytes::Bytes, std::io::Error>(std::io::Error::other("device link broken"))
                }))
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move { axum::serve(listener, device).await });
    Ok((port, requests))
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn single_device_partial_write_persists_count_only_audit()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let (port, requests) = spawn_partial_cctv_user_device().await?;
    let (target, _) = seed_device(&db, "cctv-partial-audit").await?;
    let profile = seed_cctv_profile(&db, target, port).await?;
    let body = json!({ "users": [
        {"name": "audit-probe-a"}, {"name": "audit-probe-b"}, {"name": "audit-probe-c"}
    ] })
    .to_string();
    let response = request(
        &app,
        "POST",
        &format!("/api/v1/device-fleet/{target}/users:create"),
        Some(&body),
        Some(&cookie),
    )
    .await;
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, metadata FROM audit_event WHERE target_type = 'cctv_device' AND target_id = $1",
    ).bind(target).fetch_all(&db).await?;
    sqlx::query("DELETE FROM audit_event WHERE target_type = 'cctv_device' AND target_id = $1")
        .bind(target)
        .execute(&db)
        .await?;
    drop_cctv_profile(&db, &profile).await?;

    let (status, body) = response?;
    assert_eq!(status, StatusCode::OK);
    let outcome: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(outcome["succeeded"], json!(["audit-probe-a"]));
    assert_eq!(outcome["failed"].as_array().map(Vec::len), Some(2));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "device.users.created");
    assert_eq!(rows[0].1["result"], "partial_success");
    assert_eq!(rows[0].1["succeeded_count"], 1);
    assert_eq!(rows[0].1["failed_count"], 2);
    assert_fleet_metadata_is_count_only(&rows[0].1);
    Ok(())
}

/// A fleet audit row carries counts, never the account names it acted on.
fn assert_fleet_metadata_is_count_only(metadata: &serde_json::Value) {
    let mut scanned = metadata.clone();
    if let Some(object) = scanned.as_object_mut() {
        // `auth_method` is a legitimate label, not credential material.
        object.remove("auth_method");
    }
    let serialized = scanned.to_string();
    for forbidden in ["audit-probe", "password", "secret", "token", "credential"] {
        assert!(
            !serialized.contains(forbidden),
            "fleet audit metadata leaked `{forbidden}`: {serialized}"
        );
    }
    for required in [
        "operation",
        "device_id",
        "result",
        "succeeded_count",
        "failed_count",
    ] {
        assert!(
            serialized.contains(required),
            "fleet audit metadata must retain `{required}`: {serialized}"
        );
    }
}

/// Count the persisted CCTV audit rows for one target.
async fn audit_count(
    db: &sqlx::PgPool,
    device_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event WHERE target_id = $1 AND target_type = 'cctv_device'",
    )
    .bind(device_id)
    .fetch_one(db)
    .await?;
    Ok(count)
}

/// Policies are cached in Redis behind a generation key; a direct SQL seed
/// bypasses the admin API's invalidation (same as `integration_bulk_rbac.rs`).
async fn bump_policy_cache() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut redis = build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    Ok(())
}

/// Make the seeded device resolve to an UNREACHABLE management endpoint and
/// return the record name so the caller can drop it again.
///
/// The resolver prefers a typed `cctv` connection profile: a cctv record with a
/// literal username/password and a literal host/port avoids the secret fence
/// entirely. `127.0.0.1:<closed port>` is refused deterministically, which maps
/// to a device error (502), never a request-validation 400.
///
/// The port is derived from the device id so parallel runs do not share an
/// endpoint; nothing listens on it.
async fn seed_unreachable_cctv_profile(
    db: &sqlx::PgPool,
    device_id: Uuid,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // 49000..=49999, out of the ephemeral range the test stack uses.
    let port = 49000 + (device_id.as_u128() % 1000) as u16;
    seed_cctv_profile(db, device_id, port).await
}

async fn seed_cctv_profile(
    db: &sqlx::PgPool,
    device_id: Uuid,
    port: u16,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let record_id = Uuid::now_v7();
    let record_name = format!("cctv-audit-unreachable-{}", unique_suffix());
    sqlx::query(
        r#"
        INSERT INTO connconf_record (id, name, protocol, enabled, params)
        VALUES ($1, $2, 'cctv', true, $3)
        "#,
    )
    .bind(record_id)
    .bind(&record_name)
    .bind(json!({
        "credentials": {"username": "audit-probe", "password": "not-a-real-password"},
        "host": "127.0.0.1",
        "family": "dahua",
        "mgmt_port": port,
    }))
    .execute(db)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO connconf_assignment (id, record_id, scope_type, scope_id, weight, enabled)
        VALUES ($1, $2, 'device', $3, 0, true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(record_id)
    .bind(device_id)
    .execute(db)
    .await?;
    Ok(record_name)
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn successful_ptz_handler_persists_redacted_audit_and_denied_ptz_never_dials_device()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let (device_id, _) = seed_device(&db, "cctv-audit-live").await?;
    let seeded_actor: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
        .bind(TEST_ADMIN_USERNAME)
        .fetch_one(&db)
        .await?;

    let commands = Arc::new(AtomicUsize::new(0));
    let device_commands = commands.clone();
    let ptz_enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let device_ptz_enabled = ptz_enabled.clone();
    let device = Router::new().fallback(get(move |uri: axum::http::Uri| {
        let commands = device_commands.clone();
        let ptz_enabled = device_ptz_enabled.clone();
        async move {
            if uri.path() == "/cgi-bin/ptz.cgi" {
                commands.fetch_add(1, Ordering::SeqCst);
                "OK\r\n"
            } else if uri
                .query()
                .is_some_and(|query| query.contains("name=ChannelTitle"))
            {
                "table.ChannelTitle[0].Name=Front\r\n"
            } else if uri
                .query()
                .is_some_and(|query| query.contains("name=PTZProtocol"))
            {
                if ptz_enabled.load(Ordering::SeqCst) {
                    "table.PTZProtocol[0].Enable=true\r\n"
                } else {
                    "table.PTZProtocol[0].Enable=false\r\n"
                }
            } else {
                "OK\r\n"
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move { axum::serve(listener, device).await });
    let profile_name = seed_cctv_profile(&db, device_id, port).await?;

    let uri = format!("/api/v1/cctv/{device_id}/ptz");
    let body = json!({"kind": "move", "channel": 1, "pan": 1.0}).to_string();
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(&db)
            .await?;
    let policy_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled) \
         VALUES ($1, $2, 'deny', $3, $4, NULL, $5, '[]'::jsonb, true)",
    )
    .bind(policy_id)
    .bind(format!("cctv-audit-deny-{}", unique_suffix()))
    .bind(json!(["CCTV_PTZ"]))
    .bind(json!(["CCTV"]))
    .bind(json!([format!("role:{role_id}")]))
    .execute(&db)
    .await?;
    bump_policy_cache().await?;
    let denied_result = request(&app, "POST", &uri, Some(&body), Some(&cookie)).await;
    sqlx::query("DELETE FROM rbac_policy WHERE id = $1")
        .bind(policy_id)
        .execute(&db)
        .await?;
    bump_policy_cache().await?;
    let (denied_status, _) = denied_result?;
    assert_eq!(denied_status, StatusCode::FORBIDDEN);
    assert_eq!(
        commands.load(Ordering::SeqCst),
        0,
        "denied PTZ must not dial the driver"
    );
    assert_eq!(
        audit_count(&db, device_id).await?,
        0,
        "denied PTZ must not audit"
    );

    let (unsupported_status, _) = request(&app, "POST", &uri, Some(&body), Some(&cookie)).await?;
    assert_ne!(
        unsupported_status,
        StatusCode::NO_CONTENT,
        "unsupported channel must be refused"
    );
    assert_eq!(
        commands.load(Ordering::SeqCst),
        0,
        "unsupported PTZ must not execute a command"
    );
    assert_eq!(
        audit_count(&db, device_id).await?,
        0,
        "unsupported PTZ must not audit"
    );

    ptz_enabled.store(true, Ordering::SeqCst);
    let (status, response) = request(&app, "POST", &uri, Some(&body), Some(&cookie)).await?;
    let rows: Vec<AuditEventRow> = sqlx::query_as(
        "SELECT event_type, actor_id, target_type, target_id, metadata \
             FROM audit_event WHERE target_type = 'cctv_device' AND target_id = $1",
    )
    .bind(device_id)
    .fetch_all(&db)
    .await?;
    sqlx::query("DELETE FROM audit_event WHERE target_type = 'cctv_device' AND target_id = $1")
        .bind(device_id)
        .execute(&db)
        .await?;
    drop_cctv_profile(&db, &profile_name).await?;
    server.abort();

    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&response)
    );
    assert_eq!(commands.load(Ordering::SeqCst), 1);
    assert_eq!(
        rows.len(),
        1,
        "successful handler must persist exactly one event"
    );
    let (event, actor, target_type, target, metadata) = &rows[0];
    assert_eq!(event, "cctv.ptz.commanded");
    assert_eq!(
        *actor,
        Some(seeded_actor),
        "audit actor must be the logged-in user"
    );
    assert_eq!(target_type, "cctv_device");
    assert_eq!(*target, Some(device_id));
    assert_eq!(metadata["operation"], "cctv_ptz");
    assert_eq!(metadata["device_id"], device_id.to_string());
    assert_eq!(metadata["result"], "success");
    assert_eq!(metadata["channel"], 1);
    assert_eq!(metadata["ptz_action"], "move");
    let mut redacted = metadata.clone();
    redacted
        .as_object_mut()
        .expect("metadata object")
        .remove("auth_method");
    for forbidden in ["password", "secret", "token", "credential"] {
        assert!(
            !redacted
                .to_string()
                .to_ascii_lowercase()
                .contains(forbidden)
        );
    }
    Ok(())
}

/// Delete the seeded profile so the shared database stays clean.
async fn drop_cctv_profile(
    db: &sqlx::PgPool,
    record_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        "DELETE FROM connconf_assignment WHERE record_id IN (SELECT id FROM connconf_record WHERE name = $1)",
    )
    .bind(record_name)
    .execute(db)
    .await?;
    sqlx::query("DELETE FROM connconf_record WHERE name = $1")
        .bind(record_name)
        .execute(db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn cctv_denied_requests_are_refused_before_any_device_side_effect_and_write_no_audit_row()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    let (device_id, _) = seed_device(&db, "cctv-audit").await?;
    let target = device_id.to_string();
    let profile_name = seed_unreachable_cctv_profile(&db, device_id).await?;

    assert_eq!(
        audit_count(&db, device_id).await?,
        0,
        "a freshly seeded device has no audit rows"
    );

    let requests = cctv_write_requests(&target);

    // (1) Unauthenticated: no cookie, every endpoint is 401 and writes nothing.
    for (event_type, uri, body) in &requests {
        let (status, _) = request(&app, method_for(uri), uri, body.as_deref(), None).await?;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{event_type} unauthenticated"
        );
    }
    assert_eq!(audit_count(&db, device_id).await?, 0);

    // (2) CONTROL: admin cookie, NO deny policy. The payloads are valid and RBAC
    //     passes, so each request reaches the unreachable device and answers a
    //     device error — NOT 400 (payload invalid) and NOT 403 (denied).
    let mut control_statuses = Vec::new();
    for (event_type, uri, body) in &requests {
        let (status, response_body) =
            request(&app, method_for(uri), uri, body.as_deref(), Some(&cookie)).await?;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{event_type} control must not be denied"
        );
        assert_ne!(
            status,
            StatusCode::BAD_REQUEST,
            "{event_type} payload must be valid: {}",
            String::from_utf8_lossy(&response_body)
        );
        if event_type.starts_with("device.users.") {
            assert_eq!(
                status,
                StatusCode::BAD_GATEWAY,
                "zero-success device write remains 502"
            );
        }
        control_statuses.push((*event_type, status));
    }
    let fleet_requests = [
        (
            "fleet_created",
            "/api/v1/device-fleet/users:create",
            json!({
                "target_ids": [device_id], "users": [{"name": "audit-probe"}]
            }),
        ),
        (
            "fleet_deleted",
            "/api/v1/device-fleet/users:delete",
            json!({
                "target_ids": [device_id], "usernames": ["audit-probe"]
            }),
        ),
    ];
    for (label, uri, payload) in &fleet_requests {
        let (status, body) =
            request(&app, "POST", uri, Some(&payload.to_string()), Some(&cookie)).await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "{label} control must reach the service"
        );
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert!(
            body.to_string().contains("unreachable"),
            "{label} must not claim success"
        );
    }
    assert_eq!(
        audit_count(&db, device_id).await?,
        0,
        "audit is written only after device success, which never happens here"
    );

    // (3) DENY policy constraining the admin role on all three CCTV actions.
    //
    // The work after the insert runs inside an inner async block returning a
    // `Result`; the outer function captures that result, deletes the policy and
    // bumps the cache UNCONDITIONALLY, and only then asserts. A request error
    // (or an assertion panic) therefore cannot leave the deny policy behind.
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(&db)
            .await?;
    let policy_id = Uuid::now_v7();
    let policy_name = format!("cctv-audit-deny-{}", unique_suffix());
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints, subjects, resources, enabled
        )
        VALUES ($1, $2, 'deny', $3, $4, NULL, $5, '[]'::jsonb, true)
        "#,
    )
    .bind(policy_id)
    .bind(&policy_name)
    .bind(json!(["CCTV_PTZ", "CCTV_EXPORT", "DEVICE_USERS"]))
    .bind(json!(["CCTV", "CONNECTION_TARGET"]))
    .bind(json!([format!("role:{role_id}")]))
    .execute(&db)
    .await?;
    bump_policy_cache().await?;

    let denied_run: DeniedCctvRun = {
        let app = app.clone();
        let cookie = cookie.clone();
        let requests = requests.clone();
        async move {
            let mut denied_statuses = Vec::new();
            for (event_type, uri, body) in &requests {
                let (status, _) =
                    request(&app, method_for(uri), uri, body.as_deref(), Some(&cookie)).await?;
                denied_statuses.push(((*event_type).to_string(), status));
            }
            for (label, uri, payload) in &fleet_requests {
                let (status, body) =
                    request(&app, "POST", uri, Some(&payload.to_string()), Some(&cookie)).await?;
                let body: serde_json::Value = serde_json::from_slice(&body)?;
                assert!(
                    body.to_string().contains("not_permitted"),
                    "{label} must partition denied targets"
                );
                denied_statuses.push(((*label).to_string(), status));
            }
            Ok(denied_statuses)
        }
        .await
    };

    // Unconditional teardown, before any assertion on the collected result.
    sqlx::query("DELETE FROM rbac_policy WHERE id = $1")
        .bind(policy_id)
        .execute(&db)
        .await?;
    bump_policy_cache().await?;

    let denied_statuses = denied_run?;
    for (event_type, status) in &denied_statuses {
        if event_type.starts_with("fleet_") {
            assert_eq!(
                *status,
                StatusCode::OK,
                "{event_type} returns per-target deny"
            );
            continue;
        }
        assert_eq!(
            *status,
            StatusCode::FORBIDDEN,
            "{event_type} must be denied by RBAC before the unreachable device is dialled; \
             the control run answered {control_statuses:?}"
        );
    }

    assert_eq!(
        audit_count(&db, device_id).await?,
        0,
        "denied and unreachable fleet writes must not record success"
    );

    drop_cctv_profile(&db, &profile_name).await?;
    Ok(())
}

/// A successful HTTP fleet write persists exactly one count-only CCTV event per
/// successful target, and none for a denied, failed or unreachable target.
///
/// Six targets in one request: two live mocks that answer `OK`, one that
/// acknowledges the first write before its link breaks, one live mock
/// that rejects the write (per-account failure, no success), one live mock behind
/// a target-scoped DENY policy, and one profile pointing at a closed port. The
/// denied target's server must receive zero requests — authorization precedes
/// the dial, exactly as on the single-device path.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn fleet_user_writes_audit_one_count_only_event_per_successful_target()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
        .bind(TEST_ADMIN_USERNAME)
        .fetch_one(&db)
        .await?;

    let (success_port, success_requests) = spawn_cctv_user_device(true).await?;
    let (partial_port, partial_requests) = spawn_partial_cctv_user_device().await?;
    let (failed_port, _failed_requests) = spawn_cctv_user_device(false).await?;
    let (denied_port, denied_requests) = spawn_cctv_user_device(true).await?;

    let (success_a, _) = seed_device(&db, "fleet-audit-a").await?;
    let (success_b, _) = seed_device(&db, "fleet-audit-b").await?;
    let (partial, _) = seed_device(&db, "fleet-audit-partial").await?;
    let (failed, _) = seed_device(&db, "fleet-audit-failed").await?;
    let (denied, _) = seed_device(&db, "fleet-audit-denied").await?;
    let (unreachable, _) = seed_device(&db, "fleet-audit-unreachable").await?;

    let profile_success_a = seed_cctv_profile(&db, success_a, success_port).await?;
    let profile_success_b = seed_cctv_profile(&db, success_b, success_port).await?;
    let profile_partial = seed_cctv_profile(&db, partial, partial_port).await?;
    let profile_failed = seed_cctv_profile(&db, failed, failed_port).await?;
    let profile_denied = seed_cctv_profile(&db, denied, denied_port).await?;
    let profile_unreachable = seed_unreachable_cctv_profile(&db, unreachable).await?;

    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(&db)
            .await?;
    let policy_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints, subjects, resources, enabled
        )
        VALUES ($1, $2, 'deny', $3, $4, NULL, $5, $6, true)
        "#,
    )
    .bind(policy_id)
    .bind(format!("fleet-audit-deny-{}", unique_suffix()))
    .bind(json!(["DEVICE_USERS"]))
    .bind(json!(["CONNECTION_TARGET"]))
    .bind(json!([format!("role:{role_id}")]))
    // Resource-scoped: only the one target, so the others stay permitted.
    .bind(json!([format!("connection_target:{denied}")]))
    .execute(&db)
    .await?;
    bump_policy_cache().await?;

    let targets = [success_a, success_b, partial, failed, denied, unreachable];
    let create_body = json!({
        "target_ids": targets,
        "users": [{"name": "audit-probe-a"}, {"name": "audit-probe-b"}, {"name": "audit-probe-c"}],
    })
    .to_string();
    let (create_status, create_response) = request(
        &app,
        "POST",
        "/api/v1/device-fleet/users:create",
        Some(&create_body),
        Some(&cookie),
    )
    .await?;

    let delete_body = json!({
        "target_ids": [success_a, failed, denied],
        "usernames": ["audit-probe-a"],
    })
    .to_string();
    let (delete_status, delete_response) = request(
        &app,
        "POST",
        "/api/v1/device-fleet/users:delete",
        Some(&delete_body),
        Some(&cookie),
    )
    .await?;

    // (event_type, actor_id, target_id, metadata); `target_type` is already the
    // query's filter and `actor_kind` is covered by the single-device suite.
    let rows: Vec<AuditEventCountRow> = sqlx::query_as(
        "SELECT event_type, actor_id, target_id, metadata \
         FROM audit_event WHERE target_type = 'cctv_device' AND target_id = ANY($1) ORDER BY id",
    )
    .bind(targets.to_vec())
    .fetch_all(&db)
    .await?;
    let dialled_success_targets = success_requests.load(Ordering::SeqCst);
    let dialled_partial_target = partial_requests.load(Ordering::SeqCst);
    let dialled_denied_targets = denied_requests.load(Ordering::SeqCst);

    // Teardown runs before any assertion, so a failing assertion cannot leave the
    // deny policy or audit rows behind on the shared database.
    sqlx::query(
        "DELETE FROM audit_event WHERE target_type = 'cctv_device' AND target_id = ANY($1)",
    )
    .bind(targets.to_vec())
    .execute(&db)
    .await?;
    sqlx::query("DELETE FROM rbac_policy WHERE id = $1")
        .bind(policy_id)
        .execute(&db)
        .await?;
    bump_policy_cache().await?;
    for name in [
        profile_success_a,
        profile_success_b,
        profile_partial,
        profile_failed,
        profile_denied,
        profile_unreachable,
    ] {
        drop_cctv_profile(&db, &name).await?;
    }

    assert_eq!(
        create_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&create_response)
    );
    assert_eq!(
        delete_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&delete_response)
    );

    let create_json: serde_json::Value = serde_json::from_slice(&create_response)?;
    let outcome_for = |body: &serde_json::Value, target_id: Uuid| -> serde_json::Value {
        body["per_target"]
            .as_array()
            .expect("per_target array")
            .iter()
            .find(|entry| entry["target_id"] == json!(target_id.to_string()))
            .expect("requested target is present")["outcome"]
            .clone()
    };
    let done_succeeded = |body: &serde_json::Value, target_id: Uuid| -> usize {
        outcome_for(body, target_id)["done"]["succeeded"]
            .as_array()
            .map_or_else(
                || panic!("target {target_id} did not reach the device"),
                Vec::len,
            )
    };

    assert_eq!(done_succeeded(&create_json, success_a), 3);
    assert_eq!(done_succeeded(&create_json, success_b), 3);
    assert_eq!(done_succeeded(&create_json, partial), 1);
    assert_eq!(
        outcome_for(&create_json, partial)["done"]["failed"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        done_succeeded(&create_json, failed),
        0,
        "a rejecting device is reached but has no successful account"
    );
    assert_eq!(outcome_for(&create_json, denied), json!("not_permitted"));
    assert!(
        outcome_for(&create_json, unreachable)
            .get("unreachable")
            .is_some(),
        "a closed port is unreachable, not a success"
    );

    let created: Vec<_> = rows
        .iter()
        .filter(|row| row.0 == "device.users.created")
        .collect();
    let deleted: Vec<_> = rows
        .iter()
        .filter(|row| row.0 == "device.users.deleted")
        .collect();
    assert_eq!(
        created.len(),
        3,
        "one create event per successful target, and none for denied/failed/unreachable"
    );
    assert_eq!(
        deleted.len(),
        1,
        "one delete event for the one permitted target that succeeded"
    );
    assert_eq!(rows.len(), 4, "events exist only for successful targets");

    for row in &created {
        assert_eq!(row.1, Some(admin_id), "actor must be the logged-in user");
        assert!(matches!(row.2, Some(id) if id == success_a || id == success_b || id == partial));
        assert_eq!(row.3["operation"], json!("fleet_create_users"));
        let partial_row = row.2 == Some(partial);
        assert_eq!(
            row.3["result"],
            json!(if partial_row {
                "partial_success"
            } else {
                "success"
            })
        );
        assert_eq!(
            row.3["succeeded_count"],
            json!(if partial_row { 1 } else { 3 })
        );
        assert_eq!(
            row.3["failed_count"],
            json!(if partial_row { 2 } else { 0 })
        );
        assert_eq!(row.3["device_id"], json!(row.2.unwrap().to_string()));
        assert_fleet_metadata_is_count_only(&row.3);
    }
    for row in &deleted {
        assert_eq!(row.1, Some(admin_id));
        assert_eq!(row.2, Some(success_a));
        assert_eq!(row.3["operation"], json!("fleet_delete_users"));
        assert_eq!(row.3["succeeded_count"], json!(1));
        assert_eq!(row.3["failed_count"], json!(0));
        assert_fleet_metadata_is_count_only(&row.3);
    }

    assert_eq!(
        dialled_success_targets, 7,
        "two create targets x three accounts plus one delete target x one name"
    );
    assert_eq!(
        dialled_partial_target, 2,
        "third account was never sent after link failure"
    );
    assert_eq!(
        dialled_denied_targets, 0,
        "a denied target is never dialled"
    );
    Ok(())
}

/// An empty fleet create must be refused before the handler dials any device or
/// writes any audit row.
///
/// The target's connection profile points at a closed port, so an unguarded
/// request would dial it and answer a device error — never a 400. Only a guard
/// that runs before `partition_targets`/the device call can produce the 400, and
/// a refused batch leaves no audit row behind.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn fleet_create_rejects_empty_users_before_dialling_or_audit()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    let (device_id, _) = seed_device(&db, "fleet-empty").await?;
    let profile_name = seed_unreachable_cctv_profile(&db, device_id).await?;

    let body = json!({"target_ids": [device_id], "users": []}).to_string();
    let (status, response) = request(
        &app,
        "POST",
        "/api/v1/device-fleet/users:create",
        Some(&body),
        Some(&cookie),
    )
    .await?;

    // Teardown before assertions so a failure does not leave the profile behind.
    drop_cctv_profile(&db, &profile_name).await?;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an empty user batch must be refused before dialling: {}",
        String::from_utf8_lossy(&response)
    );
    assert_eq!(
        audit_count(&db, device_id).await?,
        0,
        "a refused batch writes no audit row"
    );
    Ok(())
}

/// HTTP method per endpoint: `PUT /users`, everything else `POST` except export.
fn method_for(uri: &str) -> &'static str {
    if uri.contains("/export") {
        "GET"
    } else if uri.ends_with("/users") {
        "PUT"
    } else {
        "POST"
    }
}
