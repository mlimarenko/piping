//! HTTP fixture with its own PostgreSQL database and a selected Redis database.

use crate::common;
use piping_backend::app::AppState;
use sqlx::{Postgres, migrate::MigrateDatabase};
use std::sync::Arc;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) struct Harness {
    pub(crate) state: AppState,
    pub(crate) app: axum::Router,
    pub(crate) cookie: String,
    url: String,
}

pub(crate) async fn harness(prefix: &str, redis_database: u8) -> TestResult<Harness> {
    harness_with(prefix, redis_database, |_| {}).await
}

/// Same as [`harness`], but the caller may adjust loaded settings before
/// validation and startup (for example the bundled catalog path).
pub(crate) async fn harness_with(
    prefix: &str,
    redis_database: u8,
    configure: impl FnOnce(&mut piping_backend::config::Settings),
) -> TestResult<Harness> {
    let mut url = url::Url::parse(&common::database_url())?;
    url.set_path(&format!("/{prefix}_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let pool = sqlx::PgPool::connect(&url).await?;
    sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;

    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = url.clone();
    settings.redis.url = common::redis_url(Some(redis_database))?;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
    configure(&mut settings);
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let state = AppState::build(Arc::new(settings), pool, redis).await?;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    Ok(Harness {
        state,
        app,
        cookie,
        url,
    })
}

pub(crate) async fn shutdown(harness: Harness) -> TestResult {
    let Harness {
        state, app, url, ..
    } = harness;
    drop(app);
    state.db_pool.close().await;
    drop(state);
    <Postgres as MigrateDatabase>::force_drop_database(&url).await?;
    Ok(())
}
