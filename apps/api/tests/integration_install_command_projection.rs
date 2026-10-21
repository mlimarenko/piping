//! Integration test for canonical install command projection.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_install_command_projection -- --ignored`

mod common;

use axum::Router;
use axum::http::StatusCode;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
};
use serde_json::Value;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis.
async fn setup_wizard_install_projection_returns_canonical_metadata_for_authenticated_user() {
    let state = build_test_app_state().await.expect("test app state");
    // Build through the canonical app router: the bare REST router lacks the
    // middleware stack (RequestId extension etc.), so handlers that take
    // Extension<RequestId> would 500 with "missing request extension".
    let app: Router = piping_backend::app::build_router(state);
    let (unauthenticated_status, _) =
        request(&app, "GET", "/api/v1/setup/wizard/install", None, None)
            .await
            .expect("unauthenticated setup wizard install request");
    assert_eq!(unauthenticated_status, StatusCode::UNAUTHORIZED);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login cookie");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/setup/wizard/install",
        None,
        Some(&cookie),
    )
    .await
    .expect("setup wizard install request");

    assert_eq!(status, StatusCode::OK);
    let payload: Value = serde_json::from_slice(&body).expect("setup wizard install payload");
    assert_eq!(payload.get("command"), Some(&Value::Null));
    assert_eq!(
        payload.get("publicationStatus").and_then(Value::as_str),
        Some("pending")
    );
    assert_eq!(payload.get("shell").and_then(Value::as_str), Some("bash"));
    assert_eq!(
        payload.get("sourceRepositoryUrl").and_then(Value::as_str),
        Some("https://github.com/mlimarenko/piping")
    );
    assert_eq!(
        payload
            .get("supportedPlatforms")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );
}
