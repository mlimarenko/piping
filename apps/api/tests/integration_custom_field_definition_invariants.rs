//! Regression tests for two custom-field invariants that were fixed without
//! a test (43a76a65e).
//!
//! * `is_secret` may not be toggled on an existing definition. Flipping it
//!   either strands the plaintext values already in `custom_field_data` or
//!   promises secrecy for values that were never stored as secrets — the fix
//!   refuses the change instead of pretending either way is fine.
//! * `enforce_unique` is refused at definition time rather than accepted and
//!   silently unenforced, which is what a stored-but-ignored flag amounts to.
//!
//! Both live behind a single `if` in the service. Nothing pinned either, so
//! removing the check would have restored the original bug invisibly.

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};
use serde_json::json;

const DEFINITIONS: &str = "/api/v1/custom-fields";

async fn create_definition(
    app: &axum::Router,
    cookie: &str,
    body: &serde_json::Value,
) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    request(
        app,
        "POST",
        DEFINITIONS,
        Some(&body.to_string()),
        Some(cookie),
    )
    .await
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn is_secret_cannot_be_toggled_on_an_existing_definition()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    let name = format!("secret_toggle_{}", unique_suffix());
    let (status, body) = create_definition(
        &app,
        &cookie,
        &json!({
            "name": name,
            "label": name,
            "field_type": "text",
            "is_secret": false,
        }),
    )
    .await?;
    assert!(
        status.is_success(),
        "create: {}",
        String::from_utf8_lossy(&body)
    );
    let id = serde_json::from_slice::<serde_json::Value>(&body)?
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("definition id")?
        .to_string();

    // Plain -> secret. Existing plaintext values would be stranded.
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("{DEFINITIONS}/{id}"),
        Some(&json!({ "is_secret": true }).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "promoting an existing field to secret must be refused"
    );

    // And the stored definition is untouched.
    let (status, body) = request(
        &app,
        "GET",
        &format!("{DEFINITIONS}/{id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let stored: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        stored.get("is_secret").and_then(serde_json::Value::as_bool),
        Some(false),
        "the refused toggle must not have been applied"
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("{DEFINITIONS}/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn enforce_unique_is_refused_rather_than_stored_and_ignored()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    let name = format!("enforce_unique_{}", unique_suffix());
    let (status, body) = create_definition(
        &app,
        &cookie,
        &json!({
            "name": name,
            "label": name,
            "field_type": "text",
            "enforce_unique": true,
        }),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unenforceable uniqueness flag must be refused at definition time, \
         not accepted and quietly ignored: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn a_secret_definition_stays_secret_across_an_unrelated_update()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;

    let name = format!("stays_secret_{}", unique_suffix());
    let (status, body) = create_definition(
        &app,
        &cookie,
        &json!({
            "name": name,
            "label": name,
            "field_type": "text",
            "is_secret": true,
        }),
    )
    .await?;
    assert!(
        status.is_success(),
        "create: {}",
        String::from_utf8_lossy(&body)
    );
    let id = serde_json::from_slice::<serde_json::Value>(&body)?
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("definition id")?
        .to_string();

    // Editing the label must not be a back door to demoting a secret field:
    // the merge that fills unspecified fields from the stored row is exactly
    // the shape that erased data elsewhere in this codebase.
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("{DEFINITIONS}/{id}"),
        Some(&json!({ "label": "renamed" }).to_string()),
        Some(&cookie),
    )
    .await?;
    assert!(status.is_success(), "an unrelated edit must succeed");

    let (_, body) = request(
        &app,
        "GET",
        &format!("{DEFINITIONS}/{id}"),
        None,
        Some(&cookie),
    )
    .await?;
    let stored: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        stored.get("is_secret").and_then(serde_json::Value::as_bool),
        Some(true),
        "an unrelated edit must not demote a secret field"
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("{DEFINITIONS}/{id}"),
        None,
        Some(&cookie),
    )
    .await;
    Ok(())
}
