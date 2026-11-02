//! Process-wide Prometheus metric registry: family registration, HTTP and domain families, and the runtime recorders re-exported from `runtime`.
//
// Metric families are registered once into the process-wide Prometheus default
// registry as a fallible startup step (`init_metrics`). A duplicate family name
// yields `Err(prometheus::Error::AlreadyReg)` instead of a panic; readers
// resolve the once-initialised registry through `http_metrics()`.

use once_cell::sync::OnceCell;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGaugeVec, Opts,
    TextEncoder, gather, register_histogram_vec_with_registry,
    register_int_counter_vec_with_registry, register_int_counter_with_registry,
    register_int_gauge_vec_with_registry,
};

use crate::errors::AppError;

mod runtime;

pub use runtime::{
    SyncJobMetricsGuard, record_db_pool_acquire_failure, record_db_pool_state,
    record_playbook_dispatch_retries, record_playbook_run_outcome, record_sync_job_finished,
    record_sync_job_started, record_worker_heartbeat, set_license_seat_state,
    set_playbook_run_health,
};

#[derive(Debug)]
struct HttpMetrics {
    http_requests_total: IntCounterVec,
    http_request_duration_seconds: HistogramVec,
    http_requests_in_flight: IntGaugeVec,
    service_discovery_requests_total: IntCounterVec,
    service_discovery_targets: IntGaugeVec,
    service_discovery_last_success_timestamp_seconds: IntGaugeVec,
    cache_operations_total: IntCounterVec,
    cache_operation_duration_seconds: HistogramVec,
    build_info: IntGaugeVec,
    license_anomalies_total: IntCounterVec,
    license_activation_total: IntCounterVec,
    api_token_auth_total: IntCounterVec,
    // Sync / NetBox bidirectional outbound + inbound metrics:
    // counter and histogram families used by the inbound NetBox webhook handler
    // and the journal pruner.
    netbox_webhook_echoes_dropped_total: IntCounterVec,
    netbox_journal_probe_duration_seconds: HistogramVec,
    netbox_inbound_webhook_total: IntCounterVec,
    netbox_inbound_webhook_signature_failures_total: IntCounterVec,
    netbox_journal_pruned_rows_total: IntCounterVec,
    netbox_webhook_unauth_attempts_total: IntCounter,
}

fn build_http(registry: &prometheus::Registry) -> Result<HttpMetrics, prometheus::Error> {
    Ok(HttpMetrics {
        http_requests_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_http_requests_total",
                "Total HTTP requests by method, route and status",
            ),
            &["method", "route", "status"],
            registry
        )?,
        http_request_duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_http_request_duration_seconds",
                "HTTP request duration by method and route",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["method", "route"],
            registry
        )?,
        http_requests_in_flight: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_http_requests_in_flight",
                "Number of in-flight HTTP requests by method and route",
            ),
            &["method", "route"],
            registry
        )?,
        service_discovery_requests_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_service_discovery_requests_total",
                "Total service discovery requests",
            ),
            &["result"],
            registry
        )?,
        service_discovery_targets: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_service_discovery_targets",
                "Targets returned by the latest successful service discovery request",
            ),
            &["kind"],
            registry
        )?,
        service_discovery_last_success_timestamp_seconds: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_service_discovery_last_success_timestamp_seconds",
                "Unix timestamp of the latest successful service discovery request",
            ),
            &["kind"],
            registry
        )?,
        cache_operations_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_cache_operations_total",
                "Total cache operations by cache name, operation and result",
            ),
            &["cache", "operation", "result"],
            registry
        )?,
        cache_operation_duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_cache_operation_duration_seconds",
                "Cache operation duration by cache name and operation",
            )
            .buckets(vec![
                0.0001, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
            ]),
            &["cache", "operation"],
            registry
        )?,
        build_info: register_int_gauge_vec_with_registry!(
            Opts::new("piping_build_info", "Build information"),
            &["version"],
            registry
        )?,
        license_anomalies_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_license_anomalies_total",
                "Total suspicious licensing events by kind and source",
            ),
            &["kind", "source"],
            registry
        )?,
        license_activation_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_license_activation_total",
                "Total license activation outcomes by result and reason",
            ),
            &["result", "reason"],
            registry
        )?,
        api_token_auth_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_api_token_auth_total",
                "Total API token auth outcomes by result and reason",
            ),
            &["result", "reason"],
            registry
        )?,
        netbox_webhook_echoes_dropped_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "netbox_webhook_echoes_dropped_total",
                "Inbound NetBox webhooks dropped as echoes by dedup reason",
            ),
            &["reason"],
            registry
        )?,
        netbox_journal_probe_duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "netbox_journal_probe_duration_seconds",
                "Inbound webhook journal echo-dedup probe duration in seconds",
            )
            .buckets(vec![
                0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
            ]),
            &["probe"],
            registry
        )?,
        netbox_inbound_webhook_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "netbox_inbound_webhook_total",
                "Inbound NetBox webhooks received by terminal outcome",
            ),
            &["outcome"],
            registry
        )?,
        netbox_inbound_webhook_signature_failures_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "netbox_inbound_webhook_signature_failures_total",
                "Inbound NetBox webhooks rejected for invalid/missing HMAC signatures",
            ),
            &["reason"],
            registry
        )?,
        netbox_journal_pruned_rows_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "netbox_journal_pruned_rows_total",
                "Provider request journal rows deleted by the TTL pruner",
            ),
            &["state"],
            registry
        )?,
        netbox_webhook_unauth_attempts_total: register_int_counter_with_registry!(
            "netbox_webhook_unauth_attempts_total",
            "Inbound NetBox webhook invalid-signature attempts; used for rate-limit status-flip protection (not request rate limiting)",
            registry
        )?,
    })
}

static HTTP_METRICS: OnceCell<HttpMetrics> = OnceCell::new();

/// Fallible startup registration. A duplicate family name is returned as
/// `Err(AlreadyReg)` so the process can fail fast at boot; callers that touch a
/// metric before startup go through the private http_metrics accessor.
pub fn init_metrics() -> Result<(), AppError> {
    let metrics = HTTP_METRICS
        .get_or_try_init(|| build_http(prometheus::default_registry()))
        .map_err(|error| {
            AppError::Internal(format!("Failed to register Prometheus metrics: {error}"))
        })?;
    metrics
        .build_info
        .with_label_values(&[crate::PRODUCT_VERSION])
        .set(1);
    runtime::init_metrics().map_err(|error| {
        AppError::Internal(format!("Failed to register Prometheus metrics: {error}"))
    })?;
    Ok(())
}

#[expect(
    clippy::expect_used,
    reason = "duplicate metric name is a programmer error already surfaced by init_metrics() at startup"
)]
fn http_metrics() -> &'static HttpMetrics {
    HTTP_METRICS
        .get_or_init(|| build_http(prometheus::default_registry()).expect("register HTTP metrics"))
}

pub fn inc_in_flight(method: &str, route: &str) {
    http_metrics()
        .http_requests_in_flight
        .with_label_values(&[method, route])
        .inc();
}

pub fn dec_in_flight(method: &str, route: &str) {
    http_metrics()
        .http_requests_in_flight
        .with_label_values(&[method, route])
        .dec();
}

pub fn observe_http_request(method: &str, route: &str, status: &str, duration_seconds: f64) {
    let metrics = http_metrics();
    metrics
        .http_requests_total
        .with_label_values(&[method, route, status])
        .inc();
    metrics
        .http_request_duration_seconds
        .with_label_values(&[method, route])
        .observe(duration_seconds);
}

pub fn record_service_discovery_success(kind: &str, targets_count: usize) {
    let metrics = http_metrics();
    metrics
        .service_discovery_requests_total
        .with_label_values(&["success"])
        .inc();
    if !matches!(kind, "all" | "device" | "virtual_machine") {
        return;
    }
    let targets_count = i64::try_from(targets_count).unwrap_or(i64::MAX);
    metrics
        .service_discovery_targets
        .with_label_values(&[kind])
        .set(targets_count);
    metrics
        .service_discovery_last_success_timestamp_seconds
        .with_label_values(&[kind])
        .set(chrono::Utc::now().timestamp());
}

pub fn record_service_discovery_error() {
    http_metrics()
        .service_discovery_requests_total
        .with_label_values(&["error"])
        .inc();
}

pub fn record_cache_operation(
    cache_name: &str,
    operation: &str,
    result: &str,
    duration_seconds: f64,
) {
    let metrics = http_metrics();
    metrics
        .cache_operations_total
        .with_label_values(&[cache_name, operation, result])
        .inc();
    metrics
        .cache_operation_duration_seconds
        .with_label_values(&[cache_name, operation])
        .observe(duration_seconds);
}

pub fn record_license_anomaly(kind: &str, source: &str) {
    http_metrics()
        .license_anomalies_total
        .with_label_values(&[kind, source])
        .inc();
}

pub fn record_license_activation_success() {
    http_metrics()
        .license_activation_total
        .with_label_values(&["success", "activated"])
        .inc();
}

pub fn record_license_activation_failure(reason: &str) {
    http_metrics()
        .license_activation_total
        .with_label_values(&["failed", reason])
        .inc();
}

pub fn record_api_token_auth_success() {
    http_metrics()
        .api_token_auth_total
        .with_label_values(&["success", "authorized"])
        .inc();
}

pub fn record_api_token_auth_denied(reason: &str) {
    http_metrics()
        .api_token_auth_total
        .with_label_values(&["denied", reason])
        .inc();
}

/// Increment the inbound NetBox webhook echo-dropped counter for one of the
/// canonical reasons: `"journal"` | `"stamp"` | `"window"`.
pub fn record_netbox_webhook_echo_dropped(reason: &str) {
    http_metrics()
        .netbox_webhook_echoes_dropped_total
        .with_label_values(&[reason])
        .inc();
}

/// Observe the duration of a journal echo-dedup probe (`probe` ∈
/// `{"primary", "stamp", "hash_window"}`).
pub fn observe_netbox_journal_probe_duration(probe: &str, duration_seconds: f64) {
    http_metrics()
        .netbox_journal_probe_duration_seconds
        .with_label_values(&[probe])
        .observe(duration_seconds);
}

/// Increment the inbound NetBox webhook total counter; `outcome` is one of
/// `"accepted"` | `"echo_dropped"` | `"rejected_signature"` |
/// `"rejected_other"`.
pub fn record_netbox_inbound_webhook(outcome: &str) {
    http_metrics()
        .netbox_inbound_webhook_total
        .with_label_values(&[outcome])
        .inc();
}

/// Increment the inbound NetBox webhook signature-failure counter
/// (`reason` ∈ `{"missing", "mismatch"}`).
pub fn record_netbox_inbound_webhook_signature_failure(reason: &str) {
    http_metrics()
        .netbox_inbound_webhook_signature_failures_total
        .with_label_values(&[reason])
        .inc();
}

/// Increment the journal-pruned-rows counter; `state` matches the journal
/// state of the deleted row (always one of the three terminal states).
pub fn record_netbox_journal_pruned_rows(state: &str, count: u64) {
    http_metrics()
        .netbox_journal_pruned_rows_total
        .with_label_values(&[state])
        .inc_by(count);
}

/// Increment the aggregate inbound-webhook unauthorized-attempt counter.
/// This counter backs the rate-limit status-flip protection in
/// `WebhookUnauthLimiter`; it is NOT a reverse-proxy request-rate counter.
pub fn record_netbox_webhook_unauth_attempt() {
    http_metrics().netbox_webhook_unauth_attempts_total.inc();
}

pub fn render_prometheus_text() -> Result<String, AppError> {
    // Ensure static metric families (including build info) are initialized
    // even when observability bootstrap is not invoked in test/service modes.
    init_metrics()?;

    let encoder = TextEncoder::new();
    let metric_families = gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .map_err(|error| {
            AppError::Internal(format!("Failed to encode Prometheus metrics: {error}"))
        })?;
    String::from_utf8(buffer).map_err(|error| {
        AppError::Internal(format!(
            "Failed to build Prometheus metrics response: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_http, record_license_activation_failure, record_license_activation_success,
        render_prometheus_text,
    };

    #[test]
    fn metrics_snapshot_includes_license_activation_counters() {
        record_license_activation_success();
        record_license_activation_failure("TOKEN_REVOKED");

        let snapshot = render_prometheus_text().expect("metrics snapshot");
        assert!(
            snapshot.contains(
                "piping_license_activation_total{reason=\"activated\",result=\"success\"}"
            ),
            "success activation counter must be present in metrics output",
        );
        assert!(
            snapshot.contains(
                "piping_license_activation_total{reason=\"TOKEN_REVOKED\",result=\"failed\"}"
            ),
            "failed activation counter must be present in metrics output",
        );
    }

    // `build_http` is the seam under test: `init_metrics()` returns this same
    // `AlreadyReg` error at startup, but the process-global default registry
    // cannot be poisoned safely in-process.
    #[test]
    fn duplicate_registration_is_an_error_not_a_panic() {
        let registry = prometheus::Registry::new();
        build_http(&registry).expect("first build registers every HTTP family");
        let error = build_http(&registry).expect_err("a duplicate family name must be rejected");
        assert!(
            matches!(error, prometheus::Error::AlreadyReg),
            "expected AlreadyReg, got {error:?}"
        );
    }
}
