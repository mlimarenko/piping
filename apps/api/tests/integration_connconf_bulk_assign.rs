//! Integration tests for the connection-config bulk-assign endpoint
//! (`POST /api/v1/connection-config/records/{id}/assignments/bulk`).
//!
//! Exercises the "shared-credential group" tooling gap fix end to end: a
//! `connconf_record` fanned out to many device/VM targets in one call instead
//! of N individual `POST /connection-config/assignments` calls.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_connconf_bulk_assign -- --ignored --test-threads=1`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie, request, seed_device,
};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Seed a minimal `virtualization_virtualmachine` row (no site/cluster FK is
/// required to resolve `get_device_scopes` for a bare vm target); return its id.
async fn seed_vm(pool: &sqlx::PgPool) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    let suffix = common::unique_suffix();
    sqlx::query(
        "INSERT INTO virtualization_virtualmachine (custom_field_data, id, name, status, \
         comments, description, interface_count, virtual_disk_count, serial, start_on_boot) \
         VALUES ('{}', $1, $2, 'active', '', '', 0, 0, '', 'inherit')",
    )
    .bind(id)
    .bind(format!("bulk-assign-vm-{suffix}"))
    .execute(pool)
    .await
    .map_err(|e| format!("seed vm: {e}"))?;
    Ok(id)
}

/// Create a `cctv`-protocol connconf record carrying shared credentials
/// through the REST API (the "shared-credential group" record under test);
/// return its id.
async fn create_shared_cctv_record(
    app: &axum::Router,
    cookie: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let suffix = common::unique_suffix();
    let payload = serde_json::json!({
        "name": format!("bulk-assign-shared-{suffix}"),
        "protocol": "cctv",
        "params": {
            "credentials": { "username": "admin", "password": "shared-secret" }
        }
    })
    .to_string();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/connection-config/records",
        Some(&payload),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create shared record");
    let record: Value = serde_json::from_slice(&body)?;
    let id = record
        .get("id")
        .and_then(Value::as_str)
        .ok_or("create shared record: response missing string `id`")?;
    Ok(id.to_string())
}

async fn bulk_assign(
    app: &axum::Router,
    cookie: &str,
    record_id: &str,
    target_ids: &[Uuid],
) -> Result<(StatusCode, Value), Box<dyn std::error::Error + Send + Sync>> {
    let targets: Vec<Value> = target_ids
        .iter()
        .map(|id| serde_json::json!({ "target_id": id.to_string() }))
        .collect();
    let payload = serde_json::json!({ "targets": targets }).to_string();
    let (status, body) = request(
        app,
        "POST",
        &format!("/api/v1/connection-config/records/{record_id}/assignments/bulk"),
        Some(&payload),
        Some(cookie),
    )
    .await?;
    let outcome: Value = serde_json::from_slice(&body)?;
    Ok((status, outcome))
}

async fn count_assignments_for_record(
    app: &axum::Router,
    cookie: &str,
    record_id: &str,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "GET",
        &format!("/api/v1/connection-config/assignments?record_id={record_id}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&body)?;
    let total = list
        .get("total")
        .and_then(Value::as_i64)
        .ok_or("list assignments: response missing integer `total`")?;
    Ok(total)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_connconf_bulk_assign -- --ignored
async fn bulk_assign_creates_one_assignment_per_target()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let record_id = create_shared_cctv_record(&app, &cookie).await?;
    let device_a = seed_device(&pool, "bulk-assign").await?.0;
    let device_b = seed_device(&pool, "bulk-assign").await?.0;
    let vm_a = seed_vm(&pool).await?;

    let (status, outcome) =
        bulk_assign(&app, &cookie, &record_id, &[device_a, device_b, vm_a]).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        outcome.get("assigned_count").and_then(Value::as_i64),
        Some(3)
    );
    assert_eq!(
        outcome
            .get("already_assigned_count")
            .and_then(Value::as_i64),
        Some(0)
    );
    assert_eq!(outcome.get("failed_count").and_then(Value::as_i64), Some(0));
    let items = outcome
        .get("items")
        .and_then(Value::as_array)
        .expect("items");
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item.get("status").and_then(Value::as_str), Some("assigned"));
        assert!(
            item.get("assignment").is_some_and(|a| !a.is_null()),
            "an assigned item must carry the created assignment"
        );
    }

    assert_eq!(
        count_assignments_for_record(&app, &cookie, &record_id).await?,
        3
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_connconf_bulk_assign -- --ignored
async fn bulk_assign_is_idempotent_on_repeat_call()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let record_id = create_shared_cctv_record(&app, &cookie).await?;
    let device_a = seed_device(&pool, "bulk-assign").await?.0;
    let device_b = seed_device(&pool, "bulk-assign").await?.0;

    let (status, first) = bulk_assign(&app, &cookie, &record_id, &[device_a, device_b]).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first.get("assigned_count").and_then(Value::as_i64), Some(2));

    // Repeat call with the SAME targets: must not violate the
    // `uq_connconf_assignment` unique index, and must not create new rows.
    let (status, second) = bulk_assign(&app, &cookie, &record_id, &[device_a, device_b]).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "a repeat bulk-assign call must not error"
    );
    assert_eq!(
        second.get("assigned_count").and_then(Value::as_i64),
        Some(0)
    );
    assert_eq!(
        second.get("already_assigned_count").and_then(Value::as_i64),
        Some(2)
    );
    assert_eq!(second.get("failed_count").and_then(Value::as_i64), Some(0));

    assert_eq!(
        count_assignments_for_record(&app, &cookie, &record_id).await?,
        2,
        "repeat bulk-assign must not create duplicate assignment rows"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_connconf_bulk_assign -- --ignored
async fn concurrent_identical_bulk_assign_requests_are_idempotent()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let record_id = create_shared_cctv_record(&app, &cookie).await?;
    let device = seed_device(&pool, "bulk-assign").await?.0;

    let targets = [device];
    let (first, second) = tokio::join!(
        bulk_assign(&app, &cookie, &record_id, &targets),
        bulk_assign(&app, &cookie, &record_id, &targets),
    );
    let (first_status, first_outcome) = first?;
    let (second_status, second_outcome) = second?;

    for (status, outcome) in [
        (first_status, &first_outcome),
        (second_status, &second_outcome),
    ] {
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            outcome.get("failed_count").and_then(Value::as_i64),
            Some(0),
            "a concurrent unique conflict must be classified as already assigned: {outcome}"
        );
    }

    let assigned_count = first_outcome
        .get("assigned_count")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        + second_outcome
            .get("assigned_count")
            .and_then(Value::as_i64)
            .unwrap_or_default();
    let already_assigned_count = first_outcome
        .get("already_assigned_count")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        + second_outcome
            .get("already_assigned_count")
            .and_then(Value::as_i64)
            .unwrap_or_default();
    assert_eq!(assigned_count, 1);
    assert_eq!(already_assigned_count, 1);
    assert_eq!(
        count_assignments_for_record(&app, &cookie, &record_id).await?,
        1,
        "parallel identical requests must persist exactly one assignment"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_connconf_bulk_assign -- --ignored
async fn bulk_assign_rejects_over_max_targets()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // The cap is enforced before any I/O (including the record lookup), so a
    // record that does not even exist is enough to prove the request never
    // reaches the per-target loop.
    let record_id = Uuid::now_v7();
    let target_ids: Vec<Uuid> = (0..1001).map(|_| Uuid::now_v7()).collect();

    let (status, _outcome) =
        bulk_assign(&app, &cookie, &record_id.to_string(), &target_ids).await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a target list over MAX_BULK_ASSIGN_TARGETS must be rejected up front"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_connconf_bulk_assign -- --ignored
async fn bulk_assign_reports_per_item_failure_without_aborting_the_batch()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = build_test_db_pool().await.expect("db pool");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let record_id = create_shared_cctv_record(&app, &cookie).await?;
    let device_a = seed_device(&pool, "bulk-assign").await?.0;
    // No device/vm row exists for this id: `get_device_scopes` resolves to
    // `None`, and `classify_bulk_target` turns that into a per-item failure.
    let missing_target = Uuid::now_v7();

    let (status, outcome) =
        bulk_assign(&app, &cookie, &record_id, &[device_a, missing_target]).await?;

    assert_eq!(
        status,
        StatusCode::OK,
        "one bad target must not fail the whole batch"
    );
    assert_eq!(
        outcome.get("assigned_count").and_then(Value::as_i64),
        Some(1)
    );
    assert_eq!(outcome.get("failed_count").and_then(Value::as_i64), Some(1));

    let items = outcome
        .get("items")
        .and_then(Value::as_array)
        .expect("items");
    let assigned_item = items
        .iter()
        .find(|item| {
            item.get("target_id").and_then(Value::as_str) == Some(device_a.to_string().as_str())
        })
        .expect("assigned item present");
    assert_eq!(
        assigned_item.get("status").and_then(Value::as_str),
        Some("assigned")
    );

    let failed_item = items
        .iter()
        .find(|item| {
            item.get("target_id").and_then(Value::as_str)
                == Some(missing_target.to_string().as_str())
        })
        .expect("failed item present");
    assert_eq!(
        failed_item.get("status").and_then(Value::as_str),
        Some("failed")
    );
    assert!(
        failed_item
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("device or vm")),
        "the failed item must carry an explanatory error"
    );

    // The successful target must have been persisted despite its sibling's
    // failure — the batch is per-item, not all-or-nothing.
    assert_eq!(
        count_assignments_for_record(&app, &cookie, &record_id).await?,
        1
    );
    Ok(())
}
