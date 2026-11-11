//! Integration test target.

use piping_backend::config;
use piping_backend::observability::{LogLevel, build_env_filter, build_event_record_for_contract};

#[test]
fn debug_toggle_expands_filter_without_changing_level_contract() {
    if std::env::var("PIPING_LOG_LEVEL").is_ok() {
        return;
    }

    let mut settings = config::load_settings().expect("load settings");
    settings.observability.log_level = "INFO".to_string();
    let info_filter = build_env_filter(&settings).to_string().to_ascii_lowercase();

    settings.observability.log_level = "DEBUG".to_string();
    let debug_filter = build_env_filter(&settings).to_string().to_ascii_lowercase();

    assert!(info_filter.contains("info"));
    assert!(debug_filter.contains("debug"));

    let info_record =
        build_event_record_for_contract(LogLevel::Info, "rest.route.completed", Default::default());
    let error_record =
        build_event_record_for_contract(LogLevel::Error, "rest.route.failed", Default::default());

    assert_eq!(
        info_record.get("level").and_then(|v| v.as_str()),
        Some("INFO")
    );
    assert_eq!(
        error_record.get("level").and_then(|v| v.as_str()),
        Some("ERROR")
    );
}
