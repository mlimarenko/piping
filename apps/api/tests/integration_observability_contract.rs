//! Integration test target.

use piping_backend::observability::{
    EventFields, LogLevel, build_event_record_for_contract, metrics,
};

#[test]
fn canonical_event_record_contains_required_core_fields() {
    let fields = EventFields::default()
        .with_correlation_id("corr-123")
        .with_operation("rest.route.connection_targets")
        .with_resource("http_route", "/api/v1/connections/targets")
        .with_status("ok")
        .with_duration_ms(12.5)
        .with_context("method", "GET");

    let record = build_event_record_for_contract(LogLevel::Info, "rest.route.completed", fields);

    assert!(record.contains_key("timestamp"));
    assert_eq!(record.get("level").and_then(|v| v.as_str()), Some("INFO"));
    assert_eq!(
        record.get("event").and_then(|v| v.as_str()),
        Some("rest.route.completed")
    );
    assert_eq!(
        record.get("correlation_id").and_then(|v| v.as_str()),
        Some("corr-123")
    );
    assert_eq!(
        record.get("operation").and_then(|v| v.as_str()),
        Some("rest.route.connection_targets")
    );
    assert_eq!(
        record.get("resource_type").and_then(|v| v.as_str()),
        Some("http_route")
    );
    assert_eq!(
        record.get("resource_id").and_then(|v| v.as_str()),
        Some("/api/v1/connections/targets")
    );
    assert_eq!(record.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert!(record.contains_key("duration_ms"));
    assert!(record.contains_key("context"));
}

#[test]
fn error_record_enforces_error_fields_when_missing() {
    let fields = EventFields::default()
        .with_correlation_id("corr-error")
        .with_operation("proxy.connect")
        .with_resource("connection", "session-1");

    let record =
        build_event_record_for_contract(LogLevel::Error, "proxy.tcp_connect.failed", fields);

    assert_eq!(
        record.get("error_type").and_then(|v| v.as_str()),
        Some("unknown_error")
    );
    assert_eq!(
        record.get("error_message").and_then(|v| v.as_str()),
        Some("unknown error")
    );
}

#[test]
fn non_primitive_context_values_are_sanitized() {
    let fields = EventFields::default()
        .with_json_context("selection", &serde_json::json!({"items": ["id", "name"]}));

    let record = build_event_record_for_contract(LogLevel::Info, "rest.route.received", fields);
    let context = record
        .get("context")
        .and_then(|v| v.as_object())
        .expect("context object");

    assert_eq!(
        context
            .get("selection")
            .and_then(|v| v.as_str())
            .expect("selection field"),
        "[non-primitive]"
    );
}

#[test]
fn service_discovery_reports_the_latest_target_count_and_success_time() {
    metrics::record_service_discovery_success("device", 7);
    metrics::record_service_discovery_success("device", 0);

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert!(snapshot.contains("piping_service_discovery_targets{kind=\"device\"} 0"));
    assert!(
        snapshot
            .contains("piping_service_discovery_last_success_timestamp_seconds{kind=\"device\"}")
    );
    assert!(!snapshot.contains("piping_service_discovery_targets_total"));
}

#[test]
fn http_duration_does_not_fragment_histograms_by_status() {
    metrics::observe_http_request("GET", "/metrics-contract", "200", 0.01);
    metrics::observe_http_request("GET", "/metrics-contract", "503", 0.02);

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    let duration_lines = snapshot
        .lines()
        .filter(|line| line.starts_with("piping_http_request_duration_seconds"))
        .collect::<Vec<_>>();
    assert!(
        duration_lines
            .iter()
            .any(|line| { line.contains("_count{method=\"GET\",route=\"/metrics-contract\"} 2") })
    );
    assert!(duration_lines.iter().all(|line| !line.contains("status=")));
}

#[test]
fn unauthorized_webhook_attempts_do_not_publish_provider_ids() {
    metrics::record_netbox_webhook_unauth_attempt();

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    let sample = snapshot
        .lines()
        .find(|line| line.starts_with("netbox_webhook_unauth_attempts_total "))
        .expect("aggregate unauthorized webhook sample");
    assert!(!sample.contains('{'));
    assert!(!snapshot.contains("provider_id="));
}

fn assert_metric_labels(snapshot: &str, family: &str, label: &str, values: &[&str]) {
    for value in values {
        assert!(snapshot.contains(&format!("{family}{{{label}=\"{value}\"}}")));
    }
}

#[test]
fn product_health_metric_labels_are_fixed_vocabulary() {
    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");

    assert_metric_labels(
        &snapshot,
        "piping_db_pool_connections",
        "state",
        &["size", "idle", "acquired", "max"],
    );
    assert_metric_labels(
        &snapshot,
        "piping_playbook_runs",
        "status",
        &[
            "queued",
            "leased",
            "preparing",
            "running",
            "callback_wait",
            "succeeded",
            "failed",
            "canceled",
            "timed_out",
        ],
    );
    let outcomes = &["succeeded", "failed", "canceled", "timed_out"];
    assert_metric_labels(
        &snapshot,
        "piping_playbook_run_outcomes_total",
        "outcome",
        outcomes,
    );
    assert_metric_labels(
        &snapshot,
        "piping_playbook_run_duration_seconds_count",
        "outcome",
        outcomes,
    );
    assert_metric_labels(
        &snapshot,
        "piping_worker_heartbeat_timestamp_seconds",
        "worker",
        &[
            "outbox",
            "webhook_event_pruner",
            "journal_pruner",
            "license_refresh",
            "playbook_run",
            "observability_poller",
            "playbook_schedule",
            "official_catalog_refresh",
        ],
    );
    assert_metric_labels(
        &snapshot,
        "piping_license_seats",
        "state",
        &["used", "limit", "available"],
    );
    let sync_outcomes = &["completed", "failed"];
    assert_metric_labels(
        &snapshot,
        "piping_sync_job_outcomes_total",
        "outcome",
        sync_outcomes,
    );
    assert_metric_labels(
        &snapshot,
        "piping_sync_job_duration_seconds_count",
        "outcome",
        sync_outcomes,
    );
    for metric in [
        "piping_db_pool_acquire_failures_total",
        "piping_playbook_oldest_queued_age_seconds",
        "piping_playbook_dispatch_retries_total",
        "piping_playbook_timeouts_total",
        "piping_license_expiry_timestamp_seconds",
        "piping_sync_jobs_running",
        "piping_sync_last_success_timestamp_seconds",
    ] {
        assert!(snapshot.contains(metric), "missing metric family {metric}");
    }
}

/// The inbound NetBox webhook counter records terminal outcomes only; a
/// "received" increment in addition to the terminal one would double-count
/// every webhook. No database-free runtime check observes which outcome
/// strings the route passes, so this one stays a source guard. The former
/// second assertion (that the unauthorized-attempt metric is never called with
/// a provider id) is covered behaviourally by
/// `unauthorized_webhook_attempts_do_not_publish_provider_ids`, which pins the
/// rendered sample to a label-free `netbox_webhook_unauth_attempts_total`.
#[test]
fn netbox_webhook_terminal_counter_has_no_received_increment() {
    let source = std::fs::read_to_string(format!(
        "{}/src/api/rest/routes/netbox_webhooks.rs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("netbox webhook route source");

    assert!(!source.contains("record_netbox_inbound_webhook(\"received\")"));
}
