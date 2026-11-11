//! Integration test target.

use std::io::Write;
use std::sync::{Arc, Mutex};

use piping_backend::observability::{
    EventFields, LogLevel, build_event_record_for_contract, log_error_event, metrics,
};
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn the_json_stream_carries_service_release_and_stack_of_a_failure() {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = {
        let buffer = Arc::clone(&buffer);
        move || SharedWriter(Arc::clone(&buffer))
    };
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(writer),
    );

    tracing::subscriber::with_default(subscriber, || {
        log_error_event(
            "system.panic",
            "panic",
            "thread 'worker' panicked",
            EventFields::default().with_error_stack("at src/app/workers.rs:42"),
        );
    });

    let emitted =
        String::from_utf8(buffer.lock().expect("buffer").clone()).expect("utf-8 log line");
    let line = emitted.lines().next().expect("one structured line");
    let event: serde_json::Value = serde_json::from_str(line).expect("structured json line");
    let payload: serde_json::Value = serde_json::from_str(
        event["fields"]["payload"]
            .as_str()
            .expect("payload is the structured record"),
    )
    .expect("payload json");

    assert_eq!(
        payload["service.version"],
        env!("PIPING_PRODUCT_VERSION"),
        "a shipped line must name the release that wrote it"
    );
    assert!(
        payload["service.name"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );
    assert_eq!(payload["level"], "ERROR");
    assert_eq!(payload["error_type"], "panic");
    assert!(
        payload["error_stack"]
            .as_str()
            .is_some_and(|stack| stack.contains("workers.rs:42")),
        "the stack belongs to the failure record, not the terminal only"
    );
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

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
fn every_record_names_the_service_and_the_build_that_emitted_it() {
    let record = build_event_record_for_contract(
        LogLevel::Info,
        "rest.route.completed",
        EventFields::default(),
    );

    assert!(
        record
            .get("service.name")
            .and_then(|v| v.as_str())
            .is_some_and(|name| !name.is_empty()),
        "a record without its service cannot be attributed in VictoriaLogs"
    );
    assert_eq!(
        record.get("service.version").and_then(|v| v.as_str()),
        Some(env!("PIPING_PRODUCT_VERSION")),
        "release identity must match the build, not a guess"
    );
}

#[test]
fn a_failure_record_keeps_its_stack_and_scrubs_it() {
    let record = build_event_record_for_contract(
        LogLevel::Error,
        "system.panic",
        EventFields::default()
            .with_error("panic", "thread 'worker' panicked")
            .with_error_stack("at src/main.rs:12 password=hunter2"),
    );

    let stack = record
        .get("error_stack")
        .and_then(|v| v.as_str())
        .expect("stack field");
    assert!(stack.contains("src/main.rs:12"));
    assert!(
        !stack.contains("hunter2"),
        "a captured stack is still log text and must pass the scrubber"
    );
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
            "connection_reconciliation",
            "outbox",
            "webhook_event_pruner",
            "journal_pruner",
            "license_refresh",
            "playbook_run",
            "playbook_cancel",
            "observability_poller",
            "playbook_schedule",
        ],
    );
    for outcome in ["ok", "error"] {
        assert_metric_labels(
            &snapshot,
            "piping_worker_ticks_total",
            "outcome",
            &[outcome],
        );
    }
    assert_metric_labels(
        &snapshot,
        "piping_outbox_events",
        "state",
        &["pending", "in_progress", "failed", "dead_letter"],
    );
    assert_metric_labels(
        &snapshot,
        "piping_outbox_batches_total",
        "outcome",
        &["ok", "error"],
    );
    assert_metric_labels(
        &snapshot,
        "piping_auth_logins_total",
        "method",
        &["local", "oidc"],
    );
    assert_metric_labels(
        &snapshot,
        "piping_metrics_sample_errors_total",
        "source",
        &["outbox", "connection_sessions"],
    );
    for protocol in ["app", "rdp", "ssh", "vnc", "cctv"] {
        assert!(
            snapshot.contains(&format!(
                "piping_connection_sessions{{protocol=\"{protocol}\",state=\"active\"}}"
            )),
            "missing session gauge for protocol {protocol}"
        );
    }
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

fn sample_value(snapshot: &str, sample: &str) -> f64 {
    for line in snapshot.lines() {
        let Some((name, value)) = line.split_once(' ') else {
            continue;
        };
        if name == sample {
            return value
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("sample {sample} is not numeric: {value}"));
        }
    }
    panic!("missing sample {sample}");
}

#[test]
fn outbox_queue_snapshot_publishes_every_state_and_the_oldest_age() {
    metrics::record_outbox_queue_snapshot(3, 1, 2, 4, 12.5);

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert_eq!(
        sample_value(&snapshot, "piping_outbox_events{state=\"pending\"}"),
        3.0
    );
    assert_eq!(
        sample_value(&snapshot, "piping_outbox_events{state=\"in_progress\"}"),
        1.0
    );
    assert_eq!(
        sample_value(&snapshot, "piping_outbox_events{state=\"failed\"}"),
        2.0
    );
    assert_eq!(
        sample_value(&snapshot, "piping_outbox_events{state=\"dead_letter\"}"),
        4.0
    );
    assert_eq!(
        sample_value(&snapshot, "piping_outbox_oldest_pending_age_seconds"),
        12.5
    );
}

#[test]
fn outbox_batches_count_processed_events_and_failed_batches_separately() {
    metrics::record_outbox_batch(true);
    metrics::record_outbox_batch(false);

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert!(sample_value(&snapshot, "piping_outbox_batches_total{outcome=\"ok\"}") >= 1.0);
    assert!(sample_value(&snapshot, "piping_outbox_batches_total{outcome=\"error\"}") >= 1.0);
}

#[test]
fn external_call_guard_records_one_outcome_and_one_duration_per_call() {
    {
        let mut call =
            metrics::start_external_call(metrics::ExternalTarget::Netbox, "contract_test_ok");
        call.succeeded();
    }
    {
        let _call =
            metrics::start_external_call(metrics::ExternalTarget::Netbox, "contract_test_error");
    }

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert_eq!(
        sample_value(
            &snapshot,
            "piping_external_calls_total{operation=\"contract_test_ok\",outcome=\"success\",target=\"netbox\"}"
        ),
        1.0
    );
    assert_eq!(
        sample_value(
            &snapshot,
            "piping_external_calls_total{operation=\"contract_test_error\",outcome=\"error\",target=\"netbox\"}"
        ),
        1.0
    );
    assert_eq!(
        sample_value(
            &snapshot,
            "piping_external_calls_duration_seconds_count{operation=\"contract_test_ok\",target=\"netbox\"}"
        ),
        1.0
    );
}

#[test]
fn every_worker_heartbeat_is_published_under_its_canonical_label() {
    for worker in metrics::Worker::ALL {
        metrics::record_worker_heartbeat(worker);
    }

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    for worker in metrics::Worker::ALL {
        let sample = format!(
            "piping_worker_heartbeat_timestamp_seconds{{worker=\"{}\"}}",
            worker.as_str()
        );
        assert!(
            sample_value(&snapshot, &sample) > 1_000_000_000.0,
            "missing or zero heartbeat for worker {}",
            worker.as_str()
        );
    }
}

#[test]
fn sample_failures_are_counted_only_for_known_sources() {
    metrics::record_metrics_sample_error("outbox");
    metrics::record_metrics_sample_error("not_a_source");

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert!(
        sample_value(
            &snapshot,
            "piping_metrics_sample_errors_total{source=\"outbox\"}"
        ) >= 1.0
    );
    assert!(!snapshot.contains("source=\"not_a_source\""));
}

#[test]
fn unknown_business_labels_do_not_create_series() {
    metrics::record_connection_session_event("telnet", "started");
    metrics::record_auth_login("saml", "success");
    metrics::set_connection_session_state("telnet", 5, 5);

    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert!(!snapshot.contains("protocol=\"telnet\""));
    assert!(!snapshot.contains("method=\"saml\""));
}

#[test]
fn connection_session_events_keep_the_bounded_protocol_and_event_vocabulary() {
    metrics::record_connection_session_event("ssh", "started");

    let sample = "piping_connection_session_events_total{event=\"ended\",protocol=\"rdp\"}";
    let before = sample_value(
        &metrics::render_prometheus_text().expect("metrics snapshot"),
        sample,
    );
    metrics::record_connection_session_event("rdp", "ended");
    let after = sample_value(
        &metrics::render_prometheus_text().expect("metrics snapshot"),
        sample,
    );

    assert_eq!(
        after,
        before + 1.0,
        "a recorded session end must increment the primed series"
    );
    let snapshot = metrics::render_prometheus_text().expect("metrics snapshot");
    assert!(
        sample_value(
            &snapshot,
            "piping_connection_session_events_total{event=\"started\",protocol=\"ssh\"}"
        ) >= 1.0
    );
}
