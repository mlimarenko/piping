//! Integration test target.

use piping_backend::observability::{EventFields, LogLevel, build_event_record_for_contract};

#[test]
fn request_entry_and_completion_keep_correlation_fields() {
    let received = build_event_record_for_contract(
        LogLevel::Info,
        "http.request.received",
        EventFields::default()
            .with_correlation_id("corr-req-1")
            .with_operation("http.request")
            .with_resource("http_route", "/api/v1/connections/targets")
            .with_context("method", "GET"),
    );

    let completed = build_event_record_for_contract(
        LogLevel::Info,
        "http.request.completed",
        EventFields::default()
            .with_correlation_id("corr-req-1")
            .with_operation("http.request")
            .with_resource("http_route", "/api/v1/connections/targets")
            .with_status("200")
            .with_duration_ms(31.4)
            .with_context("method", "GET"),
    );

    assert_eq!(
        received.get("correlation_id").and_then(|v| v.as_str()),
        Some("corr-req-1")
    );
    assert_eq!(
        completed.get("correlation_id").and_then(|v| v.as_str()),
        Some("corr-req-1")
    );
    assert!(completed.contains_key("duration_ms"));
    assert_eq!(
        completed.get("status").and_then(|v| v.as_str()),
        Some("200")
    );
}
