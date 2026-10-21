//! MCP audit failure integration tests.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_mcp_audit -- --ignored`

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Router, routing::post};
use piping_backend::api::mcp::mcp_handler;
use piping_backend::api::middleware::auth::AuthContext;
use piping_backend::api::middleware::request_id::RequestId;
use serde_json::{Value, json};
use tower::ServiceExt;

use common::build_test_app_state;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_mcp_audit -- --ignored
async fn mcp_tool_call_fails_when_mandatory_audit_insert_fails() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).expect("backend manifest directory");
    let state = build_test_app_state().await.expect("app state");
    state.db_pool.close().await;

    let app = Router::new()
        .route("/api/v1/mcp", post(mcp_handler))
        .with_state(state);
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "piping.mcp.capabilities",
                    "arguments": {}
                }
            })
            .to_string(),
        ))
        .expect("MCP request");
    request
        .extensions_mut()
        .insert(RequestId("mcp-audit-insert-failure-test".to_string()));
    request.extensions_mut().insert(AuthContext {
        user_id: None,
        auth_method: "api_token".to_string(),
        oidc_groups: Vec::new(),
        api_token_id: None,
        api_token_scopes: Vec::new(),
        api_token_scope_resource_override: None,
    });

    let response = app.oneshot(request).await.expect("MCP response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("MCP response body");
    let payload: Value = serde_json::from_slice(&bytes).expect("MCP JSON response");

    assert_eq!(payload.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        payload.pointer("/result/structuredContent/code"),
        Some(&json!("AUDIT_RECORD_FAILED"))
    );
}
