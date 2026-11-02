use std::sync::Once;

use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use prometheus::{
    Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec,
    Opts, register_gauge_vec_with_registry, register_gauge_with_registry,
    register_histogram_vec_with_registry, register_int_counter_vec_with_registry,
    register_int_counter_with_registry, register_int_gauge_vec_with_registry,
    register_int_gauge_with_registry,
};

const PLAYBOOK_STATUSES: [&str; 9] = [
    "queued",
    "leased",
    "preparing",
    "running",
    "callback_wait",
    "succeeded",
    "failed",
    "canceled",
    "timed_out",
];
const PLAYBOOK_OUTCOMES: [&str; 4] = ["succeeded", "failed", "canceled", "timed_out"];
const WORKERS: [&str; 8] = [
    "outbox",
    "webhook_event_pruner",
    "journal_pruner",
    "license_refresh",
    "playbook_run",
    "observability_poller",
    "playbook_schedule",
    "official_catalog_refresh",
];
static INITIALIZE_LABELS: Once = Once::new();

struct RuntimeMetrics {
    db_pool_connections: IntGaugeVec,
    db_pool_acquire_failures_total: IntCounter,
    playbook_runs: IntGaugeVec,
    playbook_oldest_queued_age_seconds: Gauge,
    playbook_run_outcomes_total: IntCounterVec,
    playbook_dispatch_retries_total: IntCounter,
    playbook_timeouts_total: IntCounter,
    playbook_run_duration_seconds: HistogramVec,
    worker_heartbeat_timestamp_seconds: GaugeVec,
    license_seats: IntGaugeVec,
    license_expiry_timestamp_seconds: Gauge,
    sync_jobs_running: IntGauge,
    sync_job_outcomes_total: IntCounterVec,
    sync_job_duration_seconds: HistogramVec,
    sync_last_success_timestamp_seconds: Gauge,
}

fn build_runtime(registry: &prometheus::Registry) -> Result<RuntimeMetrics, prometheus::Error> {
    Ok(RuntimeMetrics {
        db_pool_connections: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_db_pool_connections",
                "Database pool connections by bounded state",
            ),
            &["state"],
            registry
        )?,
        db_pool_acquire_failures_total: register_int_counter_with_registry!(
            "piping_db_pool_acquire_failures_total",
            "Total failed database pool connection acquisitions",
            registry
        )?,
        playbook_runs: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_playbook_runs",
                "Persisted playbook runs by formal lifecycle status",
            ),
            &["status"],
            registry
        )?,
        playbook_oldest_queued_age_seconds: register_gauge_with_registry!(
            "piping_playbook_oldest_queued_age_seconds",
            "Age in seconds of the oldest queued playbook run, or zero when empty",
            registry
        )?,
        playbook_run_outcomes_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_playbook_run_outcomes_total",
                "Total newly terminal playbook runs by outcome",
            ),
            &["outcome"],
            registry
        )?,
        playbook_dispatch_retries_total: register_int_counter_with_registry!(
            "piping_playbook_dispatch_retries_total",
            "Total playbook runs requeued after sidecar dispatch failures",
            registry
        )?,
        playbook_timeouts_total: register_int_counter_with_registry!(
            "piping_playbook_timeouts_total",
            "Total playbook runs reaped after exceeding their timeout",
            registry
        )?,
        playbook_run_duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_playbook_run_duration_seconds",
                "Playbook run duration from start (or queue time) to terminal outcome",
            )
            .buckets(vec![
                1.0, 5.0, 15.0, 30.0, 60.0, 300.0, 900.0, 3600.0, 14_400.0
            ]),
            &["outcome"],
            registry
        )?,
        worker_heartbeat_timestamp_seconds: register_gauge_vec_with_registry!(
            Opts::new(
                "piping_worker_heartbeat_timestamp_seconds",
                "Unix timestamp of the latest worker loop heartbeat",
            ),
            &["worker"],
            registry
        )?,
        license_seats: register_int_gauge_vec_with_registry!(
            Opts::new("piping_license_seats", "License seats by bounded state"),
            &["state"],
            registry
        )?,
        license_expiry_timestamp_seconds: register_gauge_with_registry!(
            "piping_license_expiry_timestamp_seconds",
            "License entitlement expiry as a Unix timestamp, or zero when unavailable",
            registry
        )?,
        sync_jobs_running: register_int_gauge_with_registry!(
            "piping_sync_jobs_running",
            "Sync jobs currently executing",
            registry
        )?,
        sync_job_outcomes_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_sync_job_outcomes_total",
                "Total sync job execution outcomes",
            ),
            &["outcome"],
            registry
        )?,
        sync_job_duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_sync_job_duration_seconds",
                "Sync job execution duration by outcome",
            )
            .buckets(vec![1.0, 5.0, 15.0, 30.0, 60.0, 300.0, 900.0, 3600.0]),
            &["outcome"],
            registry
        )?,
        sync_last_success_timestamp_seconds: register_gauge_with_registry!(
            "piping_sync_last_success_timestamp_seconds",
            "Unix timestamp of the latest successful sync job",
            registry
        )?,
    })
}

static RUNTIME_METRICS: OnceCell<RuntimeMetrics> = OnceCell::new();

/// Fallible startup registration. A duplicate family name is returned as
/// `Err(AlreadyReg)` so the process can fail fast at boot; callers that touch a
/// metric before startup go through the private runtime_metrics accessor.
pub(super) fn init_metrics() -> Result<(), prometheus::Error> {
    let metrics =
        RUNTIME_METRICS.get_or_try_init(|| build_runtime(prometheus::default_registry()))?;
    prime_labels(metrics);
    Ok(())
}

#[expect(
    clippy::expect_used,
    reason = "duplicate metric name is a programmer error already surfaced by init_metrics() at startup"
)]
fn runtime_metrics() -> &'static RuntimeMetrics {
    let metrics = RUNTIME_METRICS.get_or_init(|| {
        build_runtime(prometheus::default_registry()).expect("register runtime metrics")
    });
    prime_labels(metrics);
    metrics
}

fn prime_labels(metrics: &RuntimeMetrics) {
    INITIALIZE_LABELS.call_once(|| {
        for state in ["size", "idle", "acquired", "max"] {
            metrics
                .db_pool_connections
                .with_label_values(&[state])
                .set(0);
        }
        for status in PLAYBOOK_STATUSES {
            metrics.playbook_runs.with_label_values(&[status]).set(0);
        }
        for outcome in PLAYBOOK_OUTCOMES {
            metrics
                .playbook_run_outcomes_total
                .with_label_values(&[outcome]);
            metrics
                .playbook_run_duration_seconds
                .with_label_values(&[outcome]);
        }
        for worker in WORKERS {
            metrics
                .worker_heartbeat_timestamp_seconds
                .with_label_values(&[worker])
                .set(0.0);
        }
        for state in ["used", "limit", "available"] {
            metrics.license_seats.with_label_values(&[state]).set(0);
        }
        for outcome in ["completed", "failed"] {
            metrics
                .sync_job_outcomes_total
                .with_label_values(&[outcome]);
            metrics
                .sync_job_duration_seconds
                .with_label_values(&[outcome]);
        }
    });
}

pub fn record_db_pool_state(size: u32, idle: usize, max: u32) {
    let metrics = runtime_metrics();
    let size = i64::from(size);
    let idle = i64::try_from(idle).unwrap_or(i64::MAX).min(size);
    for (state, value) in [
        ("size", size),
        ("idle", idle),
        ("acquired", size.saturating_sub(idle)),
        ("max", i64::from(max)),
    ] {
        metrics
            .db_pool_connections
            .with_label_values(&[state])
            .set(value);
    }
}

pub fn record_db_pool_acquire_failure() {
    runtime_metrics().db_pool_acquire_failures_total.inc();
}

pub fn set_playbook_run_health(status_counts: &[(&str, i64)], oldest_queued_age_seconds: f64) {
    let metrics = runtime_metrics();
    for status in PLAYBOOK_STATUSES {
        metrics.playbook_runs.with_label_values(&[status]).set(0);
    }
    for (status, count) in status_counts {
        if PLAYBOOK_STATUSES.contains(status) {
            metrics
                .playbook_runs
                .with_label_values(&[status])
                .set((*count).max(0));
        }
    }
    metrics
        .playbook_oldest_queued_age_seconds
        .set(oldest_queued_age_seconds.max(0.0));
}

pub fn record_playbook_run_outcome(
    outcome: &str,
    queued_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
) {
    let metrics = runtime_metrics();
    if !PLAYBOOK_OUTCOMES.contains(&outcome) {
        return;
    }
    let finished_at = finished_at.unwrap_or_else(Utc::now);
    let started_at = started_at.unwrap_or(queued_at);
    let duration_seconds = finished_at
        .signed_duration_since(started_at)
        .num_milliseconds()
        .max(0) as f64
        / 1000.0;
    metrics
        .playbook_run_outcomes_total
        .with_label_values(&[outcome])
        .inc();
    metrics
        .playbook_run_duration_seconds
        .with_label_values(&[outcome])
        .observe(duration_seconds);
    if outcome == "timed_out" {
        metrics.playbook_timeouts_total.inc();
    }
}

pub fn record_playbook_dispatch_retries(count: usize) {
    runtime_metrics()
        .playbook_dispatch_retries_total
        .inc_by(u64::try_from(count).unwrap_or(u64::MAX));
}

pub fn record_worker_heartbeat(worker: &str) {
    let metrics = runtime_metrics();
    if WORKERS.contains(&worker) {
        metrics
            .worker_heartbeat_timestamp_seconds
            .with_label_values(&[worker])
            .set(Utc::now().timestamp_millis() as f64 / 1000.0);
    }
}

pub fn set_license_seat_state(
    used: i64,
    limit: i32,
    available: i64,
    expiry: Option<DateTime<Utc>>,
) {
    let metrics = runtime_metrics();
    for (state, value) in [
        ("used", used.max(0)),
        ("limit", i64::from(limit).max(0)),
        ("available", available.max(0)),
    ] {
        metrics.license_seats.with_label_values(&[state]).set(value);
    }
    metrics
        .license_expiry_timestamp_seconds
        .set(expiry.map_or(0.0, |value| value.timestamp_millis() as f64 / 1000.0));
}

pub struct SyncJobMetricsGuard {
    started_at: std::time::Instant,
    finished: bool,
}

impl SyncJobMetricsGuard {
    fn finish(&mut self, succeeded: bool) {
        if self.finished {
            return;
        }
        record_sync_job_completion(self.started_at, succeeded);
        self.finished = true;
    }
}

impl Drop for SyncJobMetricsGuard {
    fn drop(&mut self) {
        self.finish(false);
    }
}

pub fn record_sync_job_started() -> SyncJobMetricsGuard {
    runtime_metrics().sync_jobs_running.inc();
    SyncJobMetricsGuard {
        started_at: std::time::Instant::now(),
        finished: false,
    }
}

pub fn record_sync_job_finished(mut guard: SyncJobMetricsGuard, succeeded: bool) {
    guard.finish(succeeded);
}

fn record_sync_job_completion(started_at: std::time::Instant, succeeded: bool) {
    let metrics = runtime_metrics();
    let outcome = if succeeded { "completed" } else { "failed" };
    metrics.sync_jobs_running.dec();
    metrics
        .sync_job_outcomes_total
        .with_label_values(&[outcome])
        .inc();
    metrics
        .sync_job_duration_seconds
        .with_label_values(&[outcome])
        .observe(started_at.elapsed().as_secs_f64());
    if succeeded {
        metrics
            .sync_last_success_timestamp_seconds
            .set(Utc::now().timestamp_millis() as f64 / 1000.0);
    }
}
