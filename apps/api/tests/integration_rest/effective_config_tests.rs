//! Effective-connection-config REST tests.
//!
//! **The product question these tests were blocked on is answered.** The old
//! version bound a **config context** to a connection target and expected it to
//! surface in the effective-config response. It cannot: the route delegates to
//! `resolve_effective_connection_config_response` in
//! `routes/connection_config.rs`, which resolves `connconf_record` +
//! `connconf_assignment` and never reads config contexts. Nothing in the code
//! feeds one into the other, and config contexts remain their own feature
//! (inventory config rendering). So config contexts are not a regressed input
//! here — these tests were simply left behind by the move to `connconf`.
//!
//! They also asserted a `{config, secret_presence}` body. That shape is gone:
//! the endpoint answers `{protocol, params, attribution, resolution_chain}`.
//!
//! Rewritten against the canonical model, keeping the behaviour that still
//! matters: the most specific scope wins leaf by leaf while less specific
//! values are inherited, an edit is visible on the next read (the stale-cache
//! guard this file existed for), and a device with nothing bound answers 200
//! with an empty result rather than an error.
//!
//! These are no longer excluded by name in CI.

use super::*;
use serde_json::{Value, json};

/// Create a connconf record of the given protocol and return its id.
async fn create_record(
    app: &Router,
    cookie: &str,
    protocol: &str,
    params: Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let payload = json!({
        "name": format!("effective-config-{}", unique_suffix()),
        "protocol": protocol,
        "params": params,
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
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create record: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(serde_json::from_slice::<Value>(&body)?
        .get("id")
        .and_then(Value::as_str)
        .ok_or("create record: response missing string `id`")?
        .to_string())
}

/// Replace a record's params through the public update route.
async fn update_record_params(
    app: &Router,
    cookie: &str,
    record_id: &str,
    params: Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let payload = json!({ "params": params }).to_string();
    let (status, body) = request(
        app,
        "PUT",
        &format!("/api/v1/connection-config/records/{record_id}"),
        Some(&payload),
        Some(cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "update record: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(())
}

/// Bind a record at a scope and return the assignment id. `scope_id` is `None`
/// for the global scope.
async fn assign_record(
    app: &Router,
    cookie: &str,
    record_id: &str,
    scope_type: &str,
    scope_id: Option<Uuid>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut payload = json!({
        "record_id": record_id,
        "scope_type": scope_type,
    });
    if let Some(id) = scope_id {
        payload["scope_id"] = Value::String(id.to_string());
    }
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/connection-config/assignments",
        Some(&payload.to_string()),
        Some(cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create assignment: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(serde_json::from_slice::<Value>(&body)?
        .get("id")
        .and_then(Value::as_str)
        .ok_or("create assignment: response missing string `id`")?
        .to_string())
}

/// Remove an assignment. A global one is visible to every device, so the test
/// that created it has to take it back down — otherwise it silently changes
/// what every other test in this database sees.
async fn delete_assignment(
    app: &Router,
    cookie: &str,
    assignment_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "DELETE",
        &format!("/api/v1/connection-config/assignments/{assignment_id}"),
        None,
        Some(cookie),
    )
    .await?;
    assert!(
        status == StatusCode::NO_CONTENT || status == StatusCode::OK,
        "delete assignment: {status} {}",
        String::from_utf8_lossy(&body)
    );
    Ok(())
}

async fn effective_config(
    app: &Router,
    cookie: &str,
    device_id: Uuid,
    protocol: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "GET",
        &format!("/api/v1/connection-config/effective/{device_id}?protocol={protocol}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "effective config: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(serde_json::from_slice(&body)?)
}

/// A device-scoped record must override the global one leaf by leaf, while
/// leaves only the global record sets are still inherited. That ladder is the
/// whole point of the model, so it is the first thing worth pinning.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn effective_config_lets_the_most_specific_scope_win_and_inherits_the_rest()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (device_id, _) = common::seed_device(&pool, "effcfg-specificity").await?;

    let global = create_record(
        &app,
        &cookie,
        "ssh",
        json!({ "host": "10.0.0.1", "port": 22 }),
    )
    .await?;
    let global_assignment = assign_record(&app, &cookie, &global, "global", None).await?;

    let device_scoped =
        create_record(&app, &cookie, "ssh", json!({ "host": "10.40.0.10" })).await?;
    let _device_assignment =
        assign_record(&app, &cookie, &device_scoped, "device", Some(device_id)).await?;

    let effective = effective_config(&app, &cookie, device_id, "ssh").await?;

    assert_eq!(
        effective.pointer("/params/host"),
        Some(&Value::String("10.40.0.10".to_string())),
        "the device-scoped record must win the leaf it sets: {effective}"
    );
    assert_eq!(
        effective.pointer("/params/port").and_then(Value::as_i64),
        Some(22),
        "a leaf only the global record sets must still be inherited: {effective}"
    );

    let chain: Vec<String> = effective
        .get("resolution_chain")
        .and_then(Value::as_array)
        .ok_or("response missing resolution_chain")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let global_position = chain.iter().position(|id| id == &global);
    let device_position = chain.iter().position(|id| id == &device_scoped);
    assert!(
        global_position.is_some() && device_position.is_some(),
        "both records must appear in the resolution chain, got {chain:?}"
    );
    assert!(
        global_position < device_position,
        "the chain runs least to most specific, got {chain:?}"
    );

    assert_eq!(
        effective
            .pointer("/attribution/host/record_id")
            .and_then(Value::as_str),
        Some(device_scoped.as_str()),
        "attribution must name the record that actually won the leaf: {effective}"
    );

    // The device-scoped assignment only affects this test's own device, but the
    // global one reaches every device in the database — put it back.
    delete_assignment(&app, &cookie, &global_assignment).await?;

    Ok(())
}

/// Editing a record must be visible on the next read. The resolved config is
/// cached, so this is the stale-cache guard the original file existed for.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn effective_config_reflects_a_record_update_without_serving_a_stale_cache()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (device_id, _) = common::seed_device(&pool, "effcfg-cache").await?;
    let record = create_record(&app, &cookie, "ssh", json!({ "host": "10.40.0.10" })).await?;
    let _assignment = assign_record(&app, &cookie, &record, "device", Some(device_id)).await?;

    let before = effective_config(&app, &cookie, device_id, "ssh").await?;
    assert_eq!(
        before.pointer("/params/host"),
        Some(&Value::String("10.40.0.10".to_string())),
        "{before}"
    );

    update_record_params(&app, &cookie, &record, json!({ "host": "10.40.0.99" })).await?;

    let after = effective_config(&app, &cookie, device_id, "ssh").await?;
    assert_eq!(
        after.pointer("/params/host"),
        Some(&Value::String("10.40.0.99".to_string())),
        "the update must be visible on the next read, not served from cache: {after}"
    );

    Ok(())
}

/// A device with nothing bound to it is a normal state, not an error: the
/// endpoint must answer 200 with an empty result so callers can render "no
/// configuration" instead of special-casing a 404.
///
/// It asks about `app`, a protocol no test in this file creates a record for.
/// Asking about `ssh` would make the assertion depend on every other test in
/// the database having cleaned up its global assignments, which is exactly the
/// kind of order coupling this suite should not have.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn effective_config_returns_an_empty_result_for_a_device_without_assignments()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (device_id, _) = common::seed_device(&pool, "effcfg-empty").await?;

    let effective = effective_config(&app, &cookie, device_id, "app").await?;

    assert_eq!(
        effective.get("protocol").and_then(Value::as_str),
        Some("app"),
        "{effective}"
    );
    assert_eq!(
        effective
            .get("resolution_chain")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "no assignments means no records participated: {effective}"
    );
    assert_eq!(
        effective.get("params"),
        Some(&json!({})),
        "params must be an empty object, not null: {effective}"
    );

    Ok(())
}
