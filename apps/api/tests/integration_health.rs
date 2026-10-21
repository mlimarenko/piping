//! Integration tests for health/readiness endpoints and stack connectivity.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_health -- --ignored`

mod common;

use common::{build_test_db_pool, build_test_redis};

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_health -- --ignored
async fn database_connectivity() {
    let pool = build_test_db_pool().await.expect("database connection");
    let row: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(&pool)
        .await
        .expect("SELECT 1");
    assert_eq!(row.0, 1);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_health -- --ignored
async fn redis_connectivity() {
    let mut redis = build_test_redis().await.expect("redis connection");
    let pong: String = redis::cmd("PING")
        .query_async(&mut redis)
        .await
        .expect("PING");
    assert_eq!(pong, "PONG");
}
