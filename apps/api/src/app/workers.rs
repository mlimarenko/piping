use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use futures_util::{StreamExt, stream};
use saffron::Cron;
use serde_json::Value;
use sqlx::Row;
use tokio::sync::watch;
use uuid::Uuid;

use crate::contexts::access_control::infrastructure::persistence::UserRepository;
use crate::contexts::connections::infrastructure::connection_session_snapshot;
use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::sync::infrastructure::{
    ProviderRequestJournalRepository, WebhookEventRepository, outbox_queue_snapshot,
};
use crate::{
    contexts::access_control::domain::Principal,
    contexts::audit::domain::{
        AuditContext, AuditDecision, AuditEmitter, AuditEmitterKind, AuditEvent, AuditOutcome,
        safe_projection::project_metadata,
    },
    contexts::connections::application::ResolveSshExecutionLaunchInput,
    contexts::playbooks::application::{
        LeaseDueSchedulesInput, LeaseQueuedRunsInput, ReleaseScheduleOccurrenceInput,
        ScheduleOccurrenceAdvance, playbook_service::LAUNCH_TRACEPARENT_KEY, principal_subject,
    },
    contexts::playbooks::domain::{
        InventorySelection, LaunchSpec, PlaybookRun, PlaybookRunGroup, PlaybookRunStatus,
        PlaybookScheduleMisfirePolicy, automation_lifecycle_event,
    },
    contexts::playbooks::infrastructure::persistence::{
        CompleteRunBatchItemRecord, CompleteRunGroupBatchRecord, PlaybookRepository,
    },
    contexts::playbooks::infrastructure::{
        AnsibleDispatchOutcome, AnsibleJobRequest, AnsibleRunnerClient, RUNNER_CONTRACT_VERSION,
        build_managed_inventory_line, sanitize_host_alias, snapshot_for_launch,
    },
    errors::AppError,
    observability::{
        EventFields, LogLevel, NoiseBudgetProfile, attach_parent_context_from_traceparent,
        instrument_span, log_error_event, log_event, metrics,
    },
};
use piping_contracts::playbook_runner::{
    ProjectSnapshot, ResolvedInventory, ResolvedLaunch, ResolvedTarget as WireResolvedTarget,
    SupervisorCallbacks,
};

use super::AppState;
use super::worker_backoff::{WorkerBackoff, WorkerRetryConfig};

const SESSION_RECONCILIATION_BATCH: u32 = 64;
const SESSION_RECONCILIATION_CONCURRENCY: usize = 8;
const SESSION_RECONCILIATION_INTERVAL_SECS: u64 = 10;
const SESSION_RECONCILIATION_TIMEOUT_SECS: u64 = 3;
/// Queue and pool gauges describe slow-moving state; sampling them on every
/// poll tick would add a full-table aggregate per hundred milliseconds.
const OUTBOX_QUEUE_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Rotate through bounded UUID-ordered batches; only authoritative generation-
/// pinned end evidence may change lifecycle. Missing or idle metrics do not.
pub(super) async fn run_connection_reconciliation_worker(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::ConnectionReconciliation);
    let mut after = None;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
        SESSION_RECONCILIATION_INTERVAL_SECS,
    ));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = interval.tick() => {
                metrics::record_worker_heartbeat(metrics::Worker::ConnectionReconciliation);
                sample_connection_sessions(&state).await;
                let mut tick =
                    metrics::start_worker_tick(metrics::Worker::ConnectionReconciliation);
                let batch = match tokio::time::timeout(
                    std::time::Duration::from_secs(SESSION_RECONCILIATION_TIMEOUT_SECS),
                    state.services.connection.list_reconcilable_sessions(after, SESSION_RECONCILIATION_BATCH),
                ).await {
                    Ok(Ok(batch)) => batch,
                    _ => {
                        tick.record(false);
                        log_worker_failure("connection.reconciliation.scan_failed", "database_error", "Session runtime scan failed".to_string(), "connection.reconciliation", None);
                        continue;
                    }
                };
                after = if batch.len() < SESSION_RECONCILIATION_BATCH as usize { None } else { batch.last().map(|session| session.id) };
                let probes = stream::iter(batch).for_each_concurrent(SESSION_RECONCILIATION_CONCURRENCY, |session| {
                    let service = state.services.connection.clone();
                    async move {
                        let operation = async {
                            if let Some(runtime) = service.get_connection_metrics(session.id).await?.and_then(|sample| sample.metrics.runtime) {
                                service.reconcile_runtime_end(&session, &runtime, AuditEmitterKind::Worker, None).await?;
                            }
                            Ok::<(), AppError>(())
                        };
                        // Unreachable proxies and timeouts are unknown evidence, not end evidence.
                        if let Ok(Err(_)) = tokio::time::timeout(std::time::Duration::from_secs(SESSION_RECONCILIATION_TIMEOUT_SECS), operation).await {
                            log_event(LogLevel::Debug, "connection.reconciliation.probe_unavailable", EventFields::default().with_operation("connection.reconciliation"));
                        }
                    }
                });
                tokio::select! {
                    _ = probes => tick.record(true),
                    _ = shutdown.changed() => {
                        tick.record(true);
                        return;
                    },
                }
            }
        }
    }
}

/// Publish the open-session gauge snapshot for every protocol.
///
/// A failed sample increments the sample-error counter and leaves the previous
/// gauge values in place: an unknown queue must not render as an idle one.
async fn sample_connection_sessions(state: &AppState) {
    let rows = match connection_session_snapshot(&state.db_pool).await {
        Ok(rows) => rows,
        Err(_) => {
            metrics::record_metrics_sample_error("connection_sessions");
            return;
        }
    };
    // The snapshot is complete, so reset first: a protocol that dropped to zero
    // open sessions has no row and would otherwise keep its previous value.
    for protocol in metrics::CONNECTION_PROTOCOL_LABELS {
        metrics::set_connection_session_state(protocol, 0, 0);
    }
    let mut by_protocol: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    for row in &rows {
        let entry = by_protocol.entry(row.protocol.as_str()).or_insert((0, 0));
        match row.state.as_str() {
            "pending" => entry.0 = entry.0.saturating_add(row.count),
            "active" => entry.1 = entry.1.saturating_add(row.count),
            _ => {}
        }
    }
    for (protocol, (pending, active)) in by_protocol {
        metrics::set_connection_session_state(protocol, pending, active);
    }
}

/// Maximum sidecar dispatch attempts per run group before it is terminally failed.
/// `playbook_run_group.dispatch_attempt` starts at 1 on the first dispatch, so this budget
/// allows two requeues after the initial failure.
const PLAYBOOK_DISPATCH_MAX_ATTEMPTS: i32 = 3;
/// Base backoff before a requeued run becomes leaseable again; scaled linearly
/// by the group attempt number so consecutive failures back off further.
const PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS: i64 = 30;

/// Log a worker tick failure with retry context and a tighter noise budget.
///
/// A perpetual worker facing a dead dependency emits the *same* failure every
/// attempt. The global policy already prevents an unbounded flood; this profile
/// tightens the worker-specific shape to the first failure in full, then one
/// aggregate per ten suppressed failures. The aggregate stays at `Error`, so
/// alerting still sees that the outage continues. The noise key is the worker's
/// operation — bounded and stable — so distinct workers never share a counter.
fn log_worker_failure(
    event: &str,
    error_type: &str,
    error_message: String,
    operation: &str,
    retry: Option<WorkerRetryContext>,
) {
    let mut fields = EventFields::default()
        .with_operation(operation)
        .with_status("failed")
        .with_noise_key(operation)
        .with_noise_budget(NoiseBudgetProfile::RETRYING_WORKER_FAILURE);
    if let Some(retry) = retry {
        fields = fields
            .with_context("retry_in_ms", retry.retry_in_ms.to_string())
            .with_context(
                "consecutive_failures",
                retry.consecutive_failures.to_string(),
            );
    }
    log_error_event(event, error_type, error_message, fields);
}

/// Retry scheduling context attached to a worker failure by the owning loop.
///
/// Absent for failures logged inside a tick, where the loop has not yet
/// decided the next delay.
#[derive(Debug, Clone, Copy)]
struct WorkerRetryContext {
    consecutive_failures: u32,
    retry_in_ms: u64,
}

impl WorkerRetryContext {
    fn new(backoff: &WorkerBackoff, retry_in_ms: u64) -> Self {
        Self {
            consecutive_failures: backoff.consecutive_failures(),
            retry_in_ms,
        }
    }
}

/// Whether a worker tick reached the dependency it polls.
///
/// Drives retry backoff: the loop cannot infer this from a tick that logs its
/// own errors and returns unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickOutcome {
    Healthy,
    DependencyFailed,
}

/// Emit a single recovery event on the first success after a failure streak.
///
/// Without this the aggregated failures have no visible end: the error stream
/// simply stops, which is indistinguishable from the worker dying. Call before
/// `on_success` so the streak length is still available.
fn log_worker_recovery(backoff: &WorkerBackoff, event: &str, operation: &str) {
    if !backoff.was_failing() {
        return;
    }
    log_event(
        LogLevel::Info,
        event,
        EventFields::default()
            .with_operation(operation)
            .with_status("recovered")
            .with_context(
                "consecutive_failures",
                backoff.consecutive_failures().to_string(),
            ),
    );
}

pub(super) async fn run_outbox_worker(state: AppState, mut shutdown_rx: watch::Receiver<bool>) {
    metrics::record_worker_heartbeat(metrics::Worker::Outbox);
    let sync_runtime = &state.settings.sync_runtime;
    let poll_interval_ms = sync_runtime.outbox_worker_poll_interval_ms;
    let batch_size = sync_runtime.outbox_worker_batch_size as usize;
    let outbound_service = state.services.outbound_service.clone();
    let mut backoff =
        WorkerBackoff::starting_immediately(poll_interval_ms, WorkerRetryConfig::POLLER);
    let mut last_queue_sample_at: Option<std::time::Instant> = None;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff.next_sleep_ms())) => {
                metrics::record_worker_heartbeat(metrics::Worker::Outbox);
                let mut tick = metrics::start_worker_tick(metrics::Worker::Outbox);
                match outbound_service.process_batch(batch_size).await {
                    Ok(_) => {
                        tick.record(true);
                        metrics::record_outbox_batch(true);
                        log_worker_recovery(
                            &backoff,
                            "sync.outbox.worker_tick.recovered",
                            "outbox_worker_tick",
                        );
                        backoff.on_success();
                    }
                    Err(error) => {
                        tick.record(false);
                        metrics::record_outbox_batch(false);
                        let retry_in_ms = backoff.on_failure();
                        log_worker_failure(
                            "sync.outbox.worker_tick.failed",
                            "outbox_worker_error",
                            error.to_string(),
                            "outbox_worker_tick",
                            Some(WorkerRetryContext::new(&backoff, retry_in_ms)),
                        );
                    }
                }
                if last_queue_sample_at
                    .is_none_or(|sampled_at| sampled_at.elapsed() >= OUTBOX_QUEUE_SAMPLE_INTERVAL)
                {
                    last_queue_sample_at = Some(std::time::Instant::now());
                    sample_outbox_queue(&state).await;
                }
            }
        }
    }
}

/// Publish outbox queue depth and database pool saturation at a bounded rate.
///
/// A failed snapshot increments the sample-error counter and leaves the queue
/// gauges untouched; the pool gauges still describe a live pool.
async fn sample_outbox_queue(state: &AppState) {
    match outbox_queue_snapshot(&state.db_pool).await {
        Ok(snapshot) => metrics::record_outbox_queue_snapshot(
            snapshot.pending,
            snapshot.in_progress,
            snapshot.failed,
            snapshot.dead_letter,
            snapshot.oldest_pending_age_seconds,
        ),
        Err(_) => metrics::record_metrics_sample_error("outbox"),
    }
    metrics::record_db_pool_state(
        state.db_pool.size(),
        state.db_pool.num_idle(),
        state.db_pool.options().get_max_connections(),
    );
}

/// Background poller that caches observability list-labels per host in Redis.
///
/// Each tick enumerates in-scope hosts, batch-queries the distinct
/// fleet (non-`$host`-templated) list metrics once per source, demultiplexes
/// the result vectors by correlation label, and writes per-host cached values
/// to Redis. The read endpoint `GET /api/v1/monitoring/host-labels` serves
/// those cached values without external calls. Only runs in backend mode
/// (this worker is spawned exclusively from `serve`, the backend entrypoint).
pub(super) async fn run_observability_poller_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::ObservabilityPoller);
    let poll_interval_ms = state.settings.monitoring.list_poll_interval_ms.max(1);
    let mut backoff =
        WorkerBackoff::starting_immediately(poll_interval_ms, WorkerRetryConfig::POLLER);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff.next_sleep_ms())) => {
                metrics::record_worker_heartbeat(metrics::Worker::ObservabilityPoller);
                let mut tick = metrics::start_worker_tick(metrics::Worker::ObservabilityPoller);
                match crate::contexts::monitoring::application::poll_observability_list_metrics(
                    &state.db_pool,
                    &state.redis,
                    &state.repositories.discovery,
                    state.services.secret_store.as_ref(),
                    &state.settings.monitoring,
                )
                .await
                {
                    Ok(summary) => {
                        tick.record(true);
                        log_worker_recovery(
                            &backoff,
                            "monitoring.observability.list_poll.recovered",
                            "observability_list_poll",
                        );
                        backoff.on_success();
                        log_event(
                            LogLevel::Info,
                            "monitoring.observability.list_poll.completed",
                            EventFields::default()
                                .with_operation("observability_list_poll")
                                .with_status("ok")
                                .with_context("hosts_scanned", summary.hosts_scanned.to_string())
                                .with_context(
                                    "distinct_queries",
                                    summary.distinct_queries.to_string(),
                                )
                                .with_context(
                                    "cache_keys_written",
                                    summary.cache_keys_written.to_string(),
                                )
                                .with_context(
                                    "alert_sources_queried",
                                    summary.alert_sources_queried.to_string(),
                                )
                                .with_context(
                                    "alert_cache_keys_written",
                                    summary.alert_cache_keys_written.to_string(),
                                ),
                        );
                    }
                    Err(error) => {
                        tick.record(false);
                        let retry_in_ms = backoff.on_failure();
                        log_worker_failure(
                            "monitoring.observability.list_poll.failed",
                            "observability_list_poll_error",
                            error.to_string(),
                            "observability_list_poll",
                            Some(WorkerRetryContext::new(&backoff, retry_in_ms)),
                        );
                    }
                }
            }
        }
    }
}

/// Prunes `sync_webhook_event` rows older than 90 days once per day.
///
/// Raw payloads are not stored, but the event metadata rows
/// themselves accumulate. A 90-day retention window keeps audit trails
/// reasonable while bounding table growth.
pub(super) async fn run_webhook_event_prune_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::WebhookEventPruner);
    // Prune interval: once every 24 hours.
    const PRUNE_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
    const RETENTION_DAYS: i64 = 90;

    let repo = Arc::new(state.repositories.sync_webhook_events.clone());
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(PRUNE_INTERVAL_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = ticker.tick() => {
                metrics::record_worker_heartbeat(metrics::Worker::WebhookEventPruner);
                let mut tick = metrics::start_worker_tick(metrics::Worker::WebhookEventPruner);
                match repo.prune_older_than(RETENTION_DAYS).await {
                    Ok(pruned) => {
                        tick.record(true);
                        log_event(
                            LogLevel::Info,
                            "sync.webhook_event.prune.completed",
                            EventFields::default()
                                .with_operation("webhook_event_prune")
                                .with_status("ok")
                                .with_context("pruned_rows", pruned.to_string())
                                .with_context("retention_days", RETENTION_DAYS.to_string()),
                        );
                    }
                    Err(error) => {
                        tick.record(false);
                        log_error_event(
                            "sync.webhook_event.prune.failed",
                            "webhook_event_prune_error",
                            error.to_string(),
                            EventFields::default()
                                .with_operation("webhook_event_prune")
                                .with_status("failed"),
                        );
                    }
                }
            }
        }
    }
}

/// Periodically prunes terminal-state rows from `provider_request_journal`.
///
/// Only `succeeded`, `failed_after_send`, `failed_no_send` rows older than
/// the configured retention are deleted. `prepared` and `sent_unknown` are
/// preserved indefinitely as they represent in-flight or unresolved attempts.
pub(super) async fn run_journal_prune_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::JournalPruner);
    const PRUNE_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
    let retention_days: i64 = std::env::var("PIPING_SYNC__JOURNAL_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let repo = Arc::new(state.repositories.sync_provider_request_journal.clone());
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(PRUNE_INTERVAL_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = ticker.tick() => {
                metrics::record_worker_heartbeat(metrics::Worker::JournalPruner);
                let mut tick = metrics::start_worker_tick(metrics::Worker::JournalPruner);
                match repo.prune_older_than(retention_days).await {
                    Ok(pruned) => {
                        tick.record(true);
                        metrics::record_netbox_journal_pruned_rows("terminal", pruned);
                        log_event(
                            LogLevel::Info,
                            "sync.journal.prune.completed",
                            EventFields::default()
                                .with_operation("journal_prune")
                                .with_status("ok")
                                .with_context("pruned_rows", pruned.to_string())
                                .with_context("retention_days", retention_days.to_string()),
                        );
                    }
                    Err(error) => {
                        tick.record(false);
                        log_error_event(
                            "sync.journal.prune.failed",
                            "journal_prune_error",
                            error.to_string(),
                            EventFields::default()
                                .with_operation("journal_prune")
                                .with_status("failed"),
                        );
                    }
                }
            }
        }
    }
}

pub(super) async fn run_license_refresh_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::LicenseRefresh);
    let refresh_interval_ms = state
        .settings
        .license
        .refresh_interval_seconds
        .max(30)
        .saturating_mul(1000);
    let retry_base_ms = state.settings.license.refresh_retry_base_ms.max(1000);
    let retry_max_ms = state
        .settings
        .license
        .refresh_retry_max_ms
        .max(retry_base_ms);
    let retry_jitter_ms = state.settings.license.refresh_retry_jitter_ms;
    // Licence refresh keeps its configurable retry bounds, but shares the one
    // backoff implementation rather than hand-rolling a second copy.
    let mut backoff = WorkerBackoff::starting_after_interval(
        refresh_interval_ms,
        WorkerRetryConfig {
            base_ms: retry_base_ms,
            max_ms: retry_max_ms,
            jitter_ms: retry_jitter_ms,
        },
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff.next_sleep_ms())) => {
                metrics::record_worker_heartbeat(metrics::Worker::LicenseRefresh);
                let mut tick = metrics::start_worker_tick(metrics::Worker::LicenseRefresh);
                if state.settings.license.base_url.trim().is_empty() {
                    tick.record(true);
                    backoff.on_success();
                    continue;
                }
                match state.services.licensing.refresh_license_state().await {
                    Ok(_) => {
                        tick.record(true);
                        backoff.on_success();
                        log_event(
                            LogLevel::Info,
                            "license.refresh.worker_tick.completed",
                            EventFields::default()
                                .with_operation("license_refresh_worker_tick")
                                .with_status("ok"),
                        );
                    }
                    Err(AppError::Validation(message)) if message.contains("License is not activated") => {
                        // No activation yet: expected state for fresh installs.
                        tick.record(true);
                        backoff.on_success();
                    }
                    Err(error) => {
                        tick.record(false);
                        let retry_in_ms = backoff.on_failure();
                        log_worker_failure(
                            "license.refresh.worker_tick.failed",
                            "license_refresh_worker_error",
                            error.to_string(),
                            "license_refresh_worker_tick",
                            Some(WorkerRetryContext::new(&backoff, retry_in_ms)),
                        );
                    }
                }
            }
        }
    }
}

// ============================================================================
// Playbook executor data-plane (sequential)
// ============================================================================

/// Background worker that drains queued playbook run groups: reaps timed-out
/// runs, leases groups, resolves one immutable launch per group, and dispatches
/// each group as a fully-specified Ansible job to the `ansible-runner` sidecar.
/// Backend mode only.
///
/// Dispatch is asynchronous: once a durable receipt is reconciled the group moves
/// to `running` and the sidecar streams events/artifacts/completion back through
/// supervisor callback tokens. The timeout reaper is the only backstop for
/// callbacks that never arrive.
///
/// Dispatch is sequential: the lease limit is small and groups dispatch one at a
/// time per tick. A definitively unaccepted sidecar dispatch is requeued with a
/// backoff (up to [`PLAYBOOK_DISPATCH_MAX_ATTEMPTS`]) before failing terminally.
pub(super) async fn run_playbook_run_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::PlaybookRun);
    let runtime = &state.settings.playbook_runtime;
    let poll_interval_ms = runtime.run_poll_interval_ms;
    let lease_owner = format!("playbook-run-worker-{}", Uuid::now_v7());
    // The sidecar client holds a reqwest connection pool; build it once and reuse
    // it across every tick rather than rebuilding on each poll.
    let runner_client = match AnsibleRunnerClient::from_settings(&state.settings) {
        Ok(client) => client,
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.runner_client_init_failed",
                "playbook_run_worker_runner_client_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_init")
                    .with_status("failed"),
            );
            return;
        }
    };
    let mut backoff =
        WorkerBackoff::starting_immediately(poll_interval_ms, WorkerRetryConfig::POLLER);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff.next_sleep_ms())) => {
                metrics::record_worker_heartbeat(metrics::Worker::PlaybookRun);
                let mut tick = metrics::start_worker_tick(metrics::Worker::PlaybookRun);
                if let Err(error) = refresh_playbook_run_metrics(&state).await {
                    log_error_event(
                        "playbooks.run_worker.metrics_refresh_failed",
                        "playbook_run_worker_metrics_error",
                        error.to_string(),
                        EventFields::default()
                            .with_operation("playbook_run_worker_metrics")
                            .with_status("failed"),
                    );
                }
                match run_playbook_run_worker_tick(&state, &lease_owner, &runner_client).await {
                    TickOutcome::Healthy => {
                        tick.record(true);
                        log_worker_recovery(
                            &backoff,
                            "playbooks.run_worker.recovered",
                            "playbook_run_worker_tick",
                        );
                        backoff.on_success();
                    }
                    TickOutcome::DependencyFailed => {
                        tick.record(false);
                        backoff.on_failure();
                    }
                }
            }
        }
    }
}

async fn run_playbook_run_worker_tick(
    state: &AppState,
    lease_owner: &str,
    runner_client: &AnsibleRunnerClient,
) -> TickOutcome {
    let runtime = &state.settings.playbook_runtime;
    let now = Utc::now();
    let mut outcome = TickOutcome::Healthy;

    // 1) Reap runs stuck past their timeout (crashed-worker recovery).
    match state
        .services
        .playbooks
        .timeout_stuck_runs(now, Some(runtime.timeout_reap_limit))
        .await
    {
        Ok(timed_out) if !timed_out.runs.is_empty() => {
            log_event(
                LogLevel::Warning,
                "playbooks.run_worker.reaped_timeouts",
                EventFields::default()
                    .with_operation("playbook_run_worker_reap")
                    .with_status("ok")
                    .with_context("reaped_runs", timed_out.runs.len().to_string()),
            );
        }
        Ok(_) => {}
        Err(error) => {
            outcome = TickOutcome::DependencyFailed;
            log_worker_failure(
                "playbooks.run_worker.reap_failed",
                "playbook_run_worker_reap_error",
                error.to_string(),
                "playbook_run_worker_reap",
                None,
            );
        }
    }

    // 2) Lease groups directly. The execution unit is the run group, so a Native
    // launch with zero host rows is leased and dispatched like any other group.
    let groups = match state
        .services
        .playbooks
        .lease_queued_groups(LeaseQueuedRunsInput {
            lease_owner: lease_owner.to_string(),
            limit: Some(runtime.run_lease_limit),
            lease_ttl_seconds: Some(runtime.run_lease_ttl_seconds),
            max_concurrent_groups: Some(i64::from(runtime.max_concurrent_runs)),
            max_concurrent_runs_per_group: Some(i64::from(runtime.max_concurrent_runs_per_group)),
        })
        .await
    {
        Ok(groups) => groups,
        Err(error) => {
            log_worker_failure(
                "playbooks.run_worker.lease_failed",
                "playbook_run_worker_lease_error",
                error.to_string(),
                "playbook_run_worker_lease",
                None,
            );
            return TickOutcome::DependencyFailed;
        }
    };

    for group in groups {
        // Each group continues the launching request's trace when one was stored.
        let span = tracing::info_span!(
            "playbooks.run_group",
            run_group_id = %group.id,
            trace_id = tracing::field::Empty,
            span_id = tracing::field::Empty,
        );
        if let Some(traceparent) = group
            .metadata
            .get(LAUNCH_TRACEPARENT_KEY)
            .and_then(Value::as_str)
        {
            attach_parent_context_from_traceparent(&span, traceparent);
        }
        instrument_span(
            span,
            execute_leased_run_group(state, lease_owner, runner_client, group),
        )
        .await;
    }

    outcome
}

/// Dispatch backoff: attempt 1 waits one base interval, attempt 2 two, and so
/// on. A definitively unaccepted dispatch is parked for the next lease.
fn dispatch_retry_delay_seconds(attempt: i32) -> i64 {
    PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS.saturating_mul(i64::from(attempt.max(1)))
}

#[derive(Debug, PartialEq, Eq)]
enum DispatchFailureAction {
    Requeue { retry_delay_seconds: i64 },
    Fail,
}

fn dispatch_failure_action(dispatch_attempt: i32) -> DispatchFailureAction {
    if dispatch_attempt >= PLAYBOOK_DISPATCH_MAX_ATTEMPTS {
        DispatchFailureAction::Fail
    } else {
        DispatchFailureAction::Requeue {
            retry_delay_seconds: dispatch_retry_delay_seconds(dispatch_attempt),
        }
    }
}

/// Validate the stored launch's pinned image reference.
///
/// `image` is the repository name and `image_digest` is only the
/// `sha256:<64 hex>` content digest; the stored launch is the sole image source
/// at dispatch, so mutable execution-environment configuration is never reloaded.
fn pinned_image(image: &str, image_digest: &str) -> Result<(String, String), AppError> {
    let image = image.trim();
    let digest = image_digest.trim();
    if image.is_empty() || !digest.starts_with("sha256:") || digest.len() != 71 {
        return Err(AppError::Validation(
            "Stored launch image must be a repository plus a sha256:<64 hex> digest".to_string(),
        ));
    }
    Ok((image.to_string(), digest.to_string()))
}

/// Resolve the run group's live initiator.
///
/// Manual and scheduled runs both re-check their requesting user.
pub(crate) async fn live_launch_principal(
    state: &AppState,
    group: &PlaybookRunGroup,
) -> Result<Principal, AppError> {
    match group.requested_by_user_id {
        Some(user_id) => {
            let user = state.repositories.users.get_by_id(user_id).await?;
            manual_launch_principal(user_id, user.as_ref())
        }
        None => Err(AppError::Forbidden(
            "playbook run group has no live initiator to re-check".to_string(),
        )),
    }
}

fn manual_launch_principal(
    user_id: Uuid,
    user: Option<&crate::contexts::access_control::domain::User>,
) -> Result<Principal, AppError> {
    if !user.is_some_and(|user| user.id == user_id && user.is_active()) {
        return Err(AppError::Forbidden(
            "playbook run initiator no longer exists or is not active".to_string(),
        ));
    }
    Ok(Principal::user(user_id))
}

fn launch_target_for_run<'a>(
    launch: &'a LaunchSpec,
    run: &PlaybookRun,
) -> Option<&'a crate::contexts::playbooks::domain::ResolvedTarget> {
    launch
        .targets
        .iter()
        .find(|target| match (target.target_id, run.target_id) {
            (Some(left), Some(right)) => left == right,
            _ => target.hostname == run.hostname,
        })
}

struct PreparedGroupLaunch {
    snapshot: ProjectSnapshot,
    secret_env: BTreeMap<String, String>,
    native_connection: Option<Value>,
}

/// Load and verify everything that must hold before the group may start.
///
/// The immutable revision is loaded by the IDs captured in the stored launch and
/// its digest is recomputed with the shared snapshot function. Neither the
/// project, the template, the image configuration nor the target set is reloaded.
async fn prepare_group_launch(
    state: &AppState,
    group: &PlaybookRunGroup,
) -> Result<PreparedGroupLaunch, String> {
    let launch = &group.launch_spec;
    let revision = state
        .services
        .playbooks
        .get_project_revision(launch.project_id, launch.revision_id)
        .await
        .map_err(|error| format!("Failed to load the pinned project revision: {error}"))?
        .ok_or_else(|| "Pinned project revision was not found".to_string())?;
    let snapshot = snapshot_for_launch(launch, revision).map_err(|error| error.to_string())?;
    let principal = live_launch_principal(state, group)
        .await
        .map_err(|error| error.to_string())?;
    let fresh_authorization = super::playbook_secret_gate::authorize_prepared_launch(
        &state.db_pool,
        &state.services.access_control,
        &state.repositories.secrets,
        &principal,
        None,
        launch,
    )
    .await
    .map_err(|error| error.to_string())?;
    let native_connection = match &launch.inventory {
        InventorySelection::Native {
            ssh_record_id: Some(record_id),
            ..
        } => {
            let contexts: Vec<Uuid> = serde_json::from_value(
                group
                    .metadata
                    .get("ssh_record_contexts")
                    .cloned()
                    .ok_or_else(|| {
                        "Native SSH credential delegation is missing from the launch snapshot"
                            .to_string()
                    })?,
            )
            .map_err(|_| "Native SSH credential delegation snapshot is invalid".to_string())?;
            let credential_user_id = match &principal {
                Principal::User { id } => Some(*id),
                Principal::ServicePrincipal(_) => None,
            };
            Some(
                state
                    .services
                    .connection
                    .resolve_delegated_ssh_record(
                        *record_id,
                        credential_user_id,
                        crate::contexts::connections::application::connection_service::SshRecordPurpose::NativeDefaults,
                        &super::playbook_secret_gate::RecordSshDelegation::for_dispatch(
                            &state.services.access_control,
                            &principal,
                            &contexts,
                        ),
                    )
                    .await
                    .map_err(|error| error.to_string())?,
            )
        }
        _ => None,
    };
    let authorized = parse_secret_env_authorization(&group.metadata);
    verify_authorization_snapshot(
        fresh_authorization.as_ref(),
        &authorized,
        &launch.options.secret_refs,
    )
    .map_err(|error| error.to_string())?;
    let secret_env = resolve_group_secret_env(state, group, &principal, &authorized)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PreparedGroupLaunch {
        snapshot,
        secret_env,
        native_connection,
    })
}

/// Execute one group leased by this worker.
///
/// The execution unit is the group, not a host row: a Native launch with zero
/// Piping run rows is dispatched exactly like a managed fan-out.
async fn execute_leased_run_group(
    state: &AppState,
    lease_owner: &str,
    runner_client: &AnsibleRunnerClient,
    group: PlaybookRunGroup,
) {
    if group.lease_owner.as_deref() != Some(lease_owner)
        || group.status != PlaybookRunStatus::Leased
    {
        return;
    }
    let run_group_id = group.id;
    let prepared = match prepare_group_launch(state, &group).await {
        Ok(prepared) => prepared,
        Err(reason) => {
            fail_group(state, lease_owner, &group, &reason).await;
            return;
        }
    };
    let started = match state
        .services
        .playbooks
        .start_leased_run_group(run_group_id, lease_owner)
        .await
    {
        Ok(started) => started,
        Err(AppError::Conflict(reason)) => {
            log_worker_failure(
                "playbooks.run_worker.start_conflict",
                "playbook_run_worker_start_conflict",
                reason,
                "playbook_run_worker_start",
                None,
            );
            return;
        }
        Err(error) => {
            fail_group(
                state,
                lease_owner,
                &group,
                &format!("Failed to start run group: {error}"),
            )
            .await;
            return;
        }
    };
    if started.run_group.id != run_group_id {
        fail_group(
            state,
            lease_owner,
            &started.run_group,
            "Started run group identity does not match the leased group",
        )
        .await;
        return;
    }
    let job = match build_group_job(state, lease_owner, &group, &prepared, &started).await {
        Ok(job) => job,
        Err(reason) => {
            fail_group(state, lease_owner, &started.run_group, &reason).await;
            return;
        }
    };
    dispatch_started_group(state, lease_owner, runner_client, &started.run_group, job).await;
}

fn allocate_target_alias(base: &str, allocated: &mut BTreeSet<String>) -> String {
    let base = sanitize_host_alias(base);
    if allocated.insert(base.clone()) {
        return base;
    }
    let mut index = 2_u32;
    loop {
        let candidate = format!("{base}_{index}");
        if allocated.insert(candidate.clone()) {
            return candidate;
        }
        index = index.saturating_add(1);
    }
}

fn run_inventory_alias_base(run: &PlaybookRun) -> String {
    sanitize_host_alias(&run.hostname)
}

fn run_inventory_alias_with_suffix(run: &PlaybookRun, base: &str) -> String {
    let suffix = run.id.simple().to_string();
    let unique_suffix_start = suffix.len().saturating_sub(16);
    format!("{base}_{}", &suffix[unique_suffix_start..])
}

fn allocate_run_inventory_alias(run: &PlaybookRun, allocated: &mut BTreeSet<String>) -> String {
    let base = run_inventory_alias_base(run);
    if allocated.insert(base.clone()) {
        return base;
    }
    let alias = run_inventory_alias_with_suffix(run, &base);
    allocated.insert(alias.clone());
    alias
}

fn piping_host_vars(run: &PlaybookRun) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::from([("piping_run_id".to_string(), run.id.to_string())]);
    if let Some(kind) = run.source_kind.as_deref() {
        vars.insert("piping_source_kind".to_string(), kind.to_string());
    }
    if let Some(source_id) = run.source_id {
        vars.insert("piping_source_id".to_string(), source_id.to_string());
    }
    if let Some(kind) = run.target_kind.as_deref() {
        vars.insert("piping_target_kind".to_string(), kind.to_string());
    }
    if let Some(target_id) = run.target_id {
        vars.insert("piping_target_id".to_string(), target_id.to_string());
    }
    if let Some(name) = run
        .target_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        vars.insert("piping_target_name".to_string(), name.to_string());
    }
    vars
}

/// Build the protocol-7 dispatch request from the stored launch, the immutable
/// snapshot and the started group's supervisor and sandbox tokens.
async fn build_group_job(
    state: &AppState,
    lease_owner: &str,
    group: &PlaybookRunGroup,
    prepared: &PreparedGroupLaunch,
    started: &crate::contexts::playbooks::application::PlaybookRunGroupExecution,
) -> Result<AnsibleJobRequest, String> {
    let launch = &group.launch_spec;
    let (image, image_digest) =
        pinned_image(&launch.image, &launch.image_digest).map_err(|error| error.to_string())?;
    let dispatch_id = started
        .run_group
        .dispatch_id
        .ok_or_else(|| "Started run group has no dispatch identity".to_string())?;

    let mut allocated_aliases = BTreeSet::new();
    let mut run_tokens = BTreeMap::new();
    let mut targets = Vec::new();
    let inventory = match &launch.inventory {
        InventorySelection::Managed { .. } => {
            let mut content = String::new();
            for execution in &started.executions {
                let run = &execution.run;
                let target = launch_target_for_run(launch, run).ok_or_else(|| {
                    format!("Run {} is not part of the stored launch targets", run.id)
                })?;
                let target_id = target
                    .target_id
                    .ok_or_else(|| format!("Managed run {} has no connection target", run.id))?;
                let connection = state
                    .services
                    .connection
                    .resolve_ssh_execution_launch(
                        ResolveSshExecutionLaunchInput {
                            execution_id: run.id,
                            target_id,
                            // A service principal is never replaced by the
                            // schedule's delegating creator: only a live user
                            // subject may select personal credentials.
                            credential_user_id: group.requested_by_user_id,
                            session_metadata: None,
                            protocol_params: None,
                            requested_proxy_node_id: None,
                        },
                        Some(state.services.access_control.as_ref()),
                        None,
                    )
                    .await
                    .map_err(|error| {
                        format!("Failed to resolve SSH launch for target {target_id}: {error}")
                    })?;
                let alias = allocate_run_inventory_alias(run, &mut allocated_aliases);
                let line = build_managed_inventory_line(
                    &alias,
                    &connection.proxy_payload,
                    &piping_host_vars(run),
                )
                .map_err(|error| error.to_string())?;
                content.push_str(&line);
                run_tokens.insert(run.id, execution.job_token.clone());
                targets.push(WireResolvedTarget {
                    hostname: alias,
                    run_id: Some(run.id),
                    target_id: Some(target_id),
                    source_id: run.source_id,
                    // Preserve the whole resolver payload, including the ordered
                    // jump chain and known_hosts_lines. Never strip the route.
                    connection: Some(connection.proxy_payload),
                });
            }
            ResolvedInventory::Managed { content }
        }
        InventorySelection::Native { sources, .. } => {
            for target in &launch.targets {
                let alias = allocate_target_alias(&target.hostname, &mut allocated_aliases);
                targets.push(WireResolvedTarget {
                    hostname: alias,
                    run_id: None,
                    target_id: target.target_id,
                    source_id: target.source_id,
                    connection: None,
                });
            }
            ResolvedInventory::Native {
                sources: sources.clone(),
            }
        }
    };

    Ok(AnsibleJobRequest {
        protocol_version: RUNNER_CONTRACT_VERSION,
        dispatch_id,
        dispatch_attempt: started.run_group.dispatch_attempt,
        run_group_id: group.id,
        callbacks: SupervisorCallbacks {
            group_token: started.group_callback_token.clone(),
            run_tokens,
        },
        sandbox_run_tokens: started.sandbox_run_tokens.clone(),
        lease_owner: lease_owner.to_string(),
        callback_base_url: state
            .settings
            .playbook_runtime
            .self_callback_base_url
            .clone(),
        project: prepared.snapshot.clone(),
        launch: ResolvedLaunch {
            image,
            image_digest,
            workdir: launch.workdir.clone(),
            playbook_paths: launch.playbook_paths.clone(),
            inventory,
            connection: prepared.native_connection.clone(),
            targets,
            argv: launch.options.argv.clone(),
            extra_vars: launch.options.extra_vars.clone(),
            limit: launch.options.limit.clone(),
            env: launch.options.env.clone(),
            secrets: prepared.secret_env.clone(),
            timeout_seconds: launch.options.timeout_seconds,
        },
    })
}

/// Check cancellation against durable state before dispatching the sidecar job.
async fn dispatch_started_group(
    state: &AppState,
    lease_owner: &str,
    runner_client: &AnsibleRunnerClient,
    group: &PlaybookRunGroup,
    job: AnsibleJobRequest,
) {
    let run_group_id = group.id;
    let dispatch_id = job.dispatch_id;
    let dispatch_attempt = job.dispatch_attempt;
    match state
        .services
        .playbooks
        .get_run_group_with_runs(run_group_id, 1)
        .await
    {
        Ok(Some(current)) if current.run_group.cancel_requested_at.is_some() => return,
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.pre_dispatch_reload_failed",
                "playbook_run_worker_dispatch_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_pre_dispatch_reload")
                    .with_status("failed")
                    .with_resource("playbook_run_group", run_group_id.to_string()),
            );
            return;
        }
    }

    match runner_client.dispatch_job(&job).await {
        AnsibleDispatchOutcome::Accepted(_) => {
            mark_group_running(state, lease_owner, group).await;
        }
        AnsibleDispatchOutcome::NotAccepted(reason) => {
            requeue_or_fail_group(
                state,
                lease_owner,
                group,
                &format!("Ansible runner did not accept dispatch: {reason}"),
            )
            .await;
        }
        AnsibleDispatchOutcome::AcceptanceUnknown(reason) => {
            log_error_event(
                "playbooks.run_worker.dispatch_acceptance_unknown",
                "playbook_run_worker_dispatch_acceptance_unknown",
                reason,
                EventFields::default()
                    .with_operation("playbook_run_worker_dispatch_reconcile")
                    .with_status("dispatching")
                    .with_resource("playbook_run_group", run_group_id.to_string())
                    .with_context("dispatch_id", dispatch_id.to_string())
                    .with_context("dispatch_attempt", dispatch_attempt.to_string()),
            );
            // The durable DB identity and callback tokens remain unchanged.
            // A timeout or a late runner callback may terminalize the group,
            // but no new execution can be minted without a 404 receipt proof.
        }
    }
}

async fn mark_group_running(state: &AppState, lease_owner: &str, group: &PlaybookRunGroup) {
    match state
        .services
        .playbooks
        .mark_run_group_running(group.id, lease_owner)
        .await
    {
        Ok(Some(running)) => {
            record_worker_group_audit(state, "playbook.run_group.dispatched", &running).await;
        }
        Ok(None) | Err(AppError::Conflict(_)) => {}
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.mark_running_failed",
                "playbook_run_worker_mark_running_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_mark_running")
                    .with_status("failed")
                    .with_resource("playbook_run_group", group.id.to_string()),
            );
        }
    }
}

async fn record_worker_group_audit(state: &AppState, event_type: &str, group: &PlaybookRunGroup) {
    // The transition is already committed; audit failure must not retry the operation.
    let result = match worker_group_audit_event(event_type, group, Utc::now()) {
        Ok(event) => state.services.audit.record_event(event).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        log_error_event(
            "playbooks.worker.audit_failed",
            "playbook_worker_audit_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_worker_audit")
                .with_status("failed")
                .with_resource("playbook_run_group", group.id.to_string()),
        );
    }
}

fn schedule_launch_failed_event(
    schedule_id: Uuid,
    at: DateTime<Utc>,
) -> Result<AuditEvent, AppError> {
    let event_type = "playbook.schedule.launch_failed";
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: event_type.to_string(),
        actor_id: None,
        actor_kind: "system".to_string(),
        target_type: Some("playbook_schedule".to_string()),
        target_id: Some(schedule_id),
        metadata: project_metadata(event_type, &serde_json::json!({})),
        created_at: at,
    }
    .with_context(AuditContext {
        outcome: AuditOutcome::Failed,
        emitter: Some(AuditEmitter {
            kind: AuditEmitterKind::Worker,
            id: None,
        }),
        ..Default::default()
    })
}

async fn record_schedule_launch_failure(state: &AppState, schedule_id: Uuid) {
    // The occurrence was durably leased. Recording failure is best-effort and
    // must not prevent its normal release and retry handling.
    let result = match schedule_launch_failed_event(schedule_id, Utc::now()) {
        Ok(event) => state.services.audit.record_event(event).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        log_error_event(
            "playbooks.worker.audit_failed",
            "playbook_worker_audit_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_worker_audit")
                .with_status("failed")
                .with_resource("playbook_schedule", schedule_id.to_string()),
        );
    }
}

/// Only a policy refusal is permanent: the schedule must stop instead of
/// retrying the same occurrence on every tick. Storage and availability errors
/// stay transient and keep the existing retry behaviour.
fn schedule_authority_lost(error: &AppError) -> bool {
    matches!(error, AppError::Forbidden(_))
}

fn schedule_owner_revoked_event(
    schedule_id: Uuid,
    at: DateTime<Utc>,
) -> Result<AuditEvent, AppError> {
    let event_type = "playbook.schedule.owner_access_revoked";
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: event_type.to_string(),
        actor_id: None,
        actor_kind: "system".to_string(),
        target_type: Some("playbook_schedule".to_string()),
        target_id: Some(schedule_id),
        metadata: project_metadata(event_type, &serde_json::json!({})),
        created_at: at,
    }
    .with_context(AuditContext {
        outcome: AuditOutcome::Denied,
        decision: Some(AuditDecision::Denied),
        emitter: Some(AuditEmitter {
            kind: AuditEmitterKind::Worker,
            id: None,
        }),
        ..Default::default()
    })
}

async fn record_schedule_owner_revoked(state: &AppState, schedule_id: Uuid) {
    // The schedule is already disabled; audit failure must not resurrect it.
    let result = match schedule_owner_revoked_event(schedule_id, Utc::now()) {
        Ok(event) => state.services.audit.record_event(event).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        log_error_event(
            "playbooks.worker.audit_failed",
            "playbook_worker_audit_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_worker_audit")
                .with_status("failed")
                .with_resource("playbook_schedule", schedule_id.to_string()),
        );
    }
}

fn worker_group_audit_event(
    event_type: &str,
    group: &PlaybookRunGroup,
    at: DateTime<Utc>,
) -> Result<AuditEvent, AppError> {
    let mut event = automation_lifecycle_event(
        event_type,
        group,
        None,
        AuditOutcome::Succeeded,
        AuditEmitterKind::Worker,
        at,
    )?;
    if event_type == "playbook.schedule.fired" {
        let schedule_id = group.schedule_id.ok_or_else(|| {
            AppError::Validation("Scheduled run group has no schedule identity".to_string())
        })?;
        event.target_type = Some("playbook_schedule".to_string());
        event.target_id = Some(schedule_id);
    }
    Ok(event)
}

/// Requeue only after a receipt lookup returned 404, which proves this dispatch
/// identity was never accepted. Ambiguous failures never call this function.
async fn requeue_or_fail_group(
    state: &AppState,
    lease_owner: &str,
    group: &PlaybookRunGroup,
    reason: &str,
) {
    let retry_delay_seconds = match dispatch_failure_action(group.dispatch_attempt) {
        DispatchFailureAction::Fail => {
            fail_group(
                state,
                lease_owner,
                group,
                &format!(
                    "{reason} (dispatch attempt {}/{} exhausted)",
                    group.dispatch_attempt, PLAYBOOK_DISPATCH_MAX_ATTEMPTS
                ),
            )
            .await;
            return;
        }
        DispatchFailureAction::Requeue {
            retry_delay_seconds,
        } => retry_delay_seconds,
    };
    let retry_at = Utc::now()
        .checked_add_signed(Duration::seconds(retry_delay_seconds))
        .unwrap_or_else(Utc::now);
    match state
        .services
        .playbooks
        .requeue_run_group(group.id, lease_owner, retry_at, reason)
        .await
    {
        Ok(true) => {
            // The repository changes exactly these fields when the guarded update succeeds.
            let mut requeued = group.clone();
            requeued.status = PlaybookRunStatus::Leased;
            requeued.dispatch_id = None;
            record_worker_group_audit(state, "playbook.run_group.retry_scheduled", &requeued).await;
            metrics::record_playbook_dispatch_retries(1);
            log_event(
                LogLevel::Warning,
                "playbooks.run_worker.dispatch_requeued",
                EventFields::default()
                    .with_operation("playbook_run_worker_dispatch")
                    .with_status("requeued")
                    .with_resource("playbook_run_group", group.id.to_string())
                    .with_context("retry_delay_seconds", retry_delay_seconds.to_string())
                    .with_context("reason", reason.to_string()),
            );
        }
        Ok(false) => {}
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.dispatch_requeue_failed",
                "playbook_run_worker_dispatch_requeue_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_dispatch")
                    .with_status("failed")
                    .with_resource("playbook_run_group", group.id.to_string())
                    .with_context("reason", reason.to_string()),
            );
            fail_group(state, lease_owner, group, reason).await;
        }
    }
}

/// Terminally fail a leased or dispatching group, including a Native launch with
/// zero host rows. Per-host rows are failed explicitly when they exist; the
/// group outcome is always carried in `group_status`.
async fn fail_group(state: &AppState, lease_owner: &str, group: &PlaybookRunGroup, reason: &str) {
    log_event(
        LogLevel::Warning,
        "playbooks.run_worker.group_failed",
        EventFields::default()
            .with_operation("playbook_run_worker_fail")
            .with_status("failed")
            .with_resource("playbook_run_group", group.id.to_string())
            .with_context("reason", reason.to_string()),
    );
    let runs = match state
        .services
        .playbooks
        .get_run_group_with_runs(group.id, 50_000)
        .await
    {
        Ok(Some(detail)) => detail.runs,
        Ok(None) => Vec::new(),
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.group_load_failed",
                "playbook_run_worker_group_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_group_load")
                    .with_status("failed")
                    .with_resource("playbook_run_group", group.id.to_string()),
            );
            Vec::new()
        }
    };
    let record = CompleteRunGroupBatchRecord {
        run_group_id: group.id,
        lease_owner: lease_owner.to_string(),
        runs: runs
            .iter()
            .map(|run| CompleteRunBatchItemRecord {
                run_id: run.id,
                status: PlaybookRunStatus::Failed,
                exit_code: None,
                error_message: Some(reason.to_string()),
            })
            .collect(),
        group_status: PlaybookRunStatus::Failed,
        exit_code: None,
        error_message: Some(reason.to_string()),
        // This is an internal pre-acceptance failure, not a supervisor receipt.
        // The lease-guarded completion path must not invent a dispatch receipt
        // or claim that the supervisor sealed a final console prefix.
        dispatch_id: None,
        dispatch_attempt: None,
        console_next_sequence: None,
        finished_at: Utc::now(),
    };
    if let Err(error) = state
        .repositories
        .playbooks
        .complete_run_group_batch(record)
        .await
    {
        log_error_event(
            "playbooks.run_worker.fail_complete_failed",
            "playbook_run_worker_fail_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_run_worker_fail")
                .with_status("failed")
                .with_resource("playbook_run_group", group.id.to_string()),
        );
    }
}

/// Parsed C3 authorization snapshot: `ENV_NAME -> (secret_id, key)`.
///
/// Written server-side by the launch-time C3 gate,
/// it records exactly which secret each declared env name was authorized to
/// resolve — by id, so a later spec cannot swap the secret behind an env name.
/// Values are never present in the snapshot.
fn parse_secret_env_authorization(metadata: &Value) -> BTreeMap<String, (Uuid, String)> {
    metadata
        .get("secret_env_authorization")
        .map(parse_authorization_env)
        .unwrap_or_default()
}

fn parse_authorization_env(snapshot: &Value) -> BTreeMap<String, (Uuid, String)> {
    let mut out = BTreeMap::new();
    let Some(env) = snapshot.get("env").and_then(Value::as_object) else {
        return out;
    };
    for (env_name, entry) in env {
        let secret_id = entry
            .get("secret_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        let key = entry.get("key").and_then(Value::as_str);
        if let (Some(secret_id), Some(key)) = (secret_id, key) {
            out.insert(env_name.clone(), (secret_id, key.to_string()));
        }
    }
    out
}

/// The dispatch re-check must reproduce exactly the authorization the spec was
/// stored with. A changed secret, key or grant fails closed.
fn verify_authorization_snapshot(
    fresh: Option<&Value>,
    stored: &BTreeMap<String, (Uuid, String)>,
    secret_refs: &BTreeMap<String, Uuid>,
) -> Result<(), AppError> {
    if secret_refs.is_empty() {
        if fresh.is_some() || !stored.is_empty() {
            return Err(AppError::Forbidden(
                "launch secret authorization changed since the run group was stored".to_string(),
            ));
        }
        return Ok(());
    }
    let Some(fresh) = fresh else {
        return Err(AppError::Forbidden(
            "launch secret authorization is no longer available for this run group".to_string(),
        ));
    };
    if &parse_authorization_env(fresh) != stored {
        return Err(AppError::Forbidden(
            "launch secret authorization changed since the run group was stored".to_string(),
        ));
    }
    Ok(())
}

/// Map the stored spec's `env -> secret_id` references onto the authorized
/// `env -> (secret_id, key)` snapshot. Any drift fails closed.
fn declared_secret_env(
    secret_refs: &BTreeMap<String, Uuid>,
    authorized: &BTreeMap<String, (Uuid, String)>,
) -> Result<BTreeMap<String, String>, AppError> {
    let mut declared = BTreeMap::new();
    for (env_name, secret_id) in secret_refs {
        let (authorized_id, key) = authorized.get(env_name).ok_or_else(|| {
            AppError::Forbidden(format!(
                "secret authorization snapshot mismatch: env `{env_name}` was not authorized for this run"
            ))
        })?;
        if authorized_id != secret_id {
            return Err(AppError::Forbidden(format!(
                "secret authorization snapshot mismatch: env `{env_name}` now references a different secret than was authorized"
            )));
        }
        declared.insert(env_name.clone(), key.clone());
    }
    Ok(declared)
}

/// The stored identity a dispatched run group re-checks its C3 secret grants
/// against, derived from the run group's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretRecheckSubject {
    User(Uuid),
    ServicePrincipal(Uuid),
}

async fn resolve_group_secret_env(
    state: &AppState,
    group: &PlaybookRunGroup,
    principal: &Principal,
    authorized: &BTreeMap<String, (Uuid, String)>,
) -> Result<BTreeMap<String, String>, AppError> {
    if group.launch_spec.options.secret_refs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let declared = declared_secret_env(&group.launch_spec.options.secret_refs, authorized)?;
    let target_ids = group
        .launch_spec
        .targets
        .iter()
        .filter_map(|target| target.target_id)
        .collect::<Vec<_>>();
    if target_ids.is_empty() {
        return resolve_secret_env_without_target(state, principal, &declared, authorized).await;
    }

    let mut group_env: Option<BTreeMap<String, String>> = None;
    for target_id in target_ids {
        let env = resolve_secret_env_for_principal(
            state, group.id, target_id, &declared, authorized, principal,
        )
        .await?;
        match &group_env {
            Some(existing) if existing != &env => {
                return Err(AppError::Forbidden(
                    "Run group resolves different runtime secret environment values across targets; grouped execution requires identical runtime secrets"
                        .to_string(),
                ));
            }
            None => group_env = Some(env),
            _ => {}
        }
    }
    Ok(group_env.unwrap_or_default())
}

/// Resolve the runtime secret env exported to the runner, enforced at execution
/// time against the C3 authorization snapshot (fail-closed).
///
/// `declared` is what the stored launch asks for (`ENV -> key`); `authorized` is
/// the launch-time snapshot (`ENV -> (secret_id, key)`). A secret is exported
/// only when the subject still holds the `Execute x SecretVariable` grant, the
/// secret is not protected, and it is applicable to the target's DCIM scope.
pub async fn resolve_runtime_secret_env(
    state: &AppState,
    run_id: Uuid,
    target_id: Uuid,
    declared: &BTreeMap<String, String>,
    authorized: &BTreeMap<String, (Uuid, String)>,
    subject: Option<SecretRecheckSubject>,
) -> Result<BTreeMap<String, String>, AppError> {
    if declared.is_empty() {
        return Ok(BTreeMap::new());
    }
    let principal = match subject {
        Some(SecretRecheckSubject::User(id)) => Principal::user(id),
        Some(SecretRecheckSubject::ServicePrincipal(id)) => {
            state
                .services
                .playbooks
                .load_validated_service_principal(id)
                .await?
        }
        None => {
            return Err(AppError::Forbidden(
                "playbook declares runtime secrets but the run has no re-checkable initiator"
                    .to_string(),
            ));
        }
    };
    resolve_secret_env_for_principal(state, run_id, target_id, declared, authorized, &principal)
        .await
}

async fn resolve_secret_env_for_principal(
    state: &AppState,
    run_id: Uuid,
    target_id: Uuid,
    declared: &BTreeMap<String, String>,
    authorized: &BTreeMap<String, (Uuid, String)>,
    principal: &Principal,
) -> Result<BTreeMap<String, String>, AppError> {
    let scoped = state
        .repositories
        .secrets
        .list_scope_secrets("connection_target", target_id)
        .await?;
    let mut visible_protected = BTreeMap::new();
    for variable in scoped.variables {
        visible_protected.insert(variable.id, variable.is_protected);
    }

    let mut env = BTreeMap::new();
    for (env_name, key) in declared {
        let (secret_id, authorized_key) = authorized.get(env_name).ok_or_else(|| {
            AppError::Forbidden(format!(
                "secret authorization snapshot mismatch: env `{env_name}` was not authorized for this run"
            ))
        })?;
        if authorized_key != key {
            return Err(AppError::Forbidden(format!(
                "secret authorization snapshot mismatch: env `{env_name}` now resolves a different secret than was authorized"
            )));
        }
        if !super::playbook_secret_gate::principal_secret_use_grant(
            &state.services.access_control,
            principal,
            *secret_id,
            authorized_key,
        )
        .await?
        {
            return Err(AppError::Forbidden(format!(
                "secret access revoked for the run initiator on `{authorized_key}` (env {env_name})"
            )));
        }
        match visible_protected.get(secret_id) {
            Some(true) => {
                return Err(AppError::Forbidden(format!(
                    "runtime secret `{authorized_key}` (env {env_name}) is protected and cannot be exported"
                )));
            }
            Some(false) => {}
            None => {
                return Err(AppError::Validation(format!(
                    "runtime secret `{authorized_key}` (env {env_name}) is not applicable to the run target"
                )));
            }
        }
        let value = state.services.secret_store.get_secret(*secret_id).await?;
        env.insert(env_name.clone(), value);
    }

    log_event(
        LogLevel::Info,
        "playbooks.run.secrets_used",
        EventFields::default()
            .with_operation("playbook_runtime_secret_resolution")
            .with_status("ok")
            .with_resource("playbook_run", run_id.to_string())
            .with_context("secret_count", env.len().to_string())
            .with_context("subject", principal_subject(principal)),
    );
    Ok(env)
}

/// Resolve a zero-host Native launch's runtime secrets.
///
/// There is no connection-target scope to fence against, so protected secrets
/// stay refused and the principal's live C3 grant is still required.
async fn resolve_secret_env_without_target(
    state: &AppState,
    principal: &Principal,
    declared: &BTreeMap<String, String>,
    authorized: &BTreeMap<String, (Uuid, String)>,
) -> Result<BTreeMap<String, String>, AppError> {
    let mut env = BTreeMap::new();
    for (env_name, key) in declared {
        let (secret_id, authorized_key) = authorized.get(env_name).ok_or_else(|| {
            AppError::Forbidden(format!(
                "secret authorization snapshot mismatch: env `{env_name}` was not authorized for this run"
            ))
        })?;
        if authorized_key != key {
            return Err(AppError::Forbidden(format!(
                "secret authorization snapshot mismatch: env `{env_name}` now resolves a different secret than was authorized"
            )));
        }
        if !super::playbook_secret_gate::principal_secret_use_grant(
            &state.services.access_control,
            principal,
            *secret_id,
            authorized_key,
        )
        .await?
        {
            return Err(AppError::Forbidden(format!(
                "secret access revoked for the run initiator on `{authorized_key}` (env {env_name})"
            )));
        }
        let variable = state
            .repositories
            .secrets
            .get_variable_by_id(*secret_id)
            .await?
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "runtime secret `{authorized_key}` (env {env_name}) does not exist"
                ))
            })?;
        if variable.is_protected {
            return Err(AppError::Forbidden(format!(
                "runtime secret `{authorized_key}` (env {env_name}) is protected and cannot be exported"
            )));
        }
        let value = state.services.secret_store.get_secret(*secret_id).await?;
        env.insert(env_name.clone(), value);
    }
    Ok(env)
}

/// Background worker that fires due playbook schedules: leases each due schedule,
/// creates its scheduled run group (the SQL enforces concurrency policy),
/// advances `next_run_at` via the cron crate + misfire policy, and releases the
/// lease. Backend mode only.
pub(super) async fn run_playbook_schedule_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat(metrics::Worker::PlaybookSchedule);
    let runtime = &state.settings.playbook_runtime;
    let poll_interval_ms = runtime.schedule_poll_interval_ms;
    let lease_owner = format!("playbook-schedule-worker-{}", Uuid::now_v7());
    let mut backoff =
        WorkerBackoff::starting_immediately(poll_interval_ms, WorkerRetryConfig::POLLER);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff.next_sleep_ms())) => {
                metrics::record_worker_heartbeat(metrics::Worker::PlaybookSchedule);
                let mut tick = metrics::start_worker_tick(metrics::Worker::PlaybookSchedule);
                match run_playbook_schedule_worker_tick(&state, &lease_owner).await {
                    TickOutcome::Healthy => {
                        tick.record(true);
                        log_worker_recovery(
                            &backoff,
                            "playbooks.schedule_worker.recovered",
                            "playbook_schedule_worker_tick",
                        );
                        backoff.on_success();
                    }
                    TickOutcome::DependencyFailed => {
                        tick.record(false);
                        backoff.on_failure();
                    }
                }
            }
        }
    }
}

async fn refresh_playbook_run_metrics(state: &AppState) -> Result<(), AppError> {
    let rows = sqlx::query(
        r#"
        SELECT
            status,
            COUNT(*)::bigint AS run_count,
            CASE
                WHEN status = 'queued'
                THEN EXTRACT(EPOCH FROM ($1 - MIN(queued_at)))::double precision
                ELSE NULL
            END AS oldest_queued_age_seconds
        FROM playbook_run
        GROUP BY status
        "#,
    )
    .bind(Utc::now())
    .fetch_all(&state.db_pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to load playbook run metrics: {error}")))?;

    let mut counts = Vec::with_capacity(rows.len());
    let mut oldest_queued_age_seconds = 0.0;
    for row in &rows {
        let status = row.try_get::<String, _>("status").map_err(|error| {
            AppError::Database(format!("Invalid playbook status metric: {error}"))
        })?;
        let count = row.try_get::<i64, _>("run_count").map_err(|error| {
            AppError::Database(format!("Invalid playbook count metric: {error}"))
        })?;
        if status == PlaybookRunStatus::Queued.as_str() {
            oldest_queued_age_seconds = row
                .try_get::<Option<f64>, _>("oldest_queued_age_seconds")
                .map_err(|error| {
                    AppError::Database(format!("Invalid playbook queue age metric: {error}"))
                })?
                .unwrap_or(0.0);
        }
        counts.push((status, count));
    }
    let count_refs = counts
        .iter()
        .map(|(status, count)| (status.as_str(), *count))
        .collect::<Vec<_>>();
    metrics::set_playbook_run_health(&count_refs, oldest_queued_age_seconds);
    Ok(())
}

async fn run_playbook_schedule_worker_tick(state: &AppState, lease_owner: &str) -> TickOutcome {
    let runtime = &state.settings.playbook_runtime;
    let due = match state
        .services
        .playbooks
        .list_due_schedules(Some(runtime.schedule_lease_limit))
        .await
    {
        Ok(due) => due,
        Err(error) => {
            log_error_event(
                "playbooks.schedule_worker.list_due_failed",
                "playbook_schedule_worker_list_due_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_schedule_worker_list_due")
                    .with_status("failed"),
            );
            // The schedule store is the dependency this tick is built on: if the
            // due-list query fails we learned nothing, so the loop must back off
            // rather than spin at the healthy interval against a sick database.
            return TickOutcome::DependencyFailed;
        }
    };

    let mut advances = Vec::with_capacity(due.len());
    for due_schedule in due {
        let schedule = due_schedule.schedule;
        let Some(scheduled_for) = schedule.next_run_at else {
            continue;
        };
        let next_run_at = match compute_next_run_at(
            &schedule.cron_expression,
            &schedule.timezone,
            due_schedule.database_now,
            schedule.misfire_policy,
        ) {
            Ok(next_run_at) => next_run_at,
            Err(error) => {
                log_error_event(
                    "playbooks.schedule_worker.cron_failed",
                    "playbook_schedule_worker_cron_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("playbook_schedule_worker_cron")
                        .with_status("failed")
                        .with_resource("playbook_schedule", schedule.id.to_string()),
                );
                continue;
            }
        };
        if next_run_at.is_none() {
            log_event(
                LogLevel::Warning,
                "playbooks.schedule_worker.schedule_exhausted",
                EventFields::default()
                    .with_operation("playbook_schedule_worker_cron")
                    .with_status("disabled")
                    .with_resource("playbook_schedule", schedule.id.to_string())
                    .with_context("cron_expression", schedule.cron_expression)
                    .with_context("timezone", schedule.timezone)
                    .with_context(
                        "reason",
                        "cron has no future occurrence; schedule will be disabled".to_string(),
                    ),
            );
        }
        advances.push(ScheduleOccurrenceAdvance {
            schedule_id: schedule.id,
            scheduled_for,
            next_run_at,
        });
    }

    let leased = match state
        .services
        .playbooks
        .lease_schedule_occurrences(LeaseDueSchedulesInput {
            lease_owner: lease_owner.to_string(),
            limit: Some(runtime.schedule_lease_limit),
            lease_ttl_seconds: Some(runtime.schedule_lease_ttl_seconds),
            advances,
        })
        .await
    {
        Ok(leased) => leased,
        Err(error) => {
            log_worker_failure(
                "playbooks.schedule_worker.lease_failed",
                "playbook_schedule_worker_lease_error",
                error.to_string(),
                "playbook_schedule_worker_lease",
                None,
            );
            return TickOutcome::DependencyFailed;
        }
    };

    for occurrence in leased.occurrences {
        fire_due_schedule(state, lease_owner, occurrence).await;
    }

    TickOutcome::Healthy
}

async fn fire_due_schedule(
    state: &AppState,
    lease_owner: &str,
    occurrence: crate::contexts::playbooks::application::LeasedPlaybookScheduleOccurrence,
) {
    let schedule = occurrence.schedule;
    let schedule_id = schedule.id;
    let scheduled_for = occurrence.scheduled_for;

    let create_result = state
        .services
        .playbooks
        .create_scheduled_run_group(schedule_id, scheduled_for)
        .await;

    match create_result {
        Ok(created) => {
            record_worker_group_audit(state, "playbook.schedule.fired", &created.run_group).await;
        }
        Err(error) => {
            record_schedule_launch_failure(state, schedule_id).await;
            log_error_event(
                "playbooks.schedule_worker.fire_failed",
                "playbook_schedule_worker_fire_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_schedule_worker_fire")
                    .with_status("failed")
                    .with_resource("playbook_schedule", schedule_id.to_string())
                    .with_context("scheduled_for", scheduled_for.to_rfc3339())
                    .with_context("fired_at", occurrence.fired_at.to_rfc3339()),
            );
            if schedule_authority_lost(&error) {
                // The owner lost launch authority: stop the schedule so the next
                // tick cannot retry the same occurrence forever.
                match state
                    .services
                    .playbooks
                    .stop_schedule_for_owner_loss(schedule_id)
                    .await
                {
                    Ok(true) => record_schedule_owner_revoked(state, schedule_id).await,
                    Ok(false) => {}
                    Err(stop_error) => {
                        log_error_event(
                            "playbooks.schedule_worker.owner_stop_failed",
                            "playbook_schedule_worker_owner_stop_error",
                            stop_error.to_string(),
                            EventFields::default()
                                .with_operation("playbook_schedule_worker_owner_stop")
                                .with_status("failed")
                                .with_resource("playbook_schedule", schedule_id.to_string()),
                        );
                    }
                }
            }
            if let Err(release_error) = state
                .services
                .playbooks
                .release_schedule_occurrence(ReleaseScheduleOccurrenceInput {
                    schedule_id,
                    scheduled_for,
                    lease_owner: lease_owner.to_string(),
                })
                .await
            {
                log_error_event(
                    "playbooks.schedule_worker.release_occurrence_failed",
                    "playbook_schedule_worker_release_occurrence_error",
                    release_error.to_string(),
                    EventFields::default()
                        .with_operation("playbook_schedule_worker_release_occurrence")
                        .with_status("failed")
                        .with_resource("playbook_schedule", schedule_id.to_string())
                        .with_context("scheduled_for", scheduled_for.to_rfc3339()),
                );
            }
        }
    }
}

/// Compute the next run instant for a cron expression in the schedule's IANA
/// timezone, after `after` (UTC). Returns `None` if the cron has no future
/// occurrence.
///
/// `saffron` operates on the naive wall-clock fields of a `DateTime<Utc>`, so we
/// bridge the timezone by (1) projecting `after` into the schedule tz, (2) asking
/// saffron for the next wall-clock occurrence, then (3) reinterpreting that
/// wall-clock back in the schedule tz to a real UTC instant.
fn compute_next_run_at(
    cron_expression: &str,
    timezone: &str,
    after: DateTime<Utc>,
    misfire_policy: PlaybookScheduleMisfirePolicy,
) -> Result<Option<DateTime<Utc>>, AppError> {
    let cron = Cron::from_str(cron_expression).map_err(|error| {
        AppError::Validation(format!("Invalid schedule cron expression: {error}"))
    })?;
    let tz = chrono_tz::Tz::from_str(timezone)
        .map_err(|error| AppError::Validation(format!("Invalid schedule timezone: {error}")))?;

    // Project `after` into the schedule's wall clock, then treat those naive
    // fields as a UTC instant saffron can advance.
    let local_after = after.with_timezone(&tz).naive_local();
    let saffron_input = Utc.from_utc_datetime(&local_after);

    let Some(next_naive_wall) = cron.next_after(saffron_input).map(|dt| dt.naive_utc()) else {
        return Ok(None);
    };

    // Reinterpret the wall-clock occurrence in the schedule tz back to UTC,
    // resolving DST gaps/overlaps deterministically to the earliest instant.
    // A spring-forward gap (e.g. 02:00-03:00 in America/New_York) makes
    // `from_local_datetime` return `None` for every wall-clock instant inside
    // the lost hour, so a single retry can still land in the gap (e.g. a
    // `*/30` schedule yields 02:00 then 02:30, both lost). Keep advancing the
    // cron until we resolve a real instant. The cap (a spring-forward gap is at
    // most one hour = 60 per-minute ticks) bounds the loop with comfortable
    // margin and guarantees termination even on pathological expressions.
    const MAX_GAP_ADVANCES: u32 = 1500;
    let mut candidate_naive = next_naive_wall;
    let mut next_utc = None;
    for _ in 0..MAX_GAP_ADVANCES {
        match tz.from_local_datetime(&candidate_naive) {
            chrono::LocalResult::Single(dt) => {
                next_utc = Some(dt.with_timezone(&Utc));
                break;
            }
            chrono::LocalResult::Ambiguous(earliest, _latest) => {
                next_utc = Some(earliest.with_timezone(&Utc));
                break;
            }
            chrono::LocalResult::None => {
                // Inside a spring-forward gap: advance to the next cron
                // occurrence and re-resolve. `next_after` returning None means
                // the cron is genuinely exhausted.
                let Some(advanced) = cron
                    .next_after(Utc.from_utc_datetime(&candidate_naive))
                    .map(|dt| dt.naive_utc())
                else {
                    return Ok(None);
                };
                candidate_naive = advanced;
            }
        }
    }
    let Some(next_utc) = next_utc else {
        return Ok(None);
    };

    // First release: only FireOnce exists; it advances to the next future
    // occurrence after `after`. The enum is matched explicitly so adding a real
    // SkipMissed/CatchUp later is a compile error here until it has behavior.
    match misfire_policy {
        PlaybookScheduleMisfirePolicy::FireOnce => Ok(Some(next_utc)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DispatchFailureAction, PLAYBOOK_DISPATCH_MAX_ATTEMPTS,
        PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS, allocate_run_inventory_alias,
        allocate_target_alias, compute_next_run_at, declared_secret_env, dispatch_failure_action,
        dispatch_retry_delay_seconds, parse_secret_env_authorization, pinned_image,
        piping_host_vars, schedule_launch_failed_event, worker_group_audit_event,
    };
    use crate::contexts::playbooks::domain::{
        InventorySelection, LaunchOptions, LaunchSpec, PlaybookRun, PlaybookRunGroup,
        PlaybookRunStatus, PlaybookRunTriggerKind, PlaybookScheduleMisfirePolicy,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use uuid::Uuid;

    #[test]
    fn manual_launch_initiator_must_exist_and_be_active() {
        let id = Uuid::now_v7();
        let mut user = crate::contexts::access_control::domain::User {
            id,
            username: "operator".into(),
            email: "operator@example.test".into(),
            status: "active".into(),
            auth_source: "local".into(),
            external_id: None,
        };
        assert!(super::manual_launch_principal(id, Some(&user)).is_ok());
        user.status = "inactive".into();
        assert!(super::manual_launch_principal(id, Some(&user)).is_err());
        assert!(super::manual_launch_principal(id, None).is_err());
    }

    #[test]
    fn schedule_launch_failure_has_no_unproven_identity_or_state() {
        use crate::contexts::audit::domain::{AuditEmitterKind, AuditOutcome};

        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let schedule_id = Uuid::from_u128(10);
        let event = schedule_launch_failed_event(schedule_id, now).unwrap();
        let context = event.audit_context();
        assert_eq!(event.event_type, "playbook.schedule.launch_failed");
        assert_eq!(event.actor_kind, "system");
        assert_eq!(event.actor_id, None);
        assert_eq!(event.target_type.as_deref(), Some("playbook_schedule"));
        assert_eq!(event.target_id, Some(schedule_id));
        assert_eq!(context.outcome, AuditOutcome::Failed);
        assert_eq!(context.decision, None);
        assert_eq!(context.state, None);
        assert_eq!(context.delegated_principal, None);
        assert_eq!(context.emitter.unwrap().kind, AuditEmitterKind::Worker);
        assert_eq!(context.correlation, Default::default());
        assert_eq!(event.metadata.as_object().unwrap().len(), 2);
        assert_eq!(event.metadata["_omitted"], false);
    }

    #[test]
    fn schedule_authority_loss_is_permanent_but_storage_errors_retry() {
        use crate::contexts::audit::domain::{AuditDecision, AuditEmitterKind, AuditOutcome};
        use crate::errors::AppError;

        assert!(super::schedule_authority_lost(&AppError::Forbidden(
            "Schedule owner is not active".into()
        )));
        assert!(!super::schedule_authority_lost(&AppError::Database(
            "connection refused".into()
        )));
        assert!(!super::schedule_authority_lost(&AppError::Validation(
            "bad cron".into()
        )));

        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let schedule_id = Uuid::from_u128(11);
        let event = super::schedule_owner_revoked_event(schedule_id, now).unwrap();
        assert_eq!(event.event_type, "playbook.schedule.owner_access_revoked");
        assert_eq!(event.actor_kind, "system");
        assert_eq!(event.actor_id, None);
        assert_eq!(event.target_type.as_deref(), Some("playbook_schedule"));
        assert_eq!(event.target_id, Some(schedule_id));
        let context = event.audit_context();
        assert_eq!(context.outcome, AuditOutcome::Denied);
        assert_eq!(context.decision, Some(AuditDecision::Denied));
        assert_eq!(context.emitter.unwrap().kind, AuditEmitterKind::Worker);
    }

    #[test]
    fn worker_events_describe_accepted_operation_and_persisted_state() {
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let group_id = Uuid::from_u128(1);
        let schedule_id = Uuid::from_u128(2);
        let principal_id = Uuid::from_u128(3);
        let mut group = PlaybookRunGroup {
            id: group_id,
            template_id: Uuid::from_u128(4),
            launch_spec: LaunchSpec {
                template_id: Uuid::from_u128(4),
                project_id: Uuid::from_u128(5),
                revision_id: Uuid::from_u128(6),
                tree_sha256: "0".repeat(64),
                execution_environment_id: Uuid::from_u128(7),
                image: "image".into(),
                image_digest: format!("sha256:{}", "0".repeat(64)),
                workdir: ".".into(),
                playbook_paths: vec!["site.yml".into()],
                inventory: InventorySelection::Native {
                    sources: vec!["inventory.yml".into()],
                    ssh_record_id: None,
                },
                vars_schema: None,
                options: LaunchOptions {
                    argv: Vec::new(),
                    extra_vars: json!({}),
                    limit: None,
                    env: BTreeMap::new(),
                    secret_refs: BTreeMap::new(),
                    timeout_seconds: 60,
                },
                targets: Vec::new(),
            },
            target_set_id: None,
            schedule_id: Some(schedule_id),
            scheduled_for: Some(now),
            requested_by_user_id: None,
            service_principal_id: Some(principal_id),
            trigger_kind: PlaybookRunTriggerKind::Schedule,
            status: PlaybookRunStatus::Queued,
            total_runs: 0,
            succeeded_runs: 0,
            failed_runs: 0,
            canceled_runs: 0,
            timeout_runs: 0,
            unreported_runs: 0,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            lease_owner: None,
            lease_expires_at: None,
            leased_at: None,
            dispatch_id: None,
            dispatch_attempt: 0,
            cancel_requested_at: None,
            cancel_deadline_at: None,
            cancel_outcome: None,
            metadata: json!({}),
        };
        let fired = worker_group_audit_event("playbook.schedule.fired", &group, now).unwrap();
        assert_eq!(fired.target_id, Some(schedule_id));
        assert_eq!(fired.target_type.as_deref(), Some("playbook_schedule"));
        assert_eq!(fired.actor_id, Some(principal_id));
        assert_eq!(fired.metadata["audit_context"]["outcome"], "succeeded");
        assert_eq!(fired.metadata["audit_context"]["state"]["value"], "queued");
        assert_eq!(fired.metadata["audit_context"]["emitter"]["kind"], "worker");
        assert_eq!(
            fired.metadata["audit_context"]["correlation"]["run_group_id"],
            group_id.to_string()
        );
        group.status = PlaybookRunStatus::Running;
        let dispatched =
            worker_group_audit_event("playbook.run_group.dispatched", &group, now).unwrap();
        assert_eq!(dispatched.metadata["audit_context"]["outcome"], "succeeded");
        assert_eq!(
            dispatched.metadata["audit_context"]["state"]["value"],
            "running"
        );
        group.status = PlaybookRunStatus::Leased;
        group.requested_by_user_id = Some(Uuid::from_u128(8));
        let retry =
            worker_group_audit_event("playbook.run_group.retry_scheduled", &group, now).unwrap();
        assert_eq!(retry.actor_id, group.requested_by_user_id);
        assert_eq!(retry.metadata["audit_context"]["outcome"], "succeeded");
        assert_eq!(retry.metadata["audit_context"]["state"]["value"], "leased");
        group.schedule_id = None;
        assert!(worker_group_audit_event("playbook.schedule.fired", &group, now).is_err());
    }

    fn run_with_metadata(metadata: serde_json::Value) -> PlaybookRun {
        PlaybookRun {
            id: Uuid::from_u128(1),
            run_group_id: Uuid::from_u128(2),
            hostname: "host-1".to_string(),
            target_kind: Some("connection_target".to_string()),
            target_id: Some(Uuid::from_u128(5)),
            source_kind: Some("device".to_string()),
            source_id: Some(Uuid::from_u128(5)),
            target_name: Some("host-1".to_string()),
            target_address: None,
            target_address_bound: false,
            credential_account_kind: "auto".to_string(),
            credential_account_id: None,
            credential_account_entry_id: None,
            credential_merge_policy: "context_first".to_string(),
            credential_resolution: json!({}),
            status: PlaybookRunStatus::Leased,
            cancel_requested_at: None,
            cancel_outcome: None,
            lease_owner: Some("worker".to_string()),
            lease_expires_at: None,
            queued_at: Utc::now(),
            leased_at: None,
            started_at: None,
            finished_at: None,
            exit_code: None,
            error_message: None,
            attempt: 0,
            timeout_seconds: 1800,
            metadata,
        }
    }

    #[test]
    fn parse_secret_env_authorization_reads_id_and_key_pairs_only() {
        let snapshot = json!({
            "secret_env_authorization": {
                "decided_at": "2026-07-10T00:00:00Z",
                "subject": "user:00000000-0000-7000-8000-000000000009",
                "env": {
                    "GRAD_TOKEN": {
                        "secret_id": "01890000-0000-7000-8000-000000000001",
                        "key": "grad-token"
                    },
                    "MALFORMED": { "key": "no-id" }
                }
            }
        });
        let parsed = parse_secret_env_authorization(&snapshot);
        assert_eq!(parsed.len(), 1);
        let (id, key) = parsed.get("GRAD_TOKEN").expect("authorized env");
        assert_eq!(key, "grad-token");
        assert_eq!(id.to_string(), "01890000-0000-7000-8000-000000000001");
        assert!(!parsed.contains_key("MALFORMED"));
    }

    #[test]
    fn declared_secret_env_requires_the_authorized_secret_id() {
        let secret_id = Uuid::from_u128(0xaa);
        let authorized =
            BTreeMap::from([("TOKEN".to_string(), (secret_id, "token-key".to_string()))]);

        let declared = declared_secret_env(
            &BTreeMap::from([("TOKEN".to_string(), secret_id)]),
            &authorized,
        )
        .expect("matching reference");
        assert_eq!(declared.get("TOKEN").map(String::as_str), Some("token-key"));

        assert!(
            declared_secret_env(
                &BTreeMap::from([("TOKEN".to_string(), Uuid::from_u128(0xbb))]),
                &authorized,
            )
            .is_err(),
            "a reference that now points at another secret must fail closed"
        );
        assert!(
            declared_secret_env(
                &BTreeMap::from([("MISSING".to_string(), secret_id)]),
                &authorized,
            )
            .is_err(),
            "a declared env absent from the authorization snapshot must fail closed"
        );
    }

    #[test]
    fn dispatch_retry_delay_scales_linearly_with_attempt() {
        assert_eq!(
            dispatch_retry_delay_seconds(1),
            PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS
        );
        assert_eq!(
            dispatch_retry_delay_seconds(2),
            PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS * 2
        );
        assert_eq!(
            dispatch_retry_delay_seconds(0),
            PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS
        );
    }

    #[test]
    fn dispatch_failure_retries_before_budget_and_fails_at_budget() {
        assert_eq!(
            dispatch_failure_action(PLAYBOOK_DISPATCH_MAX_ATTEMPTS - 1),
            DispatchFailureAction::Requeue {
                retry_delay_seconds: PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS * 2,
            }
        );
        assert_eq!(
            dispatch_failure_action(PLAYBOOK_DISPATCH_MAX_ATTEMPTS),
            DispatchFailureAction::Fail
        );
        assert_eq!(
            dispatch_failure_action(PLAYBOOK_DISPATCH_MAX_ATTEMPTS + 1),
            DispatchFailureAction::Fail
        );
    }

    #[test]
    fn pinned_image_requires_repository_and_sha256_digest() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let (image, parsed) = pinned_image("registry.example/ansible", &digest).unwrap();
        assert_eq!(image, "registry.example/ansible");
        assert_eq!(parsed, digest);

        assert!(
            pinned_image("", &digest).is_err(),
            "an empty repository is not an image reference"
        );
        assert!(
            pinned_image("registry.example/ansible", "sha256:short").is_err(),
            "a short digest is not a content digest"
        );
    }

    #[test]
    fn piping_host_vars_carry_only_the_run_identity_and_target() {
        let run = run_with_metadata(json!({}));
        let vars = piping_host_vars(&run);
        assert_eq!(vars.get("piping_run_id"), Some(&run.id.to_string()));
        assert_eq!(
            vars.get("piping_target_kind").map(String::as_str),
            Some("connection_target")
        );
        assert_eq!(
            vars.get("piping_target_name").map(String::as_str),
            Some("host-1")
        );
        assert!(
            !vars.contains_key("piping_api_token"),
            "supervisor callbacks are not launch environment values"
        );
    }

    #[test]
    fn run_inventory_alias_preserves_unique_hostname_and_suffixes_collisions() {
        let mut first = run_with_metadata(json!({}));
        let mut second = run_with_metadata(json!({}));
        first.id = Uuid::parse_str("019f3e36-aef1-7f4f-b3b4-000000000001").unwrap();
        second.id = Uuid::parse_str("019f3e36-aef1-7f4f-b3b4-000000000002").unwrap();
        first.hostname = "cash-1".to_string();
        second.hostname = "cash-1".to_string();

        let mut allocated = BTreeSet::new();
        let first_alias = allocate_run_inventory_alias(&first, &mut allocated);
        let second_alias = allocate_run_inventory_alias(&second, &mut allocated);

        assert_eq!(first_alias, "cash-1");
        assert_eq!(second_alias, "cash-1_b3b4000000000002");
    }

    #[test]
    fn target_alias_allocator_keeps_native_targets_distinct() {
        let mut allocated = BTreeSet::new();
        assert_eq!(allocate_target_alias("web 1", &mut allocated), "web_1");
        assert_eq!(allocate_target_alias("web 1", &mut allocated), "web_1_2");
        assert_eq!(allocate_target_alias("web 1", &mut allocated), "web_1_3");
    }

    #[test]
    fn cron_advances_in_the_schedule_timezone() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let next = compute_next_run_at(
            "0 3 * * *",
            "UTC",
            after,
            PlaybookScheduleMisfirePolicy::FireOnce,
        )
        .expect("valid cron")
        .expect("future occurrence");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 3, 0, 0).unwrap());
    }
}
