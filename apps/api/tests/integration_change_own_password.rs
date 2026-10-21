//! Protective tests for `PUT /api/v1/user/settings/password`.
//!
//! This endpoint had ZERO tests. It is the one place a user re-proves who they
//! are with a single check — `verify_local_password_for_user` — and on the
//! strength of that check it rewrites their credential. Nothing pinned that
//! the check runs at all, that a wrong current password is refused, or that a
//! rejected attempt leaves the old password working.
//!
//! These are deliberately behavioural: they drive the real HTTP surface and
//! then prove the outcome by logging in, rather than asserting on internals.

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, request, request_with_auth,
    unique_suffix,
};
use serde_json::json;

const PASSWORD_PATH: &str = "/api/v1/user/settings/password";

async fn login(
    app: &axum::Router,
    username: &str,
    password: &str,
) -> Result<StatusCode, Box<dyn std::error::Error + Send + Sync>> {
    let body = json!({ "identifier": username, "password": password }).to_string();
    let (status, _) = request(app, "POST", "/api/v1/auth/login", Some(&body), None).await?;
    Ok(status)
}

/// Creates a local user through the admin API and returns (id, username, password).
async fn seed_local_user(
    app: &axum::Router,
    admin_cookie: &str,
) -> Result<(String, String, String), Box<dyn std::error::Error + Send + Sync>> {
    let username = format!("pwd-change-{}", unique_suffix());
    let password = "Initial-Password-1!".to_string();
    // Same shape the admin API actually accepts (mirrors the helper in
    // integration_rest/license_tests.rs): status and auth_source are required,
    // and the password rides in a `credential` command.
    let body = json!({
        "username": username,
        "email": format!("{username}@test.local"),
        "status": "active",
        "auth_source": "local",
        "credential": { "mode": "manual", "password": password },
    })
    .to_string();
    let (status, response) = request(
        app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&body),
        Some(admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "seed user: {}",
        String::from_utf8_lossy(&response)
    );
    let payload: serde_json::Value = serde_json::from_slice(&response)?;
    let id = payload
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("created user has no id")?
        .to_string();
    Ok((id, username, password))
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn change_own_password_replaces_the_credential()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let admin_cookie = common::login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let (user_id, username, password) = seed_local_user(&app, &admin_cookie).await?;
    let user_cookie = common::login_and_get_cookie(&app, &username, &password)
        .await
        .ok_or("user login")?;

    let new_password = "Rotated-Password-2!";
    let body = json!({ "current_password": password, "new_password": new_password }).to_string();
    let (status, response) = request_with_auth(
        &app,
        "PUT",
        PASSWORD_PATH,
        Some(&body),
        Some(&user_cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "change must succeed: {}",
        String::from_utf8_lossy(&response)
    );

    assert_eq!(
        login(&app, &username, new_password).await?,
        StatusCode::OK,
        "the new password must work"
    );
    assert_eq!(
        login(&app, &username, &password).await?,
        StatusCode::UNAUTHORIZED,
        "the old password must stop working"
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn wrong_current_password_is_refused_and_changes_nothing()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let admin_cookie = common::login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let (user_id, username, password) = seed_local_user(&app, &admin_cookie).await?;
    let user_cookie = common::login_and_get_cookie(&app, &username, &password)
        .await
        .ok_or("user login")?;

    // The single check that stands between a hijacked session and a permanent
    // account takeover. If it ever stops running, this is what catches it.
    let body = json!({
        "current_password": "Not-The-Current-Password-9!",
        "new_password": "Attacker-Chosen-3!",
    })
    .to_string();
    let (status, _) = request_with_auth(
        &app,
        "PUT",
        PASSWORD_PATH,
        Some(&body),
        Some(&user_cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a wrong current password must be refused"
    );

    assert_eq!(
        login(&app, &username, &password).await?,
        StatusCode::OK,
        "the original password must still work after a refused change"
    );
    assert_eq!(
        login(&app, &username, "Attacker-Chosen-3!").await?,
        StatusCode::UNAUTHORIZED,
        "the attempted password must never become valid"
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn anonymous_caller_cannot_change_a_password()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);

    let body = json!({ "current_password": "whatever", "new_password": "Whatever-1!" }).to_string();
    let (status, _) = request(&app, "PUT", PASSWORD_PATH, Some(&body), None).await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the endpoint must reject an unauthenticated caller"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn new_password_must_satisfy_the_configured_policy()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let admin_cookie = common::login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let (user_id, username, password) = seed_local_user(&app, &admin_cookie).await?;
    let user_cookie = common::login_and_get_cookie(&app, &username, &password)
        .await
        .ok_or("user login")?;

    let body = json!({ "current_password": password, "new_password": "x" }).to_string();
    let (status, _) = request_with_auth(
        &app,
        "PUT",
        PASSWORD_PATH,
        Some(&body),
        Some(&user_cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a password below the configured minimum must be rejected"
    );
    assert_eq!(
        login(&app, &username, &password).await?,
        StatusCode::OK,
        "a rejected change must leave the credential untouched"
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await;
    Ok(())
}
