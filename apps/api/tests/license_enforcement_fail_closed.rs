//! Licensing enforcement state must fail closed when it cannot be decoded or persisted.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use piping_backend::config::Settings;
use piping_backend::contexts::licensing::application::service::LicensingService;
use piping_backend::contexts::licensing::infrastructure::repositories::{
    LicensingRepository, SqlxLicensingRepository,
};
use piping_backend::errors::AppError;
use sqlx::postgres::PgPoolOptions;

#[path = "common/mod.rs"]
mod common;

async fn single_connection_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>>
{
    Ok(PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&common::database_url())
        .await?)
}

async fn create_activation_context_table(
    pool: &sqlx::PgPool,
    consecutive_failures_type: &str,
    grace_started_at_type: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let statement = format!(
        r#"
        CREATE TEMP TABLE license_state (
            revision bigint NOT NULL DEFAULT 0,
            instance_id text NOT NULL,
            activation_token_encrypted text NOT NULL,
            entitlement_version text NOT NULL,
            status text NOT NULL,
            consecutive_failures {consecutive_failures_type} NOT NULL,
            grace_started_at {grace_started_at_type},
            grace_ends_at timestamptz,
            last_sync_at timestamptz,
            last_sync_error text,
            last_failure_kind text,
            updated_at timestamptz NOT NULL
        )
        "#,
    );
    sqlx::query(&statement).execute(pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test license_enforcement_fail_closed -- --ignored
async fn corrupted_consecutive_failures_refuses_activation_context()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = single_connection_pool().await?;
    create_activation_context_table(&pool, "text", "timestamptz").await?;
    sqlx::query(
        r#"
        INSERT INTO license_state (
            instance_id,
            activation_token_encrypted,
            entitlement_version,
            status,
            consecutive_failures,
            updated_at
        )
        VALUES ('corrupt-counter-instance', 'test-activation-token', 'v1', 'active', 'corrupt', NOW())
        "#,
    )
    .execute(&pool)
    .await?;

    let repository = SqlxLicensingRepository::new(pool);
    let error = repository
        .begin_refresh()
        .await
        .expect_err("an undecodable failure counter must not yield enforcement state");

    assert!(
        error.to_string().contains("`consecutive_failures`"),
        "the refused state must identify the corrupt enforcement column: {error}"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test license_enforcement_fail_closed -- --ignored
async fn corrupted_grace_timestamp_refuses_activation_context()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = single_connection_pool().await?;
    create_activation_context_table(&pool, "integer", "text").await?;
    sqlx::query(
        r#"
        INSERT INTO license_state (
            instance_id,
            activation_token_encrypted,
            entitlement_version,
            status,
            consecutive_failures,
            grace_started_at,
            updated_at
        )
        VALUES ('corrupt-grace-instance', 'test-activation-token', 'v1', 'grace', 3, 'corrupt', NOW())
        "#,
    )
    .execute(&pool)
    .await?;

    let repository = SqlxLicensingRepository::new(pool);
    let error = repository
        .begin_refresh()
        .await
        .expect_err("an undecodable grace timestamp must not yield enforcement state");

    assert!(
        error.to_string().contains("`grace_started_at`"),
        "the refused state must identify the corrupt enforcement column: {error}"
    );
    Ok(())
}

fn test_settings() -> Result<Settings, Box<dyn std::error::Error + Send + Sync>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/default.toml");
    let mut settings: Settings = config::Config::builder()
        .add_source(config::File::from(path))
        .build()?
        .try_deserialize()?;
    settings.license.base_url = "http://127.0.0.1:9".to_string();
    settings.license.request_timeout_ms = 100;
    Ok(settings)
}

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test license_enforcement_fail_closed -- --ignored
async fn refresh_refuses_when_failure_state_cannot_be_persisted()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = single_connection_pool().await?;
    create_activation_context_table(&pool, "integer", "timestamptz").await?;
    sqlx::query(
        r#"
        INSERT INTO license_state (
            instance_id,
            activation_token_encrypted,
            entitlement_version,
            status,
            consecutive_failures,
            updated_at
        )
        VALUES ('persistence-failure-instance', 'test-activation-token', 'v1', 'active', 0, NOW())
        "#,
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        CREATE FUNCTION pg_temp.reject_license_refresh_failure()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            RAISE EXCEPTION 'simulated license persistence failure';
        END;
        $$
        "#,
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TRIGGER reject_license_refresh_failure
        BEFORE UPDATE ON license_state
        FOR EACH ROW
        WHEN (OLD.revision = NEW.revision)
        EXECUTE FUNCTION pg_temp.reject_license_refresh_failure()
        "#,
    )
    .execute(&pool)
    .await?;

    let service = LicensingService::new(Arc::new(test_settings()?), pool);
    let error = service
        .refresh_license_state()
        .await
        .expect_err("refresh must refuse when failure-state persistence fails");
    let message = error.to_string();

    assert!(
        matches!(error, AppError::Database(_)),
        "the persistence error must be propagated, got: {message}"
    );
    assert!(
        message.contains("simulated license persistence failure"),
        "the persistence failure must remain visible: {message}"
    );
    assert!(
        message.contains("License server request failed"),
        "the original refresh failure must be retained as context: {message}"
    );
    Ok(())
}
