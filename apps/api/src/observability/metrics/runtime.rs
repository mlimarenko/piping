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
const CONNECTION_PROTOCOLS: [&str; 5] = ["app", "rdp", "ssh", "vnc", "cctv"];
/// Protocols that carry session gauges. A sampler that publishes a complete
/// snapshot resets these first so a protocol that dropped to zero sessions
/// does not keep its previous value.
pub const CONNECTION_PROTOCOL_LABELS: [&str; 5] = CONNECTION_PROTOCOLS;
const CONNECTION_SESSION_STATES: [&str; 2] = ["pending", "active"];
const CONNECTION_SESSION_EVENTS: [&str; 2] = ["started", "ended"];
const AUTH_METHODS: [&str; 2] = ["local", "oidc"];
const AUTH_OUTCOMES: [&str; 2] = ["success", "failure"];
const OUTBOX_STATES: [&str; 4] = ["pending", "in_progress", "failed", "dead_letter"];
const OUTBOX_OUTCOMES: [&str; 2] = ["ok", "error"];
const TICK_OUTCOMES: [&str; 2] = ["ok", "error"];
static INITIALIZE_LABELS: Once = Once::new();

/// Background worker loop that reports a heartbeat and per-tick outcome.
///
/// The enum replaces a string allow-list: a worker without an entry here no
/// longer compiles instead of silently dropping its heartbeat or tick sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worker {
    ConnectionReconciliation,
    Outbox,
    WebhookEventPruner,
    JournalPruner,
    LicenseRefresh,
    PlaybookRun,
    PlaybookCancel,
    ObservabilityPoller,
    PlaybookSchedule,
    PullSchedule,
}

impl Worker {
    pub const ALL: [Worker; 10] = [
        Worker::ConnectionReconciliation,
        Worker::Outbox,
        Worker::WebhookEventPruner,
        Worker::JournalPruner,
        Worker::LicenseRefresh,
        Worker::PlaybookRun,
        Worker::PlaybookCancel,
        Worker::ObservabilityPoller,
        Worker::PlaybookSchedule,
        Worker::PullSchedule,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Worker::ConnectionReconciliation => "connection_reconciliation",
            Worker::Outbox => "outbox",
            Worker::WebhookEventPruner => "webhook_event_pruner",
            Worker::JournalPruner => "journal_pruner",
            Worker::LicenseRefresh => "license_refresh",
            Worker::PlaybookRun => "playbook_run",
            Worker::PlaybookCancel => "playbook_cancel",
            Worker::ObservabilityPoller => "observability_poller",
            Worker::PlaybookSchedule => "playbook_schedule",
            Worker::PullSchedule => "pull_schedule",
        }
    }
}

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
    worker_ticks_total: IntCounterVec,
    worker_tick_duration_seconds: HistogramVec,
    outbox_events: IntGaugeVec,
    outbox_oldest_pending_age_seconds: Gauge,
    outbox_batches_total: IntCounterVec,
    connection_sessions: IntGaugeVec,
    connection_session_events_total: IntCounterVec,
    auth_logins_total: IntCounterVec,
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
        worker_ticks_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_worker_ticks_total",
                "Total worker loop ticks by worker and outcome",
            ),
            &["worker", "outcome"],
            registry
        )?,
        worker_tick_duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_worker_tick_duration_seconds",
                "Worker loop tick duration by worker",
            )
            .buckets(vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0]),
            &["worker"],
            registry
        )?,
        outbox_events: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_outbox_events",
                "Outbound events by durable queue state",
            ),
            &["state"],
            registry
        )?,
        outbox_oldest_pending_age_seconds: register_gauge_with_registry!(
            "piping_outbox_oldest_pending_age_seconds",
            "Age in seconds of the oldest outbound event awaiting delivery, or zero when the queue is empty",
            registry
        )?,
        outbox_batches_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_outbox_batches_total",
                "Total outbound worker batches by outcome",
            ),
            &["outcome"],
            registry
        )?,
        connection_sessions: register_int_gauge_vec_with_registry!(
            Opts::new(
                "piping_connection_sessions",
                "Connection sessions by protocol and lifecycle state",
            ),
            &["protocol", "state"],
            registry
        )?,
        connection_session_events_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_connection_session_events_total",
                "Total connection session lifecycle events by protocol and event",
            ),
            &["protocol", "event"],
            registry
        )?,
        auth_logins_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_auth_logins_total",
                "Total sign-in attempts by authentication method and outcome",
            ),
            &["method", "outcome"],
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
        for worker in Worker::ALL {
            metrics
                .worker_heartbeat_timestamp_seconds
                .with_label_values(&[worker.as_str()])
                .set(0.0);
            for outcome in TICK_OUTCOMES {
                metrics
                    .worker_ticks_total
                    .with_label_values(&[worker.as_str(), outcome]);
            }
            metrics
                .worker_tick_duration_seconds
                .with_label_values(&[worker.as_str()]);
        }
        for state in OUTBOX_STATES {
            metrics.outbox_events.with_label_values(&[state]).set(0);
        }
        metrics.outbox_oldest_pending_age_seconds.set(0.0);
        for outcome in OUTBOX_OUTCOMES {
            metrics.outbox_batches_total.with_label_values(&[outcome]);
        }
        for protocol in CONNECTION_PROTOCOLS {
            for state in CONNECTION_SESSION_STATES {
                metrics
                    .connection_sessions
                    .with_label_values(&[protocol, state])
                    .set(0);
            }
            for event in CONNECTION_SESSION_EVENTS {
                metrics
                    .connection_session_events_total
                    .with_label_values(&[protocol, event]);
            }
        }
        for method in AUTH_METHODS {
            for outcome in AUTH_OUTCOMES {
                metrics
                    .auth_logins_total
                    .with_label_values(&[method, outcome]);
            }
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

pub fn record_worker_heartbeat(worker: Worker) {
    runtime_metrics()
        .worker_heartbeat_timestamp_seconds
        .with_label_values(&[worker.as_str()])
        .set(Utc::now().timestamp_millis() as f64 / 1000.0);
}

/// One in-flight worker tick. Records the tick counter and its duration once;
/// a tick abandoned without an explicit outcome counts as `error`.
pub struct WorkerTickGuard {
    worker: Worker,
    started_at: std::time::Instant,
    recorded: bool,
}

impl WorkerTickGuard {
    /// Record the tick outcome. A second call is ignored.
    pub fn record(&mut self, ok: bool) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        let metrics = runtime_metrics();
        metrics
            .worker_ticks_total
            .with_label_values(&[self.worker.as_str(), if ok { "ok" } else { "error" }])
            .inc();
        metrics
            .worker_tick_duration_seconds
            .with_label_values(&[self.worker.as_str()])
            .observe(self.started_at.elapsed().as_secs_f64());
    }
}

impl Drop for WorkerTickGuard {
    fn drop(&mut self) {
        self.record(false);
    }
}

pub fn start_worker_tick(worker: Worker) -> WorkerTickGuard {
    WorkerTickGuard {
        worker,
        started_at: std::time::Instant::now(),
        recorded: false,
    }
}

/// Publish one outbox queue snapshot. Negative counts are clamped to zero; a
/// caller that failed to sample must not call this with fabricated zeros.
pub fn record_outbox_queue_snapshot(
    pending: i64,
    in_progress: i64,
    failed: i64,
    dead_letter: i64,
    oldest_pending_age_seconds: f64,
) {
    let metrics = runtime_metrics();
    for (state, value) in [
        ("pending", pending),
        ("in_progress", in_progress),
        ("failed", failed),
        ("dead_letter", dead_letter),
    ] {
        metrics
            .outbox_events
            .with_label_values(&[state])
            .set(value.max(0));
    }
    metrics
        .outbox_oldest_pending_age_seconds
        .set(oldest_pending_age_seconds.max(0.0));
}

/// Record one outbound worker batch outcome. Both outcomes count one sample per
/// batch, so the error share stays a ratio of batches rather than a mix of
/// batches and events.
pub fn record_outbox_batch(ok: bool) {
    runtime_metrics()
        .outbox_batches_total
        .with_label_values(&[if ok { "ok" } else { "error" }])
        .inc();
}

/// Set the current session gauges for one protocol. Unknown protocols are
/// ignored so a stray database value cannot create an unbounded series.
pub fn set_connection_session_state(protocol: &str, pending: i64, active: i64) {
    if !CONNECTION_PROTOCOLS.contains(&protocol) {
        return;
    }
    let metrics = runtime_metrics();
    for (state, value) in [("pending", pending), ("active", active)] {
        metrics
            .connection_sessions
            .with_label_values(&[protocol, state])
            .set(value.max(0));
    }
}

/// Count one connection session lifecycle event. Unknown protocol or event
/// values are ignored.
pub fn record_connection_session_event(protocol: &str, event: &str) {
    if !CONNECTION_PROTOCOLS.contains(&protocol) || !CONNECTION_SESSION_EVENTS.contains(&event) {
        return;
    }
    runtime_metrics()
        .connection_session_events_total
        .with_label_values(&[protocol, event])
        .inc();
}

/// Count one sign-in attempt. Unknown method or outcome values are ignored.
pub fn record_auth_login(method: &str, outcome: &str) {
    if !AUTH_METHODS.contains(&method) || !AUTH_OUTCOMES.contains(&outcome) {
        return;
    }
    runtime_metrics()
        .auth_logins_total
        .with_label_values(&[method, outcome])
        .inc();
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
