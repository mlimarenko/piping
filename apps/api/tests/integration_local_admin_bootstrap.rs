//! Regression coverage for the configured local emergency administrator.
//!
//! `auth.local_admin_password` is the single production contract for the local
//! `admin` credential: when set, startup rotates that one credential
//! idempotently and leaves every other account alone.
//!
//! Requires PostgreSQL and Redis. Run with:
//! `cargo test --test integration_local_admin_bootstrap -- --ignored`

mod common;

use std::sync::Arc;

use axum::http::StatusCode;

use common::{build_test_app_state_with, request};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

const ADMIN_USERNAME: &str = "admin";
const ORIGINAL_PASSWORD: &str = "original-emergency-pass-1";
const ROTATED_PASSWORD: &str = "rotated-emergency-pass-2";

fn login_body(password: &str) -> String {
    serde_json::json!({
        "identifier": ADMIN_USERNAME,
        "password": password,
    })
    .to_string()
}

async fn login(app: &axum::Router, password: &str) -> Result<StatusCode, BoxError> {
    let body = login_body(password);
    let (status, _) = request(app, "POST", "/api/v1/auth/login", Some(&body), None).await?;
    Ok(status)
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn configured_override_rotates_the_canonical_local_credential_idempotently() -> TestResult {
    let state = build_test_app_state_with(|settings| {
        settings.auth.local_admin_password = Some(ORIGINAL_PASSWORD.to_string());
    })
    .await?;
    let app = piping_backend::app::build_router(state.clone());

    assert_eq!(
        login(&app, ORIGINAL_PASSWORD).await?,
        StatusCode::OK,
        "the configured override must be the credential the canonical admin signs in with"
    );

    // Re-running startup with a new override rotates the same canonical account:
    // the old password stops working and the new one works.
    let mut rotated_settings = (*state.settings).clone();
    rotated_settings.auth.local_admin_password = Some(ROTATED_PASSWORD.to_string());
    let rotated = piping_backend::app::AppState::build(
        Arc::new(rotated_settings.clone()),
        state.db_pool.clone(),
        state.redis.clone(),
    )
    .await?;
    let rotated_app = piping_backend::app::build_router(rotated);

    assert_eq!(
        login(&rotated_app, ORIGINAL_PASSWORD).await?,
        StatusCode::UNAUTHORIZED,
        "rotation must retire the previous local-admin password"
    );
    assert_eq!(
        login(&rotated_app, ROTATED_PASSWORD).await?,
        StatusCode::OK,
        "rotation must activate the new local-admin password"
    );

    // Applying the same override again is idempotent: the credential keeps
    // authenticating, and no other account's credential is involved.
    let again = piping_backend::app::AppState::build(
        Arc::new(rotated_settings),
        state.db_pool.clone(),
        state.redis.clone(),
    )
    .await?;
    let again_app = piping_backend::app::build_router(again);

    assert_eq!(
        login(&again_app, ROTATED_PASSWORD).await?,
        StatusCode::OK,
        "reapplying the same override must keep the credential working"
    );

    Ok(())
}
