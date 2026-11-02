//! Integration test target.

use piping_backend::observability::{EventFields, LogLevel, build_event_record_for_contract};

#[test]
fn startup_events_use_canonical_names_and_levels() {
    let started = build_event_record_for_contract(
        LogLevel::Info,
        "system.bootstrap.started",
        EventFields::default().with_operation("bootstrap"),
    );
    let completed = build_event_record_for_contract(
        LogLevel::Info,
        "system.bootstrap.completed",
        EventFields::default()
            .with_operation("bootstrap")
            .with_status("ok"),
    );

    assert_eq!(started.get("level").and_then(|v| v.as_str()), Some("INFO"));
    assert_eq!(
        started.get("event").and_then(|v| v.as_str()),
        Some("system.bootstrap.started")
    );

    assert_eq!(
        completed.get("event").and_then(|v| v.as_str()),
        Some("system.bootstrap.completed")
    );
    assert_eq!(completed.get("status").and_then(|v| v.as_str()), Some("ok"));
}
