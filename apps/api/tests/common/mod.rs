//! Shared helpers for integration tests.
//!
//! Integration tests require PostgreSQL and Redis (e.g. `make run-up`).
//! Run with: `cargo test --test integration_* -- --ignored`

use axum::body::Body;
use axum::http::{Request, StatusCode};
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use redis::aio::ConnectionManager;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

/// Test admin credentials. Must match ensure_test_admin bootstrap.
pub(crate) const TEST_ADMIN_USERNAME: &str = "test-admin";
pub(crate) const TEST_ADMIN_PASSWORD: &str = "testpass123456";
const TEST_SECRETS_CIPHER_KEY_B64: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
#[allow(dead_code)] // Used by REST integration suites; not every test target references it.
pub(crate) const TOKEN_SCOPE_CHECK_PATH: &str = "/api/v1/providers";

/// Database URL from environment. Defaults to dev stack URL.
pub(crate) fn database_url() -> String {
    std::env::var("PIPING_DATABASE__URL").unwrap_or_else(|_| {
        "postgresql://piping:piping-dev-password@localhost:55433/piping_selfhost".into()
    })
}

/// Redis URL from environment. Defaults to dev stack URL.
pub(crate) fn redis_url() -> String {
    std::env::var("PIPING_REDIS__URL")
        .unwrap_or_else(|_| "redis://:dev-redis-password@localhost:56380/0".into())
}

/// Build a PostgreSQL connection pool for tests.
pub(crate) async fn build_test_db_pool()
-> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let url = database_url();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        // Generous: this bounds "is a database reachable at all", not latency.
        // Five seconds was tight enough to flake on a loaded CI runner, and a
        // flake here fails a whole DB suite for reasons unrelated to the code.
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await?;
    Ok(pool)
}

/// Build a Redis connection manager for tests.
pub(crate) async fn build_test_redis()
-> Result<ConnectionManager, Box<dyn std::error::Error + Send + Sync>> {
    let url = redis_url();
    let client = redis::Client::open(url)?;
    let manager = ConnectionManager::new(client).await?;
    Ok(manager)
}

/// Ensures test admin user exists with full-access policy.
/// Call after migrations. Idempotent.
pub(crate) async fn ensure_test_admin(
    pool: &sqlx::PgPool,
    password_pepper: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use argon2::password_hash::{SaltString, rand_core::OsRng};
    use argon2::{Argon2, PasswordHasher};

    let admin_user_id = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    let admin_role_id = Uuid::parse_str("00000000-0000-7000-8000-000000000002")?;
    let admin_policy_id = Uuid::parse_str("00000000-0000-7000-8000-000000000003")?;

    // Keep the seed user stable across test binaries. Avoid collisions when previous
    // runs left rows behind (integration tests often share a single dev database).
    sqlx::query(
        r#"
        DELETE FROM identity_user
        WHERE username = $2 AND id <> $1
        "#,
    )
    .bind(admin_user_id)
    .bind(TEST_ADMIN_USERNAME)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO identity_user (id, username, email, status, auth_source)
        VALUES ($1, $2, $3, 'active', 'local')
        ON CONFLICT (id)
        DO UPDATE SET username = EXCLUDED.username,
                      email = EXCLUDED.email,
                      auth_source = 'local',
                      status = 'active'
        "#,
    )
    .bind(admin_user_id)
    .bind(TEST_ADMIN_USERNAME)
    .bind("test-admin@test.local")
    .execute(pool)
    .await?;

    let password_input = format!("{}{}", TEST_ADMIN_PASSWORD, password_pepper);
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password_input.as_bytes(), &salt)
        .map_err(|e| format!("argon2 hash: {}", e))?
        .to_string();

    sqlx::query(
        r#"
        INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (user_id) DO UPDATE SET password_hash = EXCLUDED.password_hash
        "#,
    )
    .bind(admin_user_id)
    .bind(&hash)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO rbac_role (id, name, description)
        VALUES ($1, 'test-admin-role', 'Full access for integration tests')
        ON CONFLICT (name) DO NOTHING
        "#,
    )
    .bind(admin_role_id)
    .execute(pool)
    .await?;

    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(pool)
            .await?;

    // Every Action/ResourceType variant, derived from the enums themselves rather
    // than a hand-maintained list — a hardcoded subset silently rotted out of sync
    // as new resource types (License, Monitoring, ...) landed under RBAC
    // enforcement, since this suite never ran in CI to catch the drift.
    let actions = serde_json::to_value(Action::ALL.to_vec())
        .map_err(|e| format!("Action variants serialize to JSON: {e}"))?;
    let resource_types = serde_json::to_value(ResourceType::ALL.to_vec())
        .map_err(|e| format!("ResourceType variants serialize to JSON: {e}"))?;

    // The seed owns a FIXED id, so it can collide on either key: another row may
    // hold the name, or — as seen on a shared dev database, where id
    // ...0003 was left behind as 'qa-playbooks-policy' — the id may be held
    // under a different name. `ON CONFLICT (name)` alone caught only the first
    // case and let the second fail on the primary key, which is why the three
    // access-control pagination tests failed on any database another suite had
    // touched. Clear the name squatter first, then upsert on the id the seed
    // owns; this mirrors how the admin user above is seeded.
    sqlx::query(
        r#"
        DELETE FROM rbac_policy
        WHERE name = 'test-admin-policy' AND id <> $1
        "#,
    )
    .bind(admin_policy_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints, subjects, resources, enabled
        )
        VALUES ($1, 'test-admin-policy', 'allow', $2, $3, NULL, '[]'::jsonb, '[]'::jsonb, true)
        ON CONFLICT (id)
        DO UPDATE SET name = EXCLUDED.name,
                      effect = EXCLUDED.effect,
                      actions = EXCLUDED.actions,
                      resource_types = EXCLUDED.resource_types,
                      enabled = EXCLUDED.enabled
        "#,
    )
    .bind(admin_policy_id)
    .bind(&actions)
    .bind(&resource_types)
    .execute(pool)
    .await?;

    let policy_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_policy WHERE name = 'test-admin-policy'")
            .fetch_one(pool)
            .await?;

    // Role→policy linkage is now a canonical `role:{uuid}` entry in the policy's
    // `rbac_policy.subjects` array (the legacy role binding table is dropped).
    sqlx::query(
        r#"
        UPDATE rbac_policy
        SET subjects = (
            SELECT COALESCE(jsonb_agg(DISTINCT s), '[]'::jsonb)
            FROM (
                SELECT jsonb_array_elements_text(subjects) AS s
                UNION
                SELECT 'role:' || $1::text
            ) merged
        )
        WHERE id = $2
        "#,
    )
    .bind(role_id)
    .bind(policy_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO rbac_role_assignment (user_id, role_id)
        VALUES ($1, $2)
        ON CONFLICT (user_id, role_id) DO NOTHING
        "#,
    )
    .bind(admin_user_id)
    .bind(role_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Test provider ID for sync/mapping tests. Must match ensure_test_provider.
pub(crate) const TEST_PROVIDER_ID: &str = "00000000-0000-7000-8000-000000000010";

/// Ensures test provider exists for sync/mapping tests.
/// Call after migrations. Idempotent.
pub(crate) async fn ensure_test_provider(
    pool: &sqlx::PgPool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let provider_id = Uuid::parse_str(TEST_PROVIDER_ID)?;
    // Matches the `make netbox-up`/`netbox-seed` local stack (docker-compose.yml
    // --profile netbox, scripts/netbox-seed.py): NETBOX_PORT defaults to 28080,
    // NETBOX_API_TOKEN to the seeded superuser token.
    let base_url = std::env::var("NETBOX_URL").unwrap_or_else(|_| {
        let port = std::env::var("NETBOX_PORT").unwrap_or_else(|_| "28080".to_string());
        format!("http://localhost:{port}")
    });
    let api_token =
        std::env::var("NETBOX_API_TOKEN").unwrap_or_else(|_| "netbox-admin-token".to_string());
    let config = serde_json::json!({
        "base_url": base_url,
        "api_token": api_token,
        "page_size": 200
    });

    sqlx::query(
        r#"
        DELETE FROM sync_provider
        WHERE name = 'test-provider' AND id <> $1
        "#,
    )
    .bind(provider_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO sync_provider (id, name, provider_type, config, is_enabled)
        VALUES ($1, 'test-provider', 'netbox', $2, true)
        ON CONFLICT (id)
        DO UPDATE SET name = EXCLUDED.name,
                      provider_type = EXCLUDED.provider_type,
                      config = EXCLUDED.config,
                      is_enabled = EXCLUDED.is_enabled
        "#,
    )
    .bind(provider_id)
    .bind(&config)
    .execute(pool)
    .await?;

    Ok(provider_id)
}

/// Test secret ID for secrets tests. Must match ensure_test_secret.
pub(crate) const TEST_SECRET_ID: &str = "00000000-0000-7000-8000-000000000020";

/// Ensures test secret variable exists for secrets tests.
/// Call after migrations. Idempotent.
pub(crate) async fn ensure_test_secret(
    pool: &sqlx::PgPool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let secret_id = Uuid::parse_str(TEST_SECRET_ID)?;

    sqlx::query(
        r#"
        INSERT INTO secrets_variable (id, key, is_masked, is_protected)
        VALUES ($1, 'test-secret-key', true, false)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(secret_id)
    .execute(pool)
    .await?;

    Ok(secret_id)
}

/// Build AppState for HTTP integration tests.
///
/// Requires: PostgreSQL, Redis, and env vars. Runs ordered `*_init*.sql` migrations and ensures test admin.
#[allow(dead_code)]
pub(crate) async fn build_test_app_state()
-> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    build_test_app_state_with(|_| {}).await
}

/// Same as [`build_test_app_state`], but lets the caller mutate the loaded
/// settings before validation and startup. Used by suites that must exercise a
/// specific configuration branch (e.g. the local-admin credential override)
/// without duplicating the migration/seed/bootstrap scaffolding.
#[allow(dead_code)]
pub(crate) async fn build_test_app_state_with(
    configure: impl FnOnce(&mut piping_backend::config::Settings),
) -> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = piping_backend::config::load_settings()?;
    if settings.database.url.trim().is_empty() {
        settings.database.url = database_url();
    }
    if settings.redis.url.trim().is_empty() {
        settings.redis.url = redis_url();
    }
    configure_test_cipher(&mut settings);
    settings.security.allow_weak_dev_secrets = true;
    configure(&mut settings);
    let settings = Arc::new(settings);
    settings.validate_for_mode(settings.run_mode)?;
    let db_pool = build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&db_pool).await?;
    reset_test_runtime_state(&db_pool).await?;
    let password_pepper = settings.auth.password_pepper.as_str();
    ensure_test_admin(&db_pool, password_pepper).await?;
    ensure_test_provider(&db_pool).await?;
    ensure_test_secret(&db_pool).await?;
    let redis = build_test_redis().await?;
    let state = piping_backend::app::AppState::build(settings, db_pool, redis).await?;
    Ok(state)
}

/// Configure the synthetic cipher for independently isolated HTTP test databases.
#[allow(
    dead_code,
    reason = "common is compiled separately by each integration binary"
)]
pub(crate) fn configure_test_cipher(settings: &mut piping_backend::config::Settings) {
    let cipher_ref = settings.secrets.cipher.key_material_ref.trim();
    if let Some(env_key) = cipher_ref.strip_prefix("env:") {
        let missing_or_empty =
            std::env::var(env_key.trim()).map_or(true, |value| value.trim().is_empty());
        if missing_or_empty {
            settings.secrets.cipher.key_material_ref =
                format!("inline:{TEST_SECRETS_CIPHER_KEY_B64}");
        }
    }
}

async fn reset_test_runtime_state(
    pool: &sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        TRUNCATE TABLE
            auth_session,
            auth_user_api_token_scope,
            auth_user_api_token,
            license_seat_assignment,
            license_seat_ledger,
            license_state,
            sync_conflict_record,
            sync_error,
            sync_job_step,
            sync_job,
            sync_outbound_event,
            sync_run,
            sync_source_runtime,
            sync_webhook_event,
            sync_provider,
            wizard_progress
        RESTART IDENTITY CASCADE
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// POST to login and return session cookie value, or None if login failed.
#[allow(dead_code)]
pub(crate) async fn login_and_get_cookie(
    app: &axum::Router,
    username: &str,
    password: &str,
) -> Option<String> {
    let body = serde_json::json!({
        "identifier": username,
        "password": password
    })
    .to_string();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .ok()?;

    let response = app.clone().oneshot(req).await.ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let set_cookie = response.headers().get("set-cookie")?;
    let value = set_cookie.to_str().ok()?;
    value.split(';').next().map(str::to_string)
}

/// Generate unique suffix for test data isolation.
#[allow(dead_code)]
pub(crate) fn unique_suffix() -> String {
    let value = Uuid::now_v7().simple().to_string();
    value[value.len().saturating_sub(12)..].to_string()
}

/// Send HTTP request and return (status, body).
#[allow(dead_code)]
pub(crate) async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    cookie: Option<&str>,
) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    let req = if let Some(b) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
    } else {
        builder.body(Body::empty())
    }
    .map_err(|e| format!("build request: {}", e))?;
    let response = app.clone().oneshot(req).await?;
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, body_bytes.to_vec()))
}

/// Send HTTP request and return (status, body) with optional cookie and bearer token.
#[allow(dead_code)]
pub(crate) async fn request_with_auth(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    cookie: Option<&str>,
    bearer_token: Option<&str>,
) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    if let Some(token) = bearer_token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let req = if let Some(b) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
    } else {
        builder.body(Body::empty())
    }
    .map_err(|e| format!("build request: {}", e))?;
    let response = app.clone().oneshot(req).await?;
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, body_bytes.to_vec()))
}

/// Seed the FK chain a `dcim_device` requires (site, role, manufacturer, device
/// type) plus the device row itself; return the device id and its generated name.
///
/// `prefix` namespaces every generated name so two suites seeding concurrently
/// against the same database cannot collide.
#[allow(
    dead_code,
    reason = "shared across integration test binaries; each one compiles common/mod.rs separately and uses a different subset"
)]
pub(crate) async fn seed_device(
    pool: &sqlx::PgPool,
    prefix: &str,
) -> Result<(Uuid, String), Box<dyn std::error::Error + Send + Sync>> {
    let suffix = unique_suffix();
    let site_id = Uuid::now_v7();
    let role_id = Uuid::now_v7();
    let manufacturer_id = Uuid::now_v7();
    let device_type_id = Uuid::now_v7();
    let device_id = Uuid::now_v7();
    let device_name = format!("{prefix}-device-{suffix}");

    sqlx::query(
        r#"
        INSERT INTO dcim_site (
            id, custom_field_data, name, slug, status, facility, description,
            physical_address, shipping_address, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, 'active', '', '', '', '', '')
        "#,
    )
    .bind(site_id)
    .bind(format!("{prefix}-site-{suffix}"))
    .bind(format!("{prefix}-site-{suffix}"))
    .execute(pool)
    .await
    .map_err(|e| format!("seed site: {e}"))?;

    sqlx::query(
        r#"
        INSERT INTO dcim_devicerole (
            id, custom_field_data, name, slug, color, vm_role, description,
            level, lft, rght, tree_id, comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, '9e9e9e', false, '', 0, 0, 0, 0, '')
        "#,
    )
    .bind(role_id)
    .bind(format!("{prefix}-role-{suffix}"))
    .bind(format!("{prefix}-role-{suffix}"))
    .execute(pool)
    .await
    .map_err(|e| format!("seed device role: {e}"))?;

    sqlx::query(
        r#"
        INSERT INTO dcim_manufacturer (id, custom_field_data, name, slug, description, comments)
        VALUES ($1, '{}'::jsonb, $2, $3, '', '')
        "#,
    )
    .bind(manufacturer_id)
    .bind(format!("{prefix}-mfr-{suffix}"))
    .bind(format!("{prefix}-mfr-{suffix}"))
    .execute(pool)
    .await
    .map_err(|e| format!("seed manufacturer: {e}"))?;

    sqlx::query(
        r#"
        INSERT INTO dcim_devicetype (
            id, custom_field_data, model, slug, manufacturer_id, part_number,
            u_height, is_full_depth, front_image, rear_image, comments, description,
            console_port_template_count, console_server_port_template_count,
            power_port_template_count, power_outlet_template_count,
            interface_template_count, front_port_template_count,
            rear_port_template_count, device_bay_template_count,
            module_bay_template_count, inventory_item_template_count,
            device_count, exclude_from_utilization
        )
        VALUES ($1, '{}'::jsonb, $2, $3, $4, '', 1.0, false, '', '', '', '',
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false)
        "#,
    )
    .bind(device_type_id)
    .bind(format!("{prefix}-dt-{suffix}"))
    .bind(format!("{prefix}-dt-{suffix}"))
    .bind(manufacturer_id)
    .execute(pool)
    .await
    .map_err(|e| format!("seed device type: {e}"))?;

    sqlx::query(
        r#"
        INSERT INTO dcim_device (
            id, custom_field_data, name, serial, status, comments, description,
            role_id, device_type_id, site_id,
            console_port_count, console_server_port_count, power_port_count,
            power_outlet_count, interface_count, front_port_count, rear_port_count,
            device_bay_count, module_bay_count, inventory_item_count
        )
        VALUES ($1, '{}'::jsonb, $2, '', 'active', '', '',
                $3, $4, $5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        "#,
    )
    .bind(device_id)
    .bind(&device_name)
    .bind(role_id)
    .bind(device_type_id)
    .bind(site_id)
    .execute(pool)
    .await
    .map_err(|e| format!("seed device: {e}"))?;

    Ok((device_id, device_name))
}
