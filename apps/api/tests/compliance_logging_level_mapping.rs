//! Integration test target.

use piping_backend::observability::{EventFields, LogLevel, build_event_record_for_contract};

#[test]
fn canonical_level_mapping_for_representative_categories() {
    let info_record = build_event_record_for_contract(
        LogLevel::Info,
        "rest.route.completed",
        EventFields::default()
            .with_operation("rest.route.connection_targets")
            .with_resource("http_route", "/api/v1/connections/targets")
            .with_status("ok"),
    );
    assert_eq!(
        info_record.get("level").and_then(|v| v.as_str()),
        Some("INFO")
    );

    let warning_record = build_event_record_for_contract(
        LogLevel::Warning,
        "http.request.completed",
        EventFields::default()
            .with_operation("http.request")
            .with_resource("http_route", "/api/v1/connections/targets")
            .with_status("429"),
    );
    assert_eq!(
        warning_record.get("level").and_then(|v| v.as_str()),
        Some("WARNING")
    );

    let error_record = build_event_record_for_contract(
        LogLevel::Error,
        "rest.route.failed",
        EventFields::default()
            .with_operation("rest.route.secret_fields")
            .with_resource("http_route", "/api/v1/secret-fields")
            .with_error("rest_route_error", "secret lookup failed"),
    );
    assert_eq!(
        error_record.get("level").and_then(|v| v.as_str()),
        Some("ERROR")
    );
    assert_eq!(
        error_record.get("error_type").and_then(|v| v.as_str()),
        Some("rest_route_error")
    );
}
