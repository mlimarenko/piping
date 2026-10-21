//! Integration test target.

use piping_backend::observability::{EventFields, LogLevel, build_event_record_for_contract};

#[test]
fn error_event_contains_required_operation_context() {
    let record = build_event_record_for_contract(
        LogLevel::Error,
        "websocket.bridge.failed",
        EventFields::default()
            .with_correlation_id("corr-ws-1")
            .with_operation("proxy_to_client_bridge")
            .with_resource("connection", "session-123")
            .with_error("proxy_error", "Connection reset by peer"),
    );

    assert_eq!(record.get("level").and_then(|v| v.as_str()), Some("ERROR"));
    assert_eq!(
        record.get("correlation_id").and_then(|v| v.as_str()),
        Some("corr-ws-1")
    );
    assert_eq!(
        record.get("operation").and_then(|v| v.as_str()),
        Some("proxy_to_client_bridge")
    );
    assert_eq!(
        record.get("resource_type").and_then(|v| v.as_str()),
        Some("connection")
    );
    assert_eq!(
        record.get("resource_id").and_then(|v| v.as_str()),
        Some("session-123")
    );
    assert_eq!(
        record.get("error_type").and_then(|v| v.as_str()),
        Some("proxy_error")
    );
    assert!(record.contains_key("error_message"));
}
