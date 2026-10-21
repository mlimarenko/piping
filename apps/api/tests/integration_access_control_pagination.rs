//! Integration tests for access-control list pagination.
//!
//! These drive the real REST pagination endpoints end-to-end against the
//! `{items,total,limit,offset}` envelope returned by
//! `/api/v1/admin/access-control/{roles,users,groups}`. They exercise the
//! canonical pagination path (`normalize_page` -> repository `list_*_page` ->
//! `PageResult`), including limit clamping and offset behavior.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_access_control_pagination -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
};
use serde_json::Value;

/// Canonical pagination bounds (mirrors `config::constants`).
const DEFAULT_PAGINATION_LIMIT: u64 = 50;
const MAX_PAGINATION_LIMIT: u64 = 1000;

/// Fetches a paginated list endpoint and returns the parsed JSON envelope.
///
/// Asserts a 200 response and that the body carries the canonical
/// `{items,total,limit,offset}` shape.
async fn fetch_page(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(app, "GET", uri, None, Some(cookie))
        .await
        .map_err(|e| format!("request: {e}"))?;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {uri} expected 200, got {status}; body={}",
        String::from_utf8_lossy(&body)
    );
    let json: Value =
        serde_json::from_slice(&body).map_err(|e| format!("response body is JSON: {e}"))?;
    assert!(
        json.get("items").is_some(),
        "envelope missing `items`: {json}"
    );
    assert!(
        json.get("total").is_some(),
        "envelope missing `total`: {json}"
    );
    assert!(
        json.get("limit").is_some(),
        "envelope missing `limit`: {json}"
    );
    assert!(
        json.get("offset").is_some(),
        "envelope missing `offset`: {json}"
    );
    Ok(json)
}

fn items_len(envelope: &Value) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let items = envelope
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "`items` is an array".to_string())?;
    Ok(items.len() as u64)
}

fn field_u64(envelope: &Value, key: &str) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let value = envelope
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("`{key}` is a u64 in {envelope}"))?;
    Ok(value)
}

/// Shared assertions covering the canonical pagination contract for a single
/// access-control list endpoint at `base_path`.
///
/// `minimum_total` is the number of rows guaranteed to be seeded for this
/// resource by `ensure_test_admin` (roles and users seed exactly one;
/// groups are not seeded so their floor is zero).
async fn assert_pagination_contract(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
    minimum_total: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let total = assert_explicit_small_page(app, cookie, base_path, minimum_total).await?;
    assert_default_page(app, cookie, base_path, total).await?;
    assert_limit_clamps(app, cookie, base_path).await?;
    assert_offset_past_end(app, cookie, base_path, total).await?;
    Ok(())
}

/// Explicit small page: limit is echoed, page never exceeds the limit.
async fn assert_explicit_small_page(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
    minimum_total: u64,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let page = fetch_page(app, cookie, &format!("{base_path}?limit=1&offset=0")).await?;
    assert_small_page_echo(&page, base_path)?;
    assert_items_within_limit(&page, base_path, 1)?;
    assert_total_at_least(&page, base_path, minimum_total)
}

/// Echoed `limit`/`offset` of the explicit small page.
fn assert_small_page_echo(
    page: &Value,
    base_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert_eq!(
        field_u64(page, "limit")?,
        1,
        "{base_path}: limit not echoed"
    );
    assert_eq!(
        field_u64(page, "offset")?,
        0,
        "{base_path}: offset not echoed"
    );
    Ok(())
}

/// The returned page never carries more than `max` items.
fn assert_items_within_limit(
    page: &Value,
    base_path: &str,
    max: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert!(
        items_len(page)? <= max,
        "{base_path}: page returned more items than the limit"
    );
    Ok(())
}

/// The envelope total is at least the seeded floor; returns it for later phases.
fn assert_total_at_least(
    page: &Value,
    base_path: &str,
    minimum_total: u64,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let total = field_u64(page, "total")?;
    assert!(
        total >= minimum_total,
        "{base_path}: total {total} below seeded minimum {minimum_total}"
    );
    Ok(total)
}

/// Default page (no query params): limit defaults to the canonical default.
async fn assert_default_page(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
    total: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let default_page = fetch_page(app, cookie, base_path).await?;
    assert_default_page_echo(&default_page, base_path)?;
    assert_total_stable(&default_page, base_path, total)
}

/// Default `limit`/`offset` when the query carries none.
fn assert_default_page_echo(
    page: &Value,
    base_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert_eq!(
        field_u64(page, "limit")?,
        DEFAULT_PAGINATION_LIMIT,
        "{base_path}: missing limit should default to {DEFAULT_PAGINATION_LIMIT}"
    );
    assert_eq!(
        field_u64(page, "offset")?,
        0,
        "{base_path}: missing offset should default to 0"
    );
    Ok(())
}

/// The total is unchanged by a request that only changes the page window.
fn assert_total_stable(
    page: &Value,
    base_path: &str,
    total: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert_eq!(
        field_u64(page, "total")?,
        total,
        "{base_path}: total must be stable across page requests"
    );
    Ok(())
}

/// Limit below the floor and above the ceiling both clamp to canonical bounds.
async fn assert_limit_clamps(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert_limit_floor_clamp(app, cookie, base_path).await?;
    assert_limit_ceiling_clamp(app, cookie, base_path).await
}

/// Limit below the floor (0) clamps up to 1.
async fn assert_limit_floor_clamp(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clamped_low = fetch_page(app, cookie, &format!("{base_path}?limit=0")).await?;
    assert_eq!(
        field_u64(&clamped_low, "limit")?,
        1,
        "{base_path}: limit=0 should clamp up to 1"
    );
    Ok(())
}

/// Limit above the ceiling clamps down to the canonical maximum.
async fn assert_limit_ceiling_clamp(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clamped_high = fetch_page(app, cookie, &format!("{base_path}?limit=100000")).await?;
    assert_eq!(
        field_u64(&clamped_high, "limit")?,
        MAX_PAGINATION_LIMIT,
        "{base_path}: oversized limit should clamp to {MAX_PAGINATION_LIMIT}"
    );
    Ok(())
}

/// Offset past the end yields an empty page but preserves the total.
async fn assert_offset_past_end(
    app: &axum::Router,
    cookie: &str,
    base_path: &str,
    total: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let past_end = fetch_page(
        app,
        cookie,
        &format!("{base_path}?limit=10&offset={}", total + 1_000),
    )
    .await?;
    assert_eq!(
        items_len(&past_end)?,
        0,
        "{base_path}: offset past the end should return no items"
    );
    assert_total_independent_of_offset(&past_end, base_path, total)?;
    Ok(())
}

/// The total is unchanged by an offset past the last row.
fn assert_total_independent_of_offset(
    page: &Value,
    base_path: &str,
    total: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    assert_eq!(
        field_u64(page, "total")?,
        total,
        "{base_path}: total must be independent of offset"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_access_control_pagination -- --ignored
async fn roles_page_pagination_scaffold() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // `ensure_test_admin` seeds exactly one role (`test-admin-role`).
    assert_pagination_contract(&app, &cookie, "/api/v1/admin/access-control/roles", 1).await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_access_control_pagination -- --ignored
async fn users_page_pagination_scaffold() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // `ensure_test_admin` seeds exactly one user (`test-admin`).
    assert_pagination_contract(&app, &cookie, "/api/v1/admin/access-control/users", 1).await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_access_control_pagination -- --ignored
async fn groups_page_pagination_scaffold() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // Groups are not seeded by `ensure_test_admin`; floor is zero.
    assert_pagination_contract(&app, &cookie, "/api/v1/admin/access-control/groups", 0).await?;
    Ok(())
}
