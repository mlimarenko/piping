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
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                // Correlated inventory projections trigger seconds of JIT compilation
                // on interactive reads. Apply to every new connection, not the server.
                sqlx::query("SET jit = off").execute(connection).await?;
                Ok(())
            })
        })
        .connect(&config.url)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create database pool: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires PostgreSQL via PIPING_DATABASE__URL"]
    async fn pool_disables_jit_on_each_connection_and_replacement()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let config = DatabaseConfig {
            url: std::env::var("PIPING_DATABASE__URL")?,
            max_connections: 2,
            min_connections: 0,
            acquire_timeout_seconds: 5,
            idle_timeout_seconds: 60,
            max_lifetime_seconds: 60,
        };
        let pool = build_pool(&config).await?;
        let mut first = pool.acquire().await?;
        let mut second = pool.acquire().await?;
        for connection in [&mut first, &mut second] {
            let jit: String = sqlx::query_scalar("SHOW jit")
                .fetch_one(&mut **connection)
                .await?;
            assert_eq!(jit, "off");
        }
        let original_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *first)
            .await?;
        first.close().await?;
        let mut replacement = pool.acquire().await?;
        let (replacement_pid, jit, result): (i32, String, i32) =
            sqlx::query_as("SELECT pg_backend_pid(), current_setting('jit'), 6 * 7")
                .fetch_one(&mut *replacement)
                .await?;
        assert_ne!(replacement_pid, original_pid);
        assert_eq!(jit, "off");
        assert_eq!(result, 42);
        drop(second);
        drop(replacement);
        pool.close().await;
        Ok(())
    }
}
