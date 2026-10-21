use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

use crate::config::DatabaseConfig;
use crate::errors::AppError;

/// Builds a PostgreSQL connection pool with configured limits and timeouts.
///
/// The pool manages a set of database connections that can be reused across requests.
/// Configuration includes:
/// - `max_connections`: Maximum number of concurrent connections
/// - `min_connections`: Minimum number of connections to maintain
/// - `acquire_timeout`: Maximum time to wait for a connection from the pool
/// - `idle_timeout`: Time before idle connections are closed
/// - `max_lifetime`: Maximum lifetime of a connection before it's recycled
///
/// Connections are automatically managed: created on demand, reused when available,
/// and closed when idle or expired.
pub async fn build_pool(config: &DatabaseConfig) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_seconds))
        .idle_timeout(Some(Duration::from_secs(config.idle_timeout_seconds)))
        .max_lifetime(Some(Duration::from_secs(config.max_lifetime_seconds)))
        .connect(&config.url)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create database pool: {}", e)))
}
