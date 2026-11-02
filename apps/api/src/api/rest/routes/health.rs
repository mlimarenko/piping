use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use serde_json::json;

use crate::app::AppState;
use crate::observability::metrics;

/// Service health status payload.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    /// Health status indicator (e.g. `"ok"`).
    pub status: String,
}

/// Report service health.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is up", body = HealthResponse),
    ),
)]
pub async fn health_handler(State(_state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

/// Liveness probe handler returning 200 OK with status JSON
pub async fn liveness_handler(State(_): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

/// Readiness probe handler returning 200 OK when dependencies are ready.
pub async fn readiness_handler(State(state): State<AppState>) -> impl IntoResponse {
    let db_ok = match state.db_pool.acquire().await {
        Ok(mut connection) => sqlx::query("SELECT 1")
            .execute(&mut *connection)
            .await
            .is_ok(),
        Err(_) => {
            metrics::record_db_pool_acquire_failure();
            false
        }
    };
    metrics::record_db_pool_state(
        state.db_pool.size(),
        state.db_pool.num_idle(),
        state.db_pool.options().get_max_connections(),
    );

    let mut redis = state.redis.clone();
    let redis_ok = redis::cmd("PING")
        .query_async::<String>(&mut redis)
        .await
        .is_ok();

    if db_ok && redis_ok {
        (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "checks": {
                    "database": "ok",
                    "redis": "ok"
                }
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "degraded",
                "checks": {
                    "database": if db_ok { "ok" } else { "error" },
                    "redis": if redis_ok { "ok" } else { "error" }
                }
            })),
        )
    }
}
