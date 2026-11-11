//! Upstream bodies must never become operator-facing errors or job reports.

#[test]
fn opaque_inventory_json_numbers_round_trip_without_precision_loss() {
    // Arbitrary JSON custom fields are copied, not evaluated as float operands.
    let source = r#"{"evidence":{"duration":0.123456789012345678901234567890,"identifier":123456789012345678901234567890}}"#;
    let value: serde_json::Value = serde_json::from_str(source).expect("valid source JSON");
    assert_eq!(
        serde_json::to_string(&value).expect("serialized inventory JSON"),
        source
    );
}

use axum::{Router, http::StatusCode, routing::get};
use piping_backend::integrations::netbox::NetboxClient;

#[tokio::test]
async fn source_errors_expose_status_without_untrusted_body()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = Router::new()
        .route(
            "/api/status/",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    "arbitrary-confidential-upstream-content",
                )
            }),
        )
        .route(
            "/api/schema/",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    "arbitrary-confidential-upstream-content",
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        axum::serve(listener, server)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let client = NetboxClient::new(
        format!("http://{address}"),
        "synthetic-fixture-token".into(),
    )?;
    for error in [
        client
            .validate_connection()
            .await
            .expect_err("fixture refuses access"),
        client
            .get_schema()
            .await
            .expect_err("fixture refuses schema access"),
    ] {
        let message = error.to_string();
        assert!(message.contains("403 Forbidden"));
        assert!(!message.contains("arbitrary-confidential-upstream-content"));
    }
    let _ = stop.send(());
    handle.await??;
    Ok(())
}
