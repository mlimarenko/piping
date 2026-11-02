//! Smoke tests for the standalone proxy router.
//!
//! These tests must not require external services (no SSH/RDP/VNC targets).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use piping_backend::config::ProxyConfig;
use piping_backend::proxy::{ProxyState, build_proxy_router};
use tower::ServiceExt;

#[tokio::test]
async fn proxy_health_returns_ok() {
    let config = ProxyConfig::default();
    let state = Arc::new(ProxyState::new(&config).expect("proxy state"));

    // In the main app the proxy router is typically mounted under `/proxy`.
    let app = Router::new().nest("/proxy", build_proxy_router(state));

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/proxy/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.get("status").and_then(|value| value.as_str()),
        Some("ok")
    );
}
