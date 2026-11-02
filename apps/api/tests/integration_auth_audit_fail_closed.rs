//! Regression coverage for mandatory sign-in timeline records.
//!
//! The "session insert and audit insert commit in one transaction" ordering is
//! covered behaviourally by
//! `every_local_signin_path_fails_when_its_timeline_insert_fails`: a rejecting
//! audit trigger makes the sign-in fail and leaves no active session, so no
//! source-text mirror of the statement order is kept here.
//!
//! Requires PostgreSQL and Redis. Run with:
//! `cargo test --test integration_auth_audit_fail_closed -- --ignored`

mod common;
#[allow(
    dead_code,
    reason = "a #[path] include compiles the whole support module into THIS target, \
and this one needs a single helper from it; the rest is used by the sibling \
license targets that include the same file"
)]
#[path = "support/license_activation_support.rs"]
mod license_activation_support;

use std::sync::Arc;

use axum::http::StatusCode;
use uuid::Uuid;

use common::{TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, request};
use license_activation_support::build_test_entitlement_token;
use piping_backend::contexts::access_control::application::CreateUserApiTokenInput;
use piping_backend::contexts::access_control::domain::{Action, ResourceType, UserApiTokenScope};
use piping_backend::errors::AppError;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn every_local_signin_path_fails_when_its_timeline_insert_fails() -> TestResult {
    let state = build_test_app_state().await?;
    let pool = state.db_pool.clone();
    install_rejecting_signin_audit_trigger(&pool).await?;

    let enabled_app = piping_backend::app::build_router(state.clone());

    let missing_credentials = login(&enabled_app, "", "").await?;
    assert_eq!(missing_credentials, StatusCode::INTERNAL_SERVER_ERROR);

    let invalid_credentials = login(&enabled_app, "not-a-user", "not-the-password").await?;
    assert_eq!(invalid_credentials, StatusCode::INTERNAL_SERVER_ERROR);

    let mut disabled_settings = (*state.settings).clone();
    disabled_settings.auth.local_enabled = false;
    let disabled_state = piping_backend::app::AppState::build(
        Arc::new(disabled_settings),
        pool.clone(),
        state.redis.clone(),
    )
    .await?;
    let disabled_app = piping_backend::app::build_router(disabled_state);
    let disabled = login(&disabled_app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD).await?;
    assert_eq!(disabled, StatusCode::INTERNAL_SERVER_ERROR);

    let seat_holders = fill_default_free_tier(&pool).await?;
    let license_denied = login(&enabled_app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD).await?;
    assert_eq!(license_denied, StatusCode::INTERNAL_SERVER_ERROR);
    delete_seat_holders(&pool, &seat_holders).await?;

    let successful_credentials =
        login(&enabled_app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD).await?;
    assert_eq!(successful_credentials, StatusCode::INTERNAL_SERVER_ERROR);

    let active_sessions: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM auth_session
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(Uuid::parse_str("00000000-0000-7000-8000-000000000001")?)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        active_sessions, 0,
        "the session insert must roll back with the rejected audit insert"
    );

    uninstall_rejecting_signin_audit_trigger(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn every_seat_decision_fails_when_its_ledger_insert_fails() -> TestResult {
    let state = build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let user_id = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    install_rejecting_seat_ledger_trigger(&pool).await?;

    let assignment_error = state
        .services
        .licensing
        .enforce_seat_on_login(user_id)
        .await
        .expect_err("seat assignment must fail when its ledger insert fails");
    assert!(matches!(assignment_error, AppError::Database(_)));
    assert_eq!(active_seat_count_for(&pool, user_id).await?, 0);

    reset_license_state(&pool).await?;
    let seat_holders = fill_default_free_tier(&pool).await?;
    let capacity_error = state
        .services
        .licensing
        .enforce_seat_on_login(user_id)
        .await
        .expect_err("seat-limit denial must fail when its ledger insert fails");
    assert!(matches!(capacity_error, AppError::Database(_)));
    delete_seat_holders(&pool, &seat_holders).await?;

    reset_license_state(&pool).await?;
    insert_license_state(
        &pool,
        "not-a-jwt",
        chrono::Utc::now() + chrono::Duration::hours(1),
    )
    .await?;
    let invalid_error = state
        .services
        .licensing
        .enforce_seat_on_login(user_id)
        .await
        .expect_err("invalid-entitlement denial must fail when its ledger insert fails");
    assert!(matches!(invalid_error, AppError::Database(_)));

    reset_license_state(&pool).await?;
    let expired_at = chrono::Utc::now() - chrono::Duration::minutes(1);
    let expired_token = build_test_entitlement_token("audit-seat-test", 5, expired_at)?;
    insert_license_state(&pool, &expired_token, expired_at).await?;
    let expired_error = state
        .services
        .licensing
        .enforce_seat_on_login(user_id)
        .await
        .expect_err("expired-entitlement denial must fail when its ledger insert fails");
    assert!(matches!(expired_error, AppError::Database(_)));

    uninstall_rejecting_seat_ledger_trigger(&pool).await?;
    reset_license_state(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn api_token_mutations_fail_when_their_audit_insert_fails() -> TestResult {
    let state = build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let user_id = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    let service = state.services.user_api_tokens.clone();
    let now = chrono::Utc::now();

    let existing = service
        .create_for_user(user_id, token_input("revoke-audit-fixture"), now)
        .await?;
    install_rejecting_api_token_audit_trigger(&pool).await?;

    let create_error = service
        .create_for_user(user_id, token_input("create-audit-failure"), now)
        .await
        .expect_err("token creation must fail when its audit insert fails");
    assert!(matches!(create_error, AppError::Database(_)));
    let failed_create_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_user_api_token WHERE name = 'create-audit-failure'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        failed_create_count, 0,
        "failed token creation must roll back"
    );

    let revoke_error = service
        .revoke_for_user(user_id, existing.token.id, now)
        .await
        .expect_err("token revocation must fail when its audit insert fails");
    assert!(matches!(revoke_error, AppError::Database(_)));
    let status: String = sqlx::query_scalar("SELECT status FROM auth_user_api_token WHERE id = $1")
        .bind(existing.token.id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(status, "active", "failed token revocation must roll back");

    uninstall_rejecting_api_token_audit_trigger(&pool).await?;
    Ok(())
}

fn token_input(name: &str) -> CreateUserApiTokenInput {
    CreateUserApiTokenInput {
        name: name.to_string(),
        scopes: vec![UserApiTokenScope {
            action: Action::Read,
            resource_type: ResourceType::Inventory,
            resource_pattern: "*".to_string(),
        }],
        expires_in_seconds: Some(3_600),
    }
}

async fn login(
    app: &axum::Router,
    identifier: &str,
    password: &str,
) -> Result<StatusCode, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "identifier": identifier,
        "password": password,
    })
    .to_string();
    let (status, _) = request(app, "POST", "/api/v1/auth/login", Some(&body), None).await?;
    Ok(status)
}

async fn install_rejecting_signin_audit_trigger(pool: &sqlx::PgPool) -> TestResult {
    sqlx::query("DROP TRIGGER IF EXISTS reject_signin_audit_for_fail_closed_test ON audit_event")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION IF EXISTS reject_signin_audit_for_fail_closed_test()")
        .execute(pool)
        .await?;
    sqlx::query(
        r#"
        CREATE OR REPLACE FUNCTION reject_signin_audit_for_fail_closed_test()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF NEW.event_type LIKE 'timeline.signin_%' THEN
                RAISE EXCEPTION 'simulated sign-in audit insert failure';
            END IF;
            RETURN NEW;
        END;
        $$
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TRIGGER reject_signin_audit_for_fail_closed_test
        BEFORE INSERT ON audit_event
        FOR EACH ROW
        EXECUTE FUNCTION reject_signin_audit_for_fail_closed_test()
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn uninstall_rejecting_signin_audit_trigger(pool: &sqlx::PgPool) -> TestResult {
    sqlx::query("DROP TRIGGER reject_signin_audit_for_fail_closed_test ON audit_event")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION reject_signin_audit_for_fail_closed_test()")
        .execute(pool)
        .await?;
    Ok(())
}

async fn install_rejecting_seat_ledger_trigger(pool: &sqlx::PgPool) -> TestResult {
    sqlx::query(
        "DROP TRIGGER IF EXISTS reject_seat_ledger_for_fail_closed_test ON license_seat_ledger",
    )
    .execute(pool)
    .await?;
    sqlx::query("DROP FUNCTION IF EXISTS reject_seat_ledger_for_fail_closed_test()")
        .execute(pool)
        .await?;
    sqlx::query(
        r#"
        CREATE FUNCTION reject_seat_ledger_for_fail_closed_test()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            RAISE EXCEPTION 'simulated seat-ledger insert failure';
        END;
        $$
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TRIGGER reject_seat_ledger_for_fail_closed_test
        BEFORE INSERT ON license_seat_ledger
        FOR EACH ROW
        EXECUTE FUNCTION reject_seat_ledger_for_fail_closed_test()
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn uninstall_rejecting_seat_ledger_trigger(pool: &sqlx::PgPool) -> TestResult {
    sqlx::query("DROP TRIGGER reject_seat_ledger_for_fail_closed_test ON license_seat_ledger")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION reject_seat_ledger_for_fail_closed_test()")
        .execute(pool)
        .await?;
    Ok(())
}

async fn install_rejecting_api_token_audit_trigger(pool: &sqlx::PgPool) -> TestResult {
    sqlx::query(
        "DROP TRIGGER IF EXISTS reject_api_token_audit_for_fail_closed_test ON audit_event",
    )
    .execute(pool)
    .await?;
    sqlx::query("DROP FUNCTION IF EXISTS reject_api_token_audit_for_fail_closed_test()")
        .execute(pool)
        .await?;
    sqlx::query(
        r#"
        CREATE FUNCTION reject_api_token_audit_for_fail_closed_test()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF NEW.event_type LIKE 'access_control.user_api_token.%' THEN
                RAISE EXCEPTION 'simulated API-token audit insert failure';
            END IF;
            RETURN NEW;
        END;
        $$
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TRIGGER reject_api_token_audit_for_fail_closed_test
        BEFORE INSERT ON audit_event
        FOR EACH ROW
        EXECUTE FUNCTION reject_api_token_audit_for_fail_closed_test()
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn uninstall_rejecting_api_token_audit_trigger(pool: &sqlx::PgPool) -> TestResult {
    sqlx::query("DROP TRIGGER reject_api_token_audit_for_fail_closed_test ON audit_event")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION reject_api_token_audit_for_fail_closed_test()")
        .execute(pool)
        .await?;
    Ok(())
}

async fn reset_license_state(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM license_seat_ledger")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM license_seat_assignment")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM license_state")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM license_instance_identity")
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_license_state(
    pool: &sqlx::PgPool,
    entitlement_jwt: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO license_state (
            instance_id,
            status,
            max_seats,
            entitlement_jwt,
            entitlement_version,
            entitlement_expires_at,
            activation_token_encrypted
        )
        VALUES ('audit-seat-test', 'active', 5, $1, 'test-v1', $2, 'test-only')
        "#,
    )
    .bind(entitlement_jwt)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

async fn active_seat_count_for(pool: &sqlx::PgPool, user_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM license_seat_assignment WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

async fn fill_default_free_tier(pool: &sqlx::PgPool) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut user_ids = Vec::new();
    for index in 0..5 {
        let user_id = Uuid::now_v7();
        let username = format!("audit-fail-seat-holder-{index}-{user_id}");
        sqlx::query(
            r#"
            INSERT INTO identity_user (id, username, email, status, auth_source)
            VALUES ($1, $2, $3, 'active', 'local')
            "#,
        )
        .bind(user_id)
        .bind(&username)
        .bind(format!("{username}@test.local"))
        .execute(pool)
        .await?;
        sqlx::query("INSERT INTO license_seat_assignment (user_id) VALUES ($1)")
            .bind(user_id)
            .execute(pool)
            .await?;
        user_ids.push(user_id);
    }
    Ok(user_ids)
}

async fn delete_seat_holders(pool: &sqlx::PgPool, user_ids: &[Uuid]) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM license_seat_assignment WHERE user_id = ANY($1)")
        .bind(user_ids)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM identity_user WHERE id = ANY($1)")
        .bind(user_ids)
        .execute(pool)
        .await?;
    Ok(())
}
