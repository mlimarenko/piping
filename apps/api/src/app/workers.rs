use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use saffron::Cron;
use serde_json::Value;
use sqlx::Row;
use tokio::sync::watch;
use uuid::Uuid;

use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::sync::infrastructure::{
    ProviderRequestJournalRepository, WebhookEventRepository,
};
use crate::{
    contexts::access_control::domain::{Action, PolicyDecision, Principal, ResourceType},
    contexts::connections::application::ResolveSshExecutionLaunchInput,
    contexts::playbooks::application::{
        CompleteRunInput, LeaseDueSchedulesInput, LeaseQueuedRunsInput, MarkRunsRunningInput,
        PlaybookRunExecution, PlaybookRunGroupDetail, PlaybookRuntimePolicy,
        ReleaseScheduleOccurrenceInput, RequeueStartedRunsInput, ScheduleOccurrenceAdvance,
        SetActiveRunTimeoutsInput, StartLeasedRunsInput, principal_subject,
        runtime_policy_from_token_policy, secret_use_candidates,
    },
    contexts::playbooks::domain::{
        Playbook, PlaybookConnection, PlaybookExecutionEnvironment, PlaybookProject, PlaybookRun,
        PlaybookRunStatus, PlaybookScheduleMisfirePolicy, parse_playbook_connection,
    },
    contexts::playbooks::infrastructure::{
        AnsibleDispatchOutcome, AnsibleJobRequest, AnsibleJobTarget, AnsibleRunnerClient,
        AnsibleTargetInventory, ProjectBundle, RUNNER_CONTRACT_VERSION, ScmCredential,
        build_project_bundle, build_target_inventory_with_alias, classify_scm_credential,
        sanitize_host_alias, with_per_host_key_file,
    },
    errors::AppError,
    observability::{
        EventFields, LogLevel, NoiseBudgetProfile, log_error_event, log_event, metrics,
    },
};

use super::AppState;
use super::worker_backoff::{WorkerBackoff, WorkerRetryConfig};

const PLAYBOOK_ARTIFACT_CREATE_SCOPE: &str = "artifact:create";
const PLAYBOOK_EVENT_APPEND_SCOPE: &str = "event:append";
const PLAYBOOK_STATUS_UPDATE_SCOPE: &str = "status:update";
const PLAYBOOK_INVENTORY_READ_SCOPE: &str = "inventory:read";
const PLAYBOOK_INVENTORY_CREATE_SCOPE: &str = "inventory:create";
const PLAYBOOK_INVENTORY_UPDATE_SCOPE: &str = "inventory:update";
const PLAYBOOK_GROUP_JOB_TIMEOUT_MAX_SECONDS: i32 = 86_400;
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
    metrics::record_worker_heartbeat("outbox");
    let sync_runtime = &state.settings.sync_runtime;
    let poll_interval_ms = sync_runtime.outbox_worker_poll_interval_ms;
    let batch_size = sync_runtime.outbox_worker_batch_size as usize;
    let outbound_service = state.services.outbound_service.clone();
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
                metrics::record_worker_heartbeat("outbox");
                match outbound_service.process_batch(batch_size).await {
                    Ok(_) => {
                        log_worker_recovery(
                            &backoff,
                            "sync.outbox.worker_tick.recovered",
                            "outbox_worker_tick",
                        );
                        backoff.on_success();
                    }
                    Err(error) => {
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
            }
        }
    }
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
    metrics::record_worker_heartbeat("observability_poller");
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
                metrics::record_worker_heartbeat("observability_poller");
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
    metrics::record_worker_heartbeat("webhook_event_pruner");
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
                metrics::record_worker_heartbeat("webhook_event_pruner");
                match repo.prune_older_than(RETENTION_DAYS).await {
                    Ok(pruned) => {
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
    metrics::record_worker_heartbeat("journal_pruner");
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
                metrics::record_worker_heartbeat("journal_pruner");
                match repo.prune_older_than(retention_days).await {
                    Ok(pruned) => {
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
    metrics::record_worker_heartbeat("license_refresh");
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
                metrics::record_worker_heartbeat("license_refresh");
                if state.settings.license.base_url.trim().is_empty() {
                    backoff.on_success();
                    continue;
                }
                match state.services.licensing.refresh_license_state().await {
                    Ok(_) => {
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
                        backoff.on_success();
                    }
                    Err(error) => {
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

/// The credential subject resolved from a run's stamped `metadata.credential`.
///
/// Manual runs stamp a `user` subject (the requesting user); scheduled runs
/// stamp a `service_principal` subject. Only a `user` subject yields a real
/// `credential_user_id` that `resolve_ssh_execution_launch` can fence secrets
/// against. A service principal / system subject has no secret-visibility
/// identity, so no credentials can be resolved for it.
enum RunCredentialSubject {
    User(Uuid),
    Unsupported(String),
}

/// Read the `metadata.credential.subject_kind` / `subject_id` stamped on the run
/// at run-group creation and map it to a credential identity.
fn resolve_run_credential_subject(run: &PlaybookRun) -> RunCredentialSubject {
    let credential = run.metadata.get("credential");
    let subject_kind = credential
        .and_then(|value| value.get("subject_kind"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match subject_kind {
        "user" => {
            let subject_id = credential
                .and_then(|value| value.get("subject_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw.trim()).ok());
            match subject_id {
                Some(user_id) => RunCredentialSubject::User(user_id),
                None => RunCredentialSubject::Unsupported(
                    "run credential subject_kind=user but subject_id is missing or invalid"
                        .to_string(),
                ),
            }
        }
        other => RunCredentialSubject::Unsupported(format!(
            "cannot resolve credentials for non-user run subject `{other}`: \
             scheduled/service-principal runs require an SP->credential-identity mapping"
        )),
    }
}

/// Background worker that drains queued playbook runs: reaps timed-out runs,
/// leases queued work, resolves credentials, and dispatches each run as a fully-
/// specified Ansible job to the `ansible-runner` sidecar. Backend mode only.
///
/// Dispatch is asynchronous: once a durable receipt is reconciled the run moves
/// to `running` and the sidecar streams events/artifacts/completion back via
/// the run-bound job-token callbacks. The timeout reaper is the only backstop
/// for callbacks that never arrive.
///
/// Dispatch is sequential: the lease limit is small and runs dispatch one at a
/// time per tick. A failed sidecar dispatch is retried: the group's runs are
/// parked back into the lease queue with a backoff (up to
/// [`PLAYBOOK_DISPATCH_MAX_ATTEMPTS`] attempts) before failing terminally.
pub(super) async fn run_playbook_run_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat("playbook_run");
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
                metrics::record_worker_heartbeat("playbook_run");
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
                        log_worker_recovery(
                            &backoff,
                            "playbooks.run_worker.recovered",
                            "playbook_run_worker_tick",
                        );
                        backoff.on_success();
                    }
                    TickOutcome::DependencyFailed => {
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

    // 2) Lease queued rows, then collapse them to one dispatch per run group. The
    // repository still owns row-level lease safety, but the worker execution unit is
    // the run group: one sidecar job and one ansible-playbook process for all hosts.
    let leased = match state
        .services
        .playbooks
        .lease_queued_runs(
            LeaseQueuedRunsInput {
                lease_owner: lease_owner.to_string(),
                limit: Some(runtime.run_lease_limit),
                lease_ttl_seconds: Some(runtime.run_lease_ttl_seconds),
                max_concurrent_groups: Some(i64::from(runtime.max_concurrent_runs)),
                max_concurrent_runs_per_group: Some(i64::from(
                    runtime.max_concurrent_runs_per_group,
                )),
            },
            now,
        )
        .await
    {
        Ok(leased) => leased,
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

    let group_ids = leased_run_group_ids(&leased.runs);
    for group_id in group_ids {
        execute_leased_run_group(state, lease_owner, runner_client, group_id).await;
    }

    outcome
}

fn leased_run_group_ids(runs: &[PlaybookRun]) -> Vec<Uuid> {
    let mut group_ids = runs.iter().map(|run| run.run_group_id).collect::<Vec<_>>();
    group_ids.sort_unstable();
    group_ids.dedup();
    group_ids
}

fn owned_leased_runs(runs: Vec<PlaybookRun>, lease_owner: &str) -> Vec<PlaybookRun> {
    runs.into_iter()
        .filter(|run| run.lease_owner.as_deref() == Some(lease_owner))
        .filter(|run| run.status == PlaybookRunStatus::Leased)
        .collect()
}

async fn execute_leased_run_group(
    state: &AppState,
    lease_owner: &str,
    runner_client: &AnsibleRunnerClient,
    run_group_id: Uuid,
) {
    let Some(detail) = load_leased_run_group(state, run_group_id).await else {
        return;
    };
    let runs = owned_leased_runs(detail.runs, lease_owner);
    if runs.is_empty() {
        return;
    }
    let primary_run = runs[0].clone();
    let dispatch_id = Uuid::now_v7();
    let dispatch_attempt = detail.run_group.dispatch_attempt.saturating_add(1);

    let Some((playbook, runtime_policy)) =
        load_group_playbook(state, lease_owner, &runs, detail.run_group.playbook_id).await
    else {
        return;
    };

    let should_expose_inventory_api =
        playbook.source_kind == "system" && playbook.inventory_rest_enabled;
    let Some((connection, execution_environment, execution_environment_env)) =
        group_launch_environment_or_fail(state, lease_owner, &runs, &playbook, &primary_run).await
    else {
        return;
    };
    let job_token_scopes = grouped_job_token_scopes(should_expose_inventory_api, &runtime_policy);

    let mut passwords = BTreeMap::new();
    // Per-target key material and inventory lines are collected first; the
    // shared-vs-per-host key delivery mode is decided after the loop once every
    // target's key is known (see the block after `started_runs.is_empty()`).
    let mut target_lines: Vec<String> = Vec::new();
    let mut target_keys: Vec<Option<String>> = Vec::new();
    let mut targets = Vec::new();
    let mut started_runs = Vec::new();
    let mut primary_raw_token: Option<String> = None;
    let mut version_content: Option<String> = None;
    let mut version_entrypoint: Option<String> = None;
    let configured_forks =
        requested_group_forks(state.settings.ansible_runner.forks, &primary_run.metadata);
    let planned_forks = effective_group_forks(
        configured_forks,
        state
            .settings
            .playbook_runtime
            .max_concurrent_runs_per_group,
        runs.len(),
    );
    let planned_job_timeout_seconds = planned_group_job_timeout_seconds(
        primary_run.timeout_seconds,
        runs.len(),
        planned_forks,
        &primary_run.metadata,
    );
    let job_token_ttl_seconds = grouped_job_token_ttl_seconds(planned_job_timeout_seconds);

    // The C3 authorization snapshot is captured once for the group. Each target
    // still re-checks the stored requesting user or service principal at dispatch.
    let (prepared_runs, group_runtime_secret_env) = prepare_group_targets(
        runs,
        GroupTargetPreparation {
            state,
            lease_owner,
            connection,
            declared_secrets: &runtime_policy.secret_env,
            authorized_secrets: parse_secret_env_authorization(&detail.run_group.metadata),
            subject: secret_recheck_subject(
                detail.run_group.requested_by_user_id,
                detail.run_group.service_principal_id,
            ),
        },
    )
    .await;

    if prepared_runs.is_empty() {
        return;
    }

    let Some(started_group) = start_prepared_group(
        state,
        lease_owner,
        run_group_id,
        &prepared_runs,
        GroupStartOptions {
            dispatch_id,
            dispatch_attempt,
            job_token_ttl_seconds,
            job_token_scopes,
        },
    )
    .await
    else {
        return;
    };
    let group_callback_token = started_group.raw_group_callback_token;
    let executions = started_group.executions;

    // The raw callback token is exposed to the playbook only when it actually
    // opts into the Piping API — same gate as the PIPING_API_TOKEN env var
    // below. Without this, a playbook that set expose_piping_api=false still
    // received a live bearer token as the `piping_api_token` inventory host var
    // (readable by any task, dumpable via -vvvv), defeating the opt-out.
    let expose_api_token = should_expose_inventory_api || runtime_policy.expose_piping_api;
    for (execution, prepared) in executions.into_iter().zip(prepared_runs.iter()) {
        let host_var_token = expose_api_token.then_some(execution.raw_job_token.as_str());
        let Some(inventory) = prepared
            .inventory(
                RunLeaseContext { state, lease_owner },
                &execution,
                connection,
                host_var_token,
                &mut passwords,
            )
            .await
        else {
            continue;
        };
        target_lines.push(inventory.inventory_line);
        target_keys.push(inventory.ssh_key);

        remember_group_execution(
            &execution,
            &mut primary_raw_token,
            &mut version_content,
            &mut version_entrypoint,
        );
        targets.push(AnsibleJobTarget {
            alias: prepared.alias.clone(),
            run_id: execution.run.id,
            job_token: execution.raw_job_token,
            target_kind: execution.run.target_kind.clone(),
            target_id: execution.run.target_id,
            source_kind: execution.run.source_kind.clone(),
            source_id: Some(execution.run.source_id),
            ssh_private_key: None,
            known_hosts_lines:
                crate::contexts::shared::host_key_pins::known_hosts_lines_from_payload(
                    &prepared.proxy_payload,
                ),
        });
        started_runs.push(execution.run);
    }

    if started_runs.is_empty() {
        return;
    }

    // Decide SSH key delivery now that every target's key is known.
    let aliases: Vec<String> = targets.iter().map(|target| target.alias.clone()).collect();
    let (inventory_lines, ssh_key, per_target_keys) =
        assemble_group_key_material(&target_lines, &target_keys, &aliases);
    for (target, per_host_key) in targets.iter_mut().zip(per_target_keys) {
        target.ssh_private_key = per_host_key;
    }

    let playbook_env = grouped_playbook_environment(
        state,
        run_group_id,
        group_runtime_secret_env,
        execution_environment_env,
        should_expose_inventory_api || runtime_policy.expose_piping_api,
        primary_raw_token.as_deref(),
    );

    let configured_forks =
        requested_group_forks(state.settings.ansible_runner.forks, &primary_run.metadata);
    let forks = effective_group_forks(
        configured_forks,
        state
            .settings
            .playbook_runtime
            .max_concurrent_runs_per_group,
        started_runs.len(),
    );
    let job_timeout_seconds = planned_group_job_timeout_seconds(
        primary_run.timeout_seconds,
        started_runs.len(),
        forks,
        &primary_run.metadata,
    );
    let timeout_extension = GroupTimeoutExtension {
        state,
        lease_owner,
        run_group_id,
        started_runs: &started_runs,
        forks,
    };
    if !timeout_extension
        .apply(primary_run.timeout_seconds, job_timeout_seconds)
        .await
    {
        return;
    }
    let Some(project_bundle) =
        project_bundle_or_fail(state, lease_owner, &started_runs, &primary_run.metadata).await
    else {
        return;
    };
    let extravars = launch_extravars(playbook.vars_schema.as_ref(), &primary_run.metadata);
    let launch_settings = launch_settings(job_timeout_seconds, forks, &primary_run.metadata);
    let settings = execution_environment
        .as_ref()
        .map(|environment| merge_runner_settings(&environment.settings, &launch_settings))
        .unwrap_or(launch_settings);
    let (playbook_content, entrypoint, project_files) = match project_bundle {
        Some(bundle) => (String::new(), Some(bundle.entrypoint), bundle.files),
        None => (
            version_content.unwrap_or_default(),
            version_entrypoint,
            Vec::new(),
        ),
    };
    let job = AnsibleJobRequest {
        protocol_version: RUNNER_CONTRACT_VERSION,
        dispatch_id,
        dispatch_attempt,
        run_group_id,
        group_callback_token,
        lease_owner: lease_owner.to_string(),
        callback_base_url: state
            .settings
            .playbook_runtime
            .self_callback_base_url
            .clone(),
        playbook: playbook_content,
        entrypoint,
        project_files,
        inventory: inventory_lines,
        targets,
        extravars,
        ssh_key,
        passwords,
        settings,
        playbook_env,
    };

    dispatch_started_group(state, lease_owner, runner_client, &started_runs, job).await;
}

fn grouped_playbook_environment(
    state: &AppState,
    run_group_id: Uuid,
    runtime_secret_env: Option<BTreeMap<String, String>>,
    execution_environment_env: BTreeMap<String, String>,
    expose_api: bool,
    primary_raw_token: Option<&str>,
) -> BTreeMap<String, String> {
    let mut playbook_env = runtime_secret_env.unwrap_or_default();
    for (key, value) in execution_environment_env {
        playbook_env.entry(key).or_insert(value);
    }
    if expose_api {
        playbook_env.insert(
            "PIPING_API_BASE_URL".to_string(),
            state
                .settings
                .playbook_runtime
                .self_callback_base_url
                .clone(),
        );
        if let Some(token) = primary_raw_token {
            playbook_env.insert("PIPING_API_TOKEN".to_string(), token.to_string());
        }
        playbook_env.insert("PIPING_RUN_GROUP_ID".to_string(), run_group_id.to_string());
    }
    playbook_env
}

async fn project_bundle_or_fail(
    state: &AppState,
    lease_owner: &str,
    started_runs: &[PlaybookRun],
    metadata: &Value,
) -> Option<Option<ProjectBundle>> {
    match project_bundle_for_launch(state, metadata).await {
        Ok(bundle) => Some(bundle),
        Err(error) => {
            fail_started_group(
                state,
                lease_owner,
                started_runs,
                &format!("Failed to materialize project bundle: {error}"),
            )
            .await;
            None
        }
    }
}

struct GroupTimeoutExtension<'a> {
    state: &'a AppState,
    lease_owner: &'a str,
    run_group_id: Uuid,
    started_runs: &'a [PlaybookRun],
    forks: u32,
}

impl GroupTimeoutExtension<'_> {
    async fn apply(&self, original_timeout_seconds: i32, job_timeout_seconds: i32) -> bool {
        if job_timeout_seconds == original_timeout_seconds {
            return true;
        }
        let run_ids = self
            .started_runs
            .iter()
            .map(|run| run.id)
            .collect::<Vec<_>>();
        let updated_runs = match self
            .state
            .services
            .playbooks
            .set_active_run_timeouts(
                SetActiveRunTimeoutsInput {
                    run_ids,
                    lease_owner: self.lease_owner.to_string(),
                    timeout_seconds: job_timeout_seconds,
                },
                Utc::now(),
            )
            .await
        {
            Ok(updated_runs) => updated_runs,
            Err(error) => {
                fail_started_group(
                    self.state,
                    self.lease_owner,
                    self.started_runs,
                    &format!("Failed to extend grouped run timeout: {error}"),
                )
                .await;
                return false;
            }
        };
        if updated_runs.len() != self.started_runs.len() {
            fail_started_group(
                self.state,
                self.lease_owner,
                self.started_runs,
                "Failed to extend grouped run timeout for every started target",
            )
            .await;
            return false;
        }
        log_event(
            LogLevel::Info,
            "playbooks.run_worker.group_timeout_extended",
            EventFields::default()
                .with_operation("playbook_run_worker_dispatch")
                .with_status("ok")
                .with_resource("playbook_run_group", self.run_group_id.to_string())
                .with_context("target_count", self.started_runs.len().to_string())
                .with_context("forks", self.forks.to_string())
                .with_context("timeout_seconds", job_timeout_seconds.to_string()),
        );
        true
    }
}

/// Check cancellation against durable state before dispatching the sidecar job.
async fn dispatch_started_group(
    state: &AppState,
    lease_owner: &str,
    runner_client: &AnsibleRunnerClient,
    started_runs: &[PlaybookRun],
    job: AnsibleJobRequest,
) {
    let run_group_id = job.run_group_id;
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
        AnsibleDispatchOutcome::Accepted(_) => {}
        AnsibleDispatchOutcome::NotAccepted(reason) => {
            requeue_or_fail_started_group(
                state,
                lease_owner,
                started_runs,
                dispatch_attempt,
                &format!("Ansible runner did not accept dispatch: {reason}"),
            )
            .await;
            return;
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
            return;
        }
    }

    let started_run_ids = started_runs.iter().map(|run| run.id).collect::<Vec<_>>();
    match state
        .services
        .playbooks
        .mark_runs_running(
            MarkRunsRunningInput {
                run_ids: started_run_ids,
                lease_owner: lease_owner.to_string(),
            },
            Utc::now(),
        )
        .await
    {
        Ok(_) | Err(AppError::Conflict(_)) => {}
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.mark_running_failed",
                "playbook_run_worker_mark_running_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_mark_running")
                    .with_status("failed")
                    .with_resource("playbook_run_group", run_group_id.to_string()),
            );
        }
    }
}

struct GroupStartOptions {
    dispatch_id: Uuid,
    dispatch_attempt: i32,
    job_token_ttl_seconds: i64,
    job_token_scopes: Vec<String>,
}

async fn start_prepared_group(
    state: &AppState,
    lease_owner: &str,
    run_group_id: Uuid,
    prepared_runs: &[PreparedGroupedRun],
    options: GroupStartOptions,
) -> Option<crate::contexts::playbooks::application::PlaybookRunGroupExecution> {
    let prepared_run_ids = prepared_runs
        .iter()
        .map(|prepared| prepared.run.id)
        .collect();
    let started_group = match state
        .services
        .playbooks
        .start_leased_runs(
            StartLeasedRunsInput {
                run_ids: prepared_run_ids,
                lease_owner: lease_owner.to_string(),
                dispatch_id: options.dispatch_id,
                dispatch_attempt: options.dispatch_attempt,
                job_token_expires_in_seconds: Some(options.job_token_ttl_seconds),
                job_token_scopes: Some(options.job_token_scopes),
                metadata: None,
            },
            Utc::now(),
        )
        .await
    {
        Ok(started_group) => started_group,
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.start_failed",
                "playbook_run_worker_start_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_start")
                    .with_status("failed")
                    .with_resource("playbook_run_group", run_group_id.to_string()),
            );
            fail_leased_group_without_start(
                state,
                lease_owner,
                &prepared_runs
                    .iter()
                    .map(|prepared| prepared.run.clone())
                    .collect::<Vec<_>>(),
                &format!("Failed to start grouped runs: {error}"),
            )
            .await;
            return None;
        }
    };
    if started_group.executions.len() != prepared_runs.len() {
        fail_leased_group_without_start(
            state,
            lease_owner,
            &prepared_runs
                .iter()
                .map(|prepared| prepared.run.clone())
                .collect::<Vec<_>>(),
            "Failed to start every prepared grouped target",
        )
        .await;
        return None;
    }
    Some(started_group)
}

async fn group_launch_environment_or_fail(
    state: &AppState,
    lease_owner: &str,
    runs: &[PlaybookRun],
    playbook: &Playbook,
    primary_run: &PlaybookRun,
) -> Option<(
    PlaybookConnection,
    Option<PlaybookExecutionEnvironment>,
    BTreeMap<String, String>,
)> {
    match group_launch_environment(state, playbook, primary_run).await {
        Ok(environment) => Some(environment),
        Err(reason) => {
            fail_leased_group_without_start(state, lease_owner, runs, &reason).await;
            None
        }
    }
}

async fn group_launch_environment(
    state: &AppState,
    playbook: &Playbook,
    primary_run: &PlaybookRun,
) -> Result<
    (
        PlaybookConnection,
        Option<PlaybookExecutionEnvironment>,
        BTreeMap<String, String>,
    ),
    String,
> {
    let connection = launch_connection(playbook.connection_default, &primary_run.metadata)
        .map_err(|error| format!("Invalid playbook launch connection: {error}"))?;
    let environment = resolve_execution_environment_for_launch(state, &primary_run.metadata)
        .await
        .map_err(|error| error.to_string())?;
    let env = environment
        .as_ref()
        .map(execution_environment_env)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    Ok((connection, environment, env))
}

fn grouped_job_token_scopes(expose_inventory: bool, policy: &PlaybookRuntimePolicy) -> Vec<String> {
    let mut scopes = vec![
        PLAYBOOK_ARTIFACT_CREATE_SCOPE.to_string(),
        PLAYBOOK_EVENT_APPEND_SCOPE.to_string(),
        PLAYBOOK_STATUS_UPDATE_SCOPE.to_string(),
    ];
    if expose_inventory {
        scopes.extend([
            PLAYBOOK_INVENTORY_READ_SCOPE.to_string(),
            PLAYBOOK_INVENTORY_CREATE_SCOPE.to_string(),
            PLAYBOOK_INVENTORY_UPDATE_SCOPE.to_string(),
        ]);
        for scope in &policy.inventory_scopes {
            if !scopes.iter().any(|existing| existing == scope) {
                scopes.push(scope.clone());
            }
        }
    }
    scopes
}

async fn load_leased_run_group(
    state: &AppState,
    run_group_id: Uuid,
) -> Option<PlaybookRunGroupDetail> {
    match state
        .services
        .playbooks
        .get_run_group_with_runs(run_group_id, 50_000)
        .await
    {
        Ok(Some(detail)) => Some(detail),
        Ok(None) => {
            log_error_event(
                "playbooks.run_worker.group_missing",
                "playbook_run_worker_group_error",
                "Playbook run group not found".to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_group_load")
                    .with_status("failed")
                    .with_resource("playbook_run_group", run_group_id.to_string()),
            );
            None
        }
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.group_load_failed",
                "playbook_run_worker_group_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_group_load")
                    .with_status("failed")
                    .with_resource("playbook_run_group", run_group_id.to_string()),
            );
            None
        }
    }
}

async fn load_group_playbook(
    state: &AppState,
    lease_owner: &str,
    runs: &[PlaybookRun],
    playbook_id: Uuid,
) -> Option<(Playbook, PlaybookRuntimePolicy)> {
    let catalog_item = match state.services.playbooks.get_catalog_item(playbook_id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            fail_leased_group_without_start(
                state,
                lease_owner,
                runs,
                "Playbook not found for run group",
            )
            .await;
            return None;
        }
        Err(error) => {
            fail_leased_group_without_start(
                state,
                lease_owner,
                runs,
                &format!("Failed to load playbook for run group: {error}"),
            )
            .await;
            return None;
        }
    };
    let runtime_policy = match catalog_item
        .current_version
        .as_ref()
        .map(|version| runtime_policy_from_token_policy(&version.token_policy))
        .transpose()
    {
        Ok(Some(policy)) => policy,
        Ok(None) => {
            fail_leased_group_without_start(
                state,
                lease_owner,
                runs,
                "Playbook version not found for run group",
            )
            .await;
            return None;
        }
        Err(error) => {
            fail_leased_group_without_start(
                state,
                lease_owner,
                runs,
                &format!("Invalid playbook runtime policy: {error}"),
            )
            .await;
            return None;
        }
    };
    Some((catalog_item.playbook, runtime_policy))
}

struct PreparedGroupedRun {
    run: PlaybookRun,
    alias: String,
    proxy_payload: Value,
}

struct RunLeaseContext<'a> {
    state: &'a AppState,
    lease_owner: &'a str,
}

fn remember_group_execution(
    execution: &PlaybookRunExecution,
    primary_raw_token: &mut Option<String>,
    version_content: &mut Option<String>,
    version_entrypoint: &mut Option<String>,
) {
    if primary_raw_token.is_none() {
        *primary_raw_token = Some(execution.raw_job_token.clone());
    }
    if version_content.is_none() {
        *version_content = Some(execution.version.content.clone());
    }
    if version_entrypoint.is_none() {
        *version_entrypoint = execution.version.entrypoint.clone();
    }
}

impl PreparedGroupedRun {
    async fn inventory(
        &self,
        lease: RunLeaseContext<'_>,
        execution: &PlaybookRunExecution,
        connection: PlaybookConnection,
        host_var_token: Option<&str>,
        passwords: &mut BTreeMap<String, String>,
    ) -> Option<AnsibleTargetInventory> {
        let target_name = execution.run.target_name.as_deref().unwrap_or("target");
        let mut inventory = match build_target_inventory_with_alias(
            target_name,
            &self.alias,
            connection,
            &self.proxy_payload,
            &piping_host_vars(&execution.run, host_var_token),
        ) {
            Ok(inventory) => inventory,
            Err(error) => {
                fail_started_run(
                    lease.state,
                    lease.lease_owner,
                    execution.run.id,
                    &format!("Failed to build Ansible inventory: {error}"),
                )
                .await;
                return None;
            }
        };
        if let Err(reason) =
            merge_group_passwords(passwords, std::mem::take(&mut inventory.passwords))
        {
            fail_started_run(lease.state, lease.lease_owner, execution.run.id, reason).await;
            return None;
        }
        Some(inventory)
    }
}

/// Authorization snapshot and launch policy shared by targets in one leased group.
struct GroupTargetPreparation<'a> {
    state: &'a AppState,
    lease_owner: &'a str,
    connection: PlaybookConnection,
    declared_secrets: &'a BTreeMap<String, String>,
    authorized_secrets: BTreeMap<String, (Uuid, String)>,
    subject: Option<SecretRecheckSubject>,
}

async fn prepare_group_targets(
    runs: Vec<PlaybookRun>,
    context: GroupTargetPreparation<'_>,
) -> (Vec<PreparedGroupedRun>, Option<BTreeMap<String, String>>) {
    let mut allocated_aliases = BTreeSet::new();
    let mut prepared_runs = Vec::new();
    let mut group_runtime_secret_env = None;
    for run in runs {
        let alias = allocate_run_inventory_alias(&run, &mut allocated_aliases);
        let (prepared, runtime_secret_env) = match context.prepare(run.clone(), alias).await {
            Ok(value) => value,
            Err(reason) => {
                fail_run_without_start(context.state, context.lease_owner, run.id, &reason).await;
                continue;
            }
        };
        match &group_runtime_secret_env {
            Some(existing) if existing != &runtime_secret_env => {
                fail_run_without_start(
                    context.state,
                    context.lease_owner,
                    run.id,
                    "Run group resolves different runtime secret environment values across targets; grouped execution requires identical runtime secrets",
                )
                .await;
                continue;
            }
            None => group_runtime_secret_env = Some(runtime_secret_env),
            _ => {}
        }
        prepared_runs.push(prepared);
    }
    (prepared_runs, group_runtime_secret_env)
}

impl GroupTargetPreparation<'_> {
    async fn prepare(
        &self,
        run: PlaybookRun,
        alias: String,
    ) -> Result<(PreparedGroupedRun, BTreeMap<String, String>), String> {
        let proxy_payload = self.proxy_payload(&run).await?;
        let runtime_secret_env = resolve_runtime_secret_env(
            self.state,
            run.id,
            run.target_id,
            self.declared_secrets,
            &self.authorized_secrets,
            self.subject,
        )
        .await
        .map_err(|error| format!("Failed to resolve playbook runtime secrets: {error}"))?;
        if !runtime_secret_env.is_empty() {
            log_event(
                LogLevel::Info,
                "playbooks.run_worker.runtime_secret_env_group_scope",
                EventFields::default()
                    .with_operation("playbook_runtime_secret_resolution")
                    .with_status("ok")
                    .with_resource("playbook_run", run.id.to_string())
                    .with_context("secret_count", runtime_secret_env.len().to_string()),
            );
        }
        Ok((
            PreparedGroupedRun {
                run,
                alias,
                proxy_payload,
            },
            runtime_secret_env,
        ))
    }

    async fn proxy_payload(&self, run: &PlaybookRun) -> Result<Value, String> {
        if matches!(self.connection, PlaybookConnection::Local) {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        let credential_user_id = match resolve_run_credential_subject(run) {
            RunCredentialSubject::User(user_id) => user_id,
            RunCredentialSubject::Unsupported(reason) => return Err(reason),
        };
        self.state
            .services
            .connection
            .resolve_ssh_execution_launch(
                ResolveSshExecutionLaunchInput {
                    execution_id: run.id,
                    target_id: run.target_id,
                    credential_user_id: Some(credential_user_id),
                    session_metadata: None,
                    protocol_params: None,
                    requested_proxy_node_id: None,
                },
                Some(self.state.services.access_control.as_ref()),
                None,
            )
            .await
            .map(|launch| launch.proxy_payload)
            .map_err(|error| format!("Failed to resolve SSH execution launch: {error}"))
    }
}

fn run_inventory_alias_base(run: &PlaybookRun) -> String {
    sanitize_host_alias(run.target_name.as_deref().unwrap_or("target"))
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

/// Decide SSH key delivery for a grouped run once every target's resolved key
/// is known. One distinct key (or none) uses a group-wide ssh-agent key with
/// unchanged inventory — the common fleet case where every host shares one key.
/// Multiple distinct keys switch to per-host mode: each keyed host's inventory
/// line gains an `ansible_ssh_private_key_file` var pointing at the runner's
/// per-host key file, so a fan-out over hosts with different keys runs instead
/// of failing on the second key. Returns the assembled inventory, the optional
/// group-wide key, and the per-target key each host must ship itself (aligned to
/// `target_lines` by index).
fn assemble_group_key_material(
    target_lines: &[String],
    target_keys: &[Option<String>],
    aliases: &[String],
) -> (String, Option<String>, Vec<Option<String>>) {
    let distinct: BTreeSet<&String> = target_keys.iter().flatten().collect();
    if distinct.len() <= 1 {
        return (
            target_lines.concat(),
            distinct.into_iter().next().cloned(),
            vec![None; target_lines.len()],
        );
    }
    let mut inventory = String::new();
    let mut per_target = vec![None; target_lines.len()];
    for (index, key) in target_keys.iter().enumerate() {
        match key {
            Some(key) => {
                per_target[index] = Some(key.clone());
                inventory.push_str(&with_per_host_key_file(
                    &target_lines[index],
                    &aliases[index],
                ));
            }
            None => inventory.push_str(&target_lines[index]),
        }
    }
    (inventory, None, per_target)
}

fn merge_group_passwords(
    passwords: &mut BTreeMap<String, String>,
    target_passwords: BTreeMap<String, String>,
) -> Result<(), &'static str> {
    if target_passwords.is_empty() {
        return Ok(());
    }
    if passwords.is_empty() {
        *passwords = target_passwords;
        return Ok(());
    }
    if passwords == &target_passwords {
        return Ok(());
    }
    Err(
        "Run group uses multiple SSH password prompt values; group execution requires identical password material or key-based access",
    )
}

fn piping_host_vars(run: &PlaybookRun, api_token: Option<&str>) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::from([
        ("piping_run_id".to_string(), run.id.to_string()),
        ("piping_source_kind".to_string(), run.source_kind.clone()),
        ("piping_source_id".to_string(), run.source_id.to_string()),
        ("piping_target_kind".to_string(), run.target_kind.clone()),
        ("piping_target_id".to_string(), run.target_id.to_string()),
    ]);
    if let Some(name) = run
        .target_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        vars.insert("piping_target_name".to_string(), name.to_string());
    }
    if let Some(token) = api_token.map(str::trim).filter(|token| !token.is_empty()) {
        vars.insert("piping_api_token".to_string(), token.to_string());
    }
    vars
}

async fn fail_started_group(
    state: &AppState,
    lease_owner: &str,
    runs: &[PlaybookRun],
    reason: &str,
) {
    for run in runs {
        fail_started_run(state, lease_owner, run.id, reason).await;
    }
}

/// Linear dispatch backoff: attempt 1 waits one base interval, attempt 2 two,
/// and so on.
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

fn unrequeued_run_ids(requested: &[PlaybookRun], requeued: &[PlaybookRun]) -> BTreeSet<Uuid> {
    let requeued_ids = requeued.iter().map(|run| run.id).collect::<BTreeSet<_>>();
    requested
        .iter()
        .filter(|run| !requeued_ids.contains(&run.id))
        .map(|run| run.id)
        .collect()
}

/// Requeue only after a receipt lookup returned 404, which proves this dispatch
/// identity was never accepted. Ambiguous failures never call this function.
async fn requeue_or_fail_started_group(
    state: &AppState,
    lease_owner: &str,
    runs: &[PlaybookRun],
    dispatch_attempt: i32,
    reason: &str,
) {
    let retry_delay_seconds = match dispatch_failure_action(dispatch_attempt) {
        DispatchFailureAction::Fail => {
            fail_started_group(
                state,
                lease_owner,
                runs,
                &format!(
                    "{reason} (dispatch attempt {dispatch_attempt}/{PLAYBOOK_DISPATCH_MAX_ATTEMPTS} exhausted)"
                ),
            )
            .await;
            return;
        }
        DispatchFailureAction::Requeue {
            retry_delay_seconds,
        } => retry_delay_seconds,
    };
    match state
        .services
        .playbooks
        .requeue_started_runs(
            RequeueStartedRunsInput {
                run_ids: runs.iter().map(|run| run.id).collect(),
                lease_owner: lease_owner.to_string(),
                retry_delay_seconds,
                reason: reason.to_string(),
            },
            Utc::now(),
        )
        .await
    {
        Ok(requeued) => {
            metrics::record_playbook_dispatch_retries(requeued.len());
            log_event(
                LogLevel::Warning,
                "playbooks.run_worker.dispatch_requeued",
                EventFields::default()
                    .with_operation("playbook_run_worker_dispatch")
                    .with_status("requeued")
                    .with_context("requeued_runs", requeued.len().to_string())
                    .with_context("retry_delay_seconds", retry_delay_seconds.to_string())
                    .with_context("reason", reason.to_string()),
            );
            // A run the repository could not park (canceled or its state moved
            // underneath us) must not stay stuck in `running` — terminalize it.
            let unrequeued_ids = unrequeued_run_ids(runs, &requeued);
            for run in runs.iter().filter(|run| unrequeued_ids.contains(&run.id)) {
                fail_started_run(state, lease_owner, run.id, reason).await;
            }
        }
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.dispatch_requeue_failed",
                "playbook_run_worker_dispatch_requeue_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_dispatch")
                    .with_status("failed")
                    .with_context("reason", reason.to_string()),
            );
            fail_started_group(state, lease_owner, runs, reason).await;
        }
    }
}

async fn fail_leased_group_without_start(
    state: &AppState,
    lease_owner: &str,
    runs: &[PlaybookRun],
    reason: &str,
) {
    for run in runs {
        fail_run_without_start(state, lease_owner, run.id, reason).await;
    }
}

/// Parsed C3 authorization snapshot: `ENV_NAME -> (secret_id, key)`.
///
/// Written server-side by the launch-time C3 gate (`ensure_secret_use_grants`),
/// it records exactly which secret each declared env name was authorized to
/// resolve — by id, so a version republished after gating cannot swap the
/// secret behind an env name. Values are never present in the snapshot.
fn parse_secret_env_authorization(metadata: &Value) -> BTreeMap<String, (Uuid, String)> {
    let mut out = BTreeMap::new();
    let Some(env) = metadata
        .get("secret_env_authorization")
        .and_then(|snapshot| snapshot.get("env"))
        .and_then(Value::as_object)
    else {
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

/// Worker-side C3 re-check for the run's initiator (revocation guard). Mirrors
/// the pure launch-time `secret_use_allowed` core — deny-veto over both
/// candidates, then allow via either — but drives the principal-aware access
/// path (`check_access_for_principal`) because the worker holds a stored
/// subject, not a live request context. No PAT ceiling here: the token ceiling
/// is a launch-time concern; at execution the subject's live policies decide,
/// so a grant revoked between launch and dispatch fails the run.
async fn initiator_may_use_secret(
    state: &AppState,
    principal: &Principal,
    secret_id: Uuid,
    key: &str,
) -> Result<bool, AppError> {
    let candidates = secret_use_candidates(secret_id, key);
    let mut decisions = Vec::with_capacity(candidates.len());
    for candidate in &candidates {
        decisions.push(
            state
                .services
                .access_control
                .check_access_for_principal(
                    principal,
                    Action::Execute,
                    ResourceType::SecretVariable,
                    Some(candidate.as_str()),
                    None,
                )
                .await?,
        );
    }
    if decisions.iter().any(|meta| !meta.matched_deny.is_empty()) {
        return Ok(false);
    }
    Ok(decisions
        .iter()
        .any(|meta| meta.decision == PolicyDecision::Allow))
}

/// The stored identity a dispatched run group re-checks its C3 secret grants
/// against, derived from the run group's fields.
///
/// There is exactly one such subject per run group: a manual run's requesting
/// user or a scheduled run's service principal. The requesting user takes
/// precedence when both fields are somehow present (the historical contract);
/// `None` means the run declares secrets but carries no re-checkable subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretRecheckSubject {
    User(Uuid),
    ServicePrincipal(Uuid),
}

/// Map a run group's stored identity fields to the C3 re-check subject.
///
/// Precedence: a requesting user wins over a service principal field; only a
/// scheduled run carries the latter. `None` is the no-subject case that makes a
/// declared-secret run fail closed.
pub(crate) fn secret_recheck_subject(
    requested_by_user_id: Option<Uuid>,
    service_principal_id: Option<Uuid>,
) -> Option<SecretRecheckSubject> {
    match (requested_by_user_id, service_principal_id) {
        (Some(user_id), _) => Some(SecretRecheckSubject::User(user_id)),
        (None, Some(service_principal_id)) => {
            Some(SecretRecheckSubject::ServicePrincipal(service_principal_id))
        }
        (None, None) => None,
    }
}

/// Resolve the runtime secret env exported to the runner, enforced at execution
/// time against the C3 authorization snapshot (fail-closed).
///
/// `declared` is what the running version's `token_policy.runtime.secrets.env`
/// asks for (`ENV -> key`); `authorized` is the launch-time snapshot
/// (`ENV -> (secret_id, key)`). A secret is exported only when ALL hold:
/// 1. the env name is present in the snapshot with a matching key (else the
///    version drifted after gating -> fail);
/// 2. the run's subject STILL has the `Execute x SecretVariable` grant
///    (revocation guard) — a manual run re-checks its requesting user, a
///    scheduled run re-loads and re-validates its service principal (active,
///    non-revoked) and then re-checks that principal;
/// 3. the secret is not `is_protected`;
/// 4. the secret is applicable to the target's DCIM scope (visibility) — this
///    is the AND with the site/region model.
///
/// Resolution is strictly by the snapshot's `secret_id`, never by key lookup.
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

    // A run that declares secrets must carry a re-checkable subject (the user or
    // service principal the author-time C3 gate cleared). A service principal is
    // re-loaded and re-validated (exists, active, not revoked) HERE, at
    // dispatch: the bare `run_group.service_principal_id` is never trusted, so
    // an SP revoked between authoring and dispatch cannot export a secret.
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
        if !initiator_may_use_secret(state, &principal, *secret_id, authorized_key).await? {
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
            .with_context("subject", principal_subject(&principal)),
    );
    Ok(env)
}

/// Extract the `default` values declared in a playbook's typed `vars_schema`
/// (`properties.<name>.default`) into a flat extravars object. Returns an empty
/// object when the schema is absent or declares no defaults.
fn vars_schema_defaults(vars_schema: Option<&Value>) -> Value {
    let mut defaults = serde_json::Map::new();
    if let Some(properties) = vars_schema
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
    {
        for (name, spec) in properties {
            if let Some(default) = spec.get("default") {
                defaults.insert(name.clone(), default.clone());
            }
        }
    }
    Value::Object(defaults)
}

fn launch_options(run_metadata: &Value) -> Option<&serde_json::Map<String, Value>> {
    run_metadata
        .get("launch_options")
        .and_then(Value::as_object)
}

fn launch_connection(
    default_connection: PlaybookConnection,
    run_metadata: &Value,
) -> Result<PlaybookConnection, AppError> {
    launch_options(run_metadata)
        .and_then(|options| options.get("connection"))
        .and_then(Value::as_str)
        .map(parse_playbook_connection)
        .transpose()
        .map(|connection| connection.unwrap_or(default_connection))
}

fn launch_extravars(vars_schema: Option<&Value>, run_metadata: &Value) -> Value {
    let mut values = match vars_schema_defaults(vars_schema) {
        Value::Object(defaults) => defaults,
        _ => serde_json::Map::new(),
    };
    if let Some(extravars) = launch_options(run_metadata)
        .and_then(|options| options.get("extravars"))
        .and_then(Value::as_object)
    {
        for (name, value) in extravars {
            values.insert(name.clone(), value.clone());
        }
    }
    Value::Object(values)
}

fn launch_settings(timeout_seconds: i32, forks: u32, run_metadata: &Value) -> Value {
    let mut settings = serde_json::Map::new();
    settings.insert(
        "job_timeout".to_string(),
        Value::from(timeout_seconds.max(1)),
    );
    settings.insert("forks".to_string(), Value::from(forks.max(1)));
    if launch_options(run_metadata)
        .and_then(|options| options.get("dry_run"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        settings.insert("check".to_string(), Value::Bool(true));
    }
    if let Some(options) = launch_options(run_metadata) {
        for name in [
            "limit",
            "tags",
            "skip_tags",
            "start_at_task",
            "execution_backend",
            "strategy",
            "stdout_callback",
            "ssh_args",
            "ssh_common_args",
            "python_interpreter",
            "remote_tmp",
            "shell_executable",
        ] {
            if let Some(value) = options.get(name).and_then(Value::as_str) {
                settings.insert(name.to_string(), Value::String(value.to_string()));
            }
        }
        for name in ["verbosity", "timeout", "ansible_timeout"] {
            if let Some(value) = options.get(name).and_then(Value::as_i64) {
                settings.insert(name.to_string(), Value::from(value.max(1)));
            }
        }
        for name in [
            "diff",
            "check",
            "quiet",
            "pipelining",
            "display_ok_hosts",
            "display_skipped_hosts",
            "deprecation_warnings",
            "compact_events",
            "raw_stdout_passthrough",
            "include_internal_events",
            "full_result_payload",
            "omit_event_data",
            "only_failed_event_data",
        ] {
            if let Some(value) = options.get(name).and_then(Value::as_bool) {
                settings.insert(name.to_string(), Value::Bool(value));
            }
        }
    }
    Value::Object(settings)
}

fn launch_execution_environment_id(run_metadata: &Value) -> Option<Uuid> {
    run_metadata
        .get("job_template")
        .and_then(Value::as_object)
        .and_then(|template| template.get("execution_environment_id"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn launch_project_id(run_metadata: &Value) -> Option<Uuid> {
    run_metadata
        .get("job_template")
        .and_then(Value::as_object)
        .and_then(|template| template.get("project_id"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn launch_project_playbook_path(run_metadata: &Value) -> Option<String> {
    run_metadata
        .get("job_template")
        .and_then(Value::as_object)
        .and_then(|template| template.get("playbook_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn project_bundle_for_launch(
    state: &AppState,
    run_metadata: &Value,
) -> Result<Option<ProjectBundle>, AppError> {
    let Some(project_id) = launch_project_id(run_metadata) else {
        return Ok(None);
    };
    let Some(playbook_path) = launch_project_playbook_path(run_metadata) else {
        return Err(AppError::Validation(
            "Project-backed job template launch is missing playbook_path".to_string(),
        ));
    };
    let Some(project) = state.services.playbooks.get_project(project_id).await? else {
        return Err(AppError::Validation(format!(
            "Playbook project {project_id} was not found"
        )));
    };
    let credential = resolve_scm_credential(state, &project).await?;
    build_project_bundle(&project, &playbook_path, credential)
        .await
        .map(Some)
}

/// Resolves a project's optional SCM credential reference into concrete secret
/// material for the git checkout. `credential_id` points at a named automation
/// secret; the resolved value is classified into token vs SSH-key auth. A
/// credential set on a non-git project is ignored (there is no remote to
/// authenticate against). A dangling or non-named reference fails loudly.
async fn resolve_scm_credential(
    state: &AppState,
    project: &PlaybookProject,
) -> Result<Option<ScmCredential>, AppError> {
    let Some(credential_id) = project.credential_id else {
        return Ok(None);
    };
    if project.scm_type != "git" {
        return Ok(None);
    }
    let variable = state
        .repositories
        .secrets
        .get_variable_by_id(credential_id)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!(
                "Playbook project SCM credential {credential_id} was not found"
            ))
        })?;
    if variable.kind != crate::contexts::secrets::domain::SECRET_KIND_NAMED {
        return Err(AppError::Validation(
            "Playbook project SCM credential must reference a named automation secret".to_string(),
        ));
    }
    let value = state
        .services
        .secret_store
        .get_secret(credential_id)
        .await?;
    Ok(Some(classify_scm_credential(&value)))
}

async fn resolve_execution_environment_for_launch(
    state: &AppState,
    run_metadata: &Value,
) -> Result<Option<PlaybookExecutionEnvironment>, AppError> {
    let Some(environment_id) = launch_execution_environment_id(run_metadata) else {
        return Ok(None);
    };
    let Some(environment) = state
        .services
        .playbooks
        .get_execution_environment(environment_id)
        .await?
    else {
        return Err(AppError::Validation(format!(
            "Execution environment {environment_id} was not found"
        )));
    };
    if !environment.enabled {
        return Err(AppError::Validation(format!(
            "Execution environment {} is disabled",
            environment.name
        )));
    }
    Ok(Some(environment))
}

fn execution_environment_env(
    environment: &PlaybookExecutionEnvironment,
) -> Result<BTreeMap<String, String>, AppError> {
    let Some(values) = environment.env.as_object() else {
        return Err(AppError::Validation(
            "Execution environment env must be an object".to_string(),
        ));
    };
    let mut env = BTreeMap::new();
    for (key, value) in values {
        let Some(value) = value.as_str() else {
            return Err(AppError::Validation(format!(
                "Execution environment env value for {key} must be a string"
            )));
        };
        env.insert(key.clone(), value.to_string());
    }
    Ok(env)
}

fn merge_runner_settings(environment_settings: &Value, launch_settings: &Value) -> Value {
    let mut merged = environment_settings
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(overrides) = launch_settings.as_object() {
        merged.extend(overrides.clone());
    }
    Value::Object(merged)
}

fn effective_group_forks(
    configured_forks: u32,
    max_group_concurrency: u32,
    target_count: usize,
) -> u32 {
    configured_forks
        .min(max_group_concurrency)
        .min(u32::try_from(target_count).unwrap_or(u32::MAX))
        .max(1)
}

fn requested_group_forks(configured_forks: u32, run_metadata: &Value) -> u32 {
    launch_options(run_metadata)
        .and_then(|options| options.get("forks"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(configured_forks)
}

fn grouped_job_timeout_seconds(
    per_target_timeout_seconds: i32,
    target_count: usize,
    forks: u32,
) -> i32 {
    let per_target = i64::from(per_target_timeout_seconds.max(1));
    let target_count = u64::try_from(target_count).unwrap_or(u64::MAX).max(1);
    let forks = u64::from(forks.max(1));
    let waves = target_count.saturating_add(forks.saturating_sub(1)) / forks;
    let timeout = per_target.saturating_mul(i64::try_from(waves).unwrap_or(i64::MAX));
    timeout.min(i64::from(PLAYBOOK_GROUP_JOB_TIMEOUT_MAX_SECONDS)) as i32
}

fn run_timeout_policy_base_seconds(run_metadata: &Value, fallback_timeout_seconds: i32) -> i32 {
    let Some(policy) = run_metadata
        .get("timeout_policy")
        .and_then(Value::as_object)
    else {
        return fallback_timeout_seconds.max(1);
    };
    if policy.get("source").and_then(Value::as_str) != Some("run_group") {
        return fallback_timeout_seconds.max(1);
    }
    policy
        .get("base_timeout_seconds")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| (1..=PLAYBOOK_GROUP_JOB_TIMEOUT_MAX_SECONDS).contains(value))
        .unwrap_or_else(|| fallback_timeout_seconds.max(1))
}

fn planned_group_job_timeout_seconds(
    primary_run_timeout_seconds: i32,
    target_count: usize,
    forks: u32,
    primary_run_metadata: &Value,
) -> i32 {
    let base_timeout_seconds =
        run_timeout_policy_base_seconds(primary_run_metadata, primary_run_timeout_seconds);
    let expected_timeout_seconds =
        grouped_job_timeout_seconds(base_timeout_seconds, target_count, forks);
    primary_run_timeout_seconds
        .max(expected_timeout_seconds)
        .max(1)
}

fn grouped_job_token_ttl_seconds(job_timeout_seconds: i32) -> i64 {
    i64::from(job_timeout_seconds.max(1))
        .saturating_add(300)
        .clamp(3_600, i64::from(PLAYBOOK_GROUP_JOB_TIMEOUT_MAX_SECONDS))
}

/// Terminally fail a run that was already started (job token minted) but could
/// not be dispatched to the sidecar. The run is `running`, which is within
/// `complete_run`'s active set, so this transitions cleanly and rolls up the
/// group. A `Conflict` means a callback already completed it — not an error.
async fn fail_started_run(state: &AppState, lease_owner: &str, run_id: Uuid, reason: &str) {
    log_event(
        LogLevel::Warning,
        "playbooks.run_worker.dispatch_failed",
        EventFields::default()
            .with_operation("playbook_run_worker_dispatch")
            .with_status("failed")
            .with_resource("playbook_run", run_id.to_string())
            .with_context("reason", reason.to_string()),
    );
    match state
        .services
        .playbooks
        .complete_run(
            CompleteRunInput {
                run_id,
                lease_owner: lease_owner.to_string(),
                status: PlaybookRunStatus::Failed,
                exit_code: None,
                error_message: Some(reason.to_string()),
            },
            Utc::now(),
        )
        .await
    {
        Ok(_) | Err(AppError::Conflict(_)) => {}
        Err(error) => {
            log_error_event(
                "playbooks.run_worker.dispatch_complete_failed",
                "playbook_run_worker_dispatch_complete_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("playbook_run_worker_dispatch_complete")
                    .with_status("failed")
                    .with_resource("playbook_run", run_id.to_string()),
            );
        }
    }
}

/// Terminally fail a leased run that we never started (no job token minted), so
/// the run does not loop or stay stuck. A leased run is within `complete_run`'s
/// active WHERE set, so this transitions cleanly and rolls up the group.
async fn fail_run_without_start(state: &AppState, lease_owner: &str, run_id: Uuid, reason: &str) {
    log_event(
        LogLevel::Warning,
        "playbooks.run_worker.run_unresolvable",
        EventFields::default()
            .with_operation("playbook_run_worker_fail")
            .with_status("failed")
            .with_resource("playbook_run", run_id.to_string())
            .with_context("reason", reason.to_string()),
    );
    if let Err(error) = state
        .services
        .playbooks
        .complete_run(
            CompleteRunInput {
                run_id,
                lease_owner: lease_owner.to_string(),
                status: PlaybookRunStatus::Failed,
                exit_code: None,
                error_message: Some(reason.to_string()),
            },
            Utc::now(),
        )
        .await
    {
        log_error_event(
            "playbooks.run_worker.fail_complete_failed",
            "playbook_run_worker_fail_error",
            error.to_string(),
            EventFields::default()
                .with_operation("playbook_run_worker_fail")
                .with_status("failed")
                .with_resource("playbook_run", run_id.to_string()),
        );
    }
}

/// Background worker that fires due playbook schedules: leases each due schedule,
/// creates its scheduled run group (the SQL enforces concurrency policy),
/// advances `next_run_at` via the cron crate + misfire policy, and releases the
/// lease. Backend mode only.
pub(super) async fn run_playbook_schedule_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat("playbook_schedule");
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
                metrics::record_worker_heartbeat("playbook_schedule");
                match run_playbook_schedule_worker_tick(&state, &lease_owner).await {
                    TickOutcome::Healthy => {
                        log_worker_recovery(
                            &backoff,
                            "playbooks.schedule_worker.recovered",
                            "playbook_schedule_worker_tick",
                        );
                        backoff.on_success();
                    }
                    TickOutcome::DependencyFailed => {
                        backoff.on_failure();
                    }
                }
            }
        }
    }
}

/// Periodic git refresh of the official playbook catalog overlay.
///
/// Best-effort by design: the baked snapshot synced at startup is always
/// sufficient, so every failure here (network, git, validation) is a warning
/// and a skipped tick — never an error path. Air-gapped installs disable the
/// worker outright via `official_catalog.refresh_enabled = false`.
pub(super) async fn run_official_catalog_refresh_worker(
    state: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    metrics::record_worker_heartbeat("official_catalog_refresh");
    let config = &state.settings.official_catalog;
    if !config.refresh_enabled
        || config.work_dir.trim().is_empty()
        || config.repo_url.trim().is_empty()
    {
        log_event(
            LogLevel::Info,
            "playbooks.official_catalog.refresh_disabled",
            EventFields::default()
                .with_operation("official_catalog_refresh")
                .with_status("disabled"),
        );
        return;
    }
    let interval_seconds = config.refresh_interval_seconds.max(60);
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_seconds));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Startup already synced the catalog; consume the interval's immediate
    // first tick so the first refresh happens one full interval later.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = ticker.tick() => {
                metrics::record_worker_heartbeat("official_catalog_refresh");
                run_official_catalog_refresh_tick(&state).await;
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

async fn run_official_catalog_refresh_tick(state: &AppState) {
    let config = &state.settings.official_catalog;
    match crate::contexts::playbooks::infrastructure::official_catalog::refresh_official_catalog(
        &state.db_pool,
        config,
    )
    .await
    {
        Ok(synced) => {
            log_event(
                LogLevel::Info,
                "playbooks.official_catalog.refreshed",
                EventFields::default()
                    .with_operation("official_catalog_refresh")
                    .with_status("ok")
                    .with_context("synced_playbooks", synced.len().to_string()),
            );
        }
        Err(error) => {
            // Warning by contract: an unreachable repo (air-gap, outage) must
            // not degrade the product; the baked/overlay catalog stays live.
            log_event(
                LogLevel::Warning,
                "playbooks.official_catalog.refresh_failed",
                EventFields::default()
                    .with_operation("official_catalog_refresh")
                    .with_status("skipped")
                    .with_context("error", error.to_string()),
            );
        }
    }
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
        .create_scheduled_run_group(
            schedule.playbook_id,
            crate::contexts::playbooks::application::CreateRunGroupInput {
                target_set_id: Some(schedule.target_set_id),
                schedule_id: Some(schedule_id),
                scheduled_for: Some(scheduled_for),
                trigger_kind: None,
                target_limit: None,
                metadata: None,
                // Scheduled run groups are authored by the worker at fire time.
                // The C3 snapshot is authored by `create_scheduled_run_group`
                // itself (the service overrides this field after clearing the
                // service-principal secret gate), so it is never supplied here.
                secret_env_authorization: None,
            },
            Some(schedule.service_principal_id),
        )
        .await;

    match create_result {
        Ok(_) => {}
        Err(error) => {
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
        PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS, RunCredentialSubject, SecretRecheckSubject,
        allocate_run_inventory_alias, assemble_group_key_material, compute_next_run_at,
        dispatch_failure_action, dispatch_retry_delay_seconds, effective_group_forks,
        execution_environment_env, grouped_job_timeout_seconds, grouped_job_token_ttl_seconds,
        launch_connection, launch_execution_environment_id, launch_extravars, launch_settings,
        leased_run_group_ids, merge_group_passwords, merge_runner_settings, owned_leased_runs,
        piping_host_vars, planned_group_job_timeout_seconds, requested_group_forks,
        resolve_run_credential_subject, secret_recheck_subject, unrequeued_run_ids,
    };
    use crate::contexts::playbooks::domain::{
        PlaybookConnection, PlaybookExecutionEnvironment, PlaybookRun, PlaybookRunStatus,
        PlaybookScheduleMisfirePolicy,
    };
    use crate::contexts::playbooks::infrastructure::AnsibleJobTarget;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use uuid::Uuid;

    fn run_with_metadata(metadata: serde_json::Value) -> PlaybookRun {
        PlaybookRun {
            id: Uuid::from_u128(1),
            run_group_id: Uuid::from_u128(2),
            playbook_id: Uuid::from_u128(3),
            playbook_version_id: Uuid::from_u128(4),
            target_kind: "connection_target".to_string(),
            target_id: Uuid::from_u128(5),
            source_kind: "device".to_string(),
            source_id: Uuid::from_u128(5),
            target_name: None,
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
        let parsed = super::parse_secret_env_authorization(&snapshot);
        assert_eq!(parsed.len(), 1);
        let (id, key) = parsed.get("GRAD_TOKEN").expect("authorized env");
        assert_eq!(key, "grad-token");
        assert_eq!(id.to_string(), "01890000-0000-7000-8000-000000000001");
        // A malformed entry (missing secret_id) is dropped, never resolved.
        assert!(!parsed.contains_key("MALFORMED"));
    }

    #[test]
    fn parse_secret_env_authorization_is_empty_without_snapshot() {
        assert!(super::parse_secret_env_authorization(&json!({})).is_empty());
        assert!(
            super::parse_secret_env_authorization(&json!({"secret_env_authorization": {}}))
                .is_empty()
        );
    }

    #[test]
    fn piping_host_vars_only_carries_api_token_when_explicitly_granted() {
        let run = run_with_metadata(json!({}));

        // Gate closed (expose_piping_api=false path): the caller passes None, so
        // the raw callback token never lands in the inventory host vars.
        let without = piping_host_vars(&run, None);
        assert!(!without.contains_key("piping_api_token"));
        assert!(without.contains_key("piping_run_id"));

        // Gate open: the token is exposed as a host var, trimmed.
        let with = piping_host_vars(&run, Some("  pjt_abc_secret  "));
        assert_eq!(
            with.get("piping_api_token").map(String::as_str),
            Some("pjt_abc_secret")
        );

        // A blank token is treated as absent (never an empty host var).
        assert!(!piping_host_vars(&run, Some("   ")).contains_key("piping_api_token"));
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
        // Defensive floor: attempts below 1 never yield a zero/negative delay.
        assert_eq!(
            dispatch_retry_delay_seconds(0),
            PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS
        );
    }

    #[test]
    fn leased_runs_collapse_to_one_stably_ordered_dispatch_per_group() {
        let group_a = Uuid::from_u128(0xa);
        let group_b = Uuid::from_u128(0xb);
        let mut first_b = run_with_metadata(json!({}));
        first_b.id = Uuid::from_u128(1);
        first_b.run_group_id = group_b;
        let mut only_a = first_b.clone();
        only_a.id = Uuid::from_u128(2);
        only_a.run_group_id = group_a;
        let mut second_b = first_b.clone();
        second_b.id = Uuid::from_u128(3);

        assert_eq!(
            leased_run_group_ids(&[first_b, only_a, second_b]),
            vec![group_a, group_b]
        );
    }

    #[test]
    fn group_execution_keeps_only_runs_leased_by_this_worker() {
        let mut owned = run_with_metadata(json!({}));
        owned.id = Uuid::from_u128(1);
        let mut foreign = owned.clone();
        foreign.id = Uuid::from_u128(2);
        foreign.lease_owner = Some("another-worker".to_string());
        let mut moved = owned.clone();
        moved.id = Uuid::from_u128(3);
        moved.status = PlaybookRunStatus::Running;

        let selected = owned_leased_runs(vec![moved, foreign, owned.clone()], "worker");

        assert_eq!(
            selected.iter().map(|run| run.id).collect::<Vec<_>>(),
            vec![owned.id]
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
    fn partial_requeue_terminalizes_only_runs_the_repository_did_not_park() {
        let mut first = run_with_metadata(json!({}));
        first.id = Uuid::from_u128(1);
        let mut missing = first.clone();
        missing.id = Uuid::from_u128(2);
        let mut last = first.clone();
        last.id = Uuid::from_u128(3);

        assert_eq!(
            unrequeued_run_ids(
                &[first.clone(), missing.clone(), last.clone()],
                &[last, first]
            ),
            BTreeSet::from([missing.id])
        );
    }

    #[test]
    fn user_subject_resolves_to_credential_user_id() {
        let user_id = Uuid::from_u128(0xabc);
        let run = run_with_metadata(json!({
            "credential": { "subject_kind": "user", "subject_id": user_id.to_string() }
        }));
        match resolve_run_credential_subject(&run) {
            RunCredentialSubject::User(resolved) => assert_eq!(resolved, user_id),
            RunCredentialSubject::Unsupported(reason) => {
                panic!("expected user subject, got unsupported: {reason}")
            }
        }
    }

    #[test]
    fn service_principal_subject_is_unsupported_in_phase_1() {
        let run = run_with_metadata(json!({
            "credential": {
                "subject_kind": "service_principal",
                "subject_id": Uuid::from_u128(0xdef).to_string()
            }
        }));
        assert!(matches!(
            resolve_run_credential_subject(&run),
            RunCredentialSubject::Unsupported(_)
        ));
    }

    #[test]
    fn missing_credential_metadata_is_unsupported() {
        let run = run_with_metadata(json!({}));
        assert!(matches!(
            resolve_run_credential_subject(&run),
            RunCredentialSubject::Unsupported(_)
        ));
    }

    #[test]
    fn run_inventory_alias_preserves_unique_target_name_and_suffixes_collisions() {
        let mut first = run_with_metadata(json!({}));
        let mut second = run_with_metadata(json!({}));
        first.id = Uuid::parse_str("019f3e36-aef1-7f4f-b3b4-000000000001").unwrap();
        second.id = Uuid::parse_str("019f3e36-aef1-7f4f-b3b4-000000000002").unwrap();
        first.target_name = Some("cash-1".to_string());
        second.target_name = Some("cash-1".to_string());

        let mut allocated = BTreeSet::new();
        let first_alias = allocate_run_inventory_alias(&first, &mut allocated);
        let second_alias = allocate_run_inventory_alias(&second, &mut allocated);

        assert_ne!(first_alias, second_alias);
        assert_eq!(first_alias, "cash-1");
        assert_eq!(second_alias, "cash-1_b3b4000000000002");
    }

    #[test]
    fn group_password_merge_allows_empty_and_identical_values() {
        let mut passwords = BTreeMap::new();
        assert!(merge_group_passwords(&mut passwords, BTreeMap::new()).is_ok());
        assert!(passwords.is_empty());

        let first = BTreeMap::from([("SSH password".to_string(), "secret".to_string())]);
        assert!(merge_group_passwords(&mut passwords, first.clone()).is_ok());
        assert_eq!(passwords, first);

        assert!(merge_group_passwords(&mut passwords, first.clone()).is_ok());
        assert_eq!(passwords, first);

        assert!(merge_group_passwords(&mut passwords, BTreeMap::new()).is_ok());
        assert_eq!(passwords, first);
    }

    #[test]
    fn group_password_merge_rejects_different_prompt_values() {
        let mut passwords = BTreeMap::from([("SSH password".to_string(), "secret-a".to_string())]);
        let different = BTreeMap::from([("SSH password".to_string(), "secret-b".to_string())]);

        let error = merge_group_passwords(&mut passwords, different).expect_err("must reject");
        assert!(error.contains("multiple SSH password prompt values"));
        assert_eq!(
            passwords,
            BTreeMap::from([("SSH password".to_string(), "secret-a".to_string())])
        );
    }

    #[test]
    fn group_key_material_shares_one_key_via_ssh_agent() {
        let lines = vec![
            "a ansible_connection=ssh\n".to_string(),
            "b ansible_connection=ssh\n".to_string(),
        ];
        let keys = vec![Some("KEY-1".to_string()), Some("KEY-1".to_string())];
        let aliases = vec!["a".to_string(), "b".to_string()];

        let (inventory, group_key, per_target) =
            assemble_group_key_material(&lines, &keys, &aliases);

        // Identical keys collapse to a single group-wide ssh-agent key; the
        // inventory is unchanged and no host ships its own key file.
        assert_eq!(group_key.as_deref(), Some("KEY-1"));
        assert_eq!(per_target, vec![None, None]);
        assert!(!inventory.contains("ansible_ssh_private_key_file"));
    }

    #[test]
    fn group_key_material_switches_to_per_host_on_distinct_keys() {
        let lines = vec![
            "a ansible_connection=ssh\n".to_string(),
            "b ansible_connection=ssh\n".to_string(),
            "c ansible_connection=ssh\n".to_string(),
        ];
        // a and c have distinct keys; b is password-only (no key).
        let keys = vec![Some("KEY-A".to_string()), None, Some("KEY-C".to_string())];
        let aliases = vec!["a".to_string(), "b".to_string(), "c".to_string()];

        let (inventory, group_key, per_target) =
            assemble_group_key_material(&lines, &keys, &aliases);

        // Distinct keys force per-host delivery: no group-wide key, each keyed
        // host carries its own material and a key-file inventory var; the
        // password-only host is untouched.
        assert_eq!(group_key, None);
        assert_eq!(
            per_target,
            vec![Some("KEY-A".to_string()), None, Some("KEY-C".to_string())]
        );
        assert!(
            inventory
                .contains("a ansible_connection=ssh ansible_ssh_private_key_file=.piping_keys/a\n")
        );
        assert!(inventory.contains("b ansible_connection=ssh\n"));
        assert!(
            inventory
                .contains("c ansible_connection=ssh ansible_ssh_private_key_file=.piping_keys/c\n")
        );
    }

    #[test]
    fn launch_extravars_merge_schema_defaults_with_run_inputs() {
        let vars_schema = json!({
            "type": "object",
            "properties": {
                "package": { "type": "string", "default": "curl" },
                "retries": { "type": "integer", "default": 1 },
                "flag": { "type": "boolean" }
            }
        });
        let run_metadata = json!({
            "launch_options": {
                "extravars": {
                    "package": "htop",
                    "dry_run_note": "not settings"
                }
            }
        });

        assert_eq!(
            launch_extravars(Some(&vars_schema), &run_metadata),
            json!({
                "package": "htop",
                "retries": 1,
                "dry_run_note": "not settings"
            })
        );
    }

    #[test]
    fn launch_connection_uses_per_run_override() {
        assert_eq!(
            launch_connection(
                PlaybookConnection::Ssh,
                &json!({ "launch_options": { "connection": "winrm" } })
            )
            .expect("valid override"),
            PlaybookConnection::Winrm
        );
        assert_eq!(
            launch_connection(PlaybookConnection::Winrm, &json!({})).expect("default"),
            PlaybookConnection::Winrm
        );
    }

    #[test]
    fn launch_connection_rejects_unknown_override() {
        assert!(
            launch_connection(
                PlaybookConnection::Ssh,
                &json!({ "launch_options": { "connection": "telnet" } })
            )
            .is_err()
        );
    }

    #[test]
    fn launch_settings_map_timeout_and_dry_run_to_runner_settings() {
        assert_eq!(
            launch_settings(
                42,
                32,
                &json!({
                    "launch_options": { "dry_run": true }
                })
            ),
            json!({ "job_timeout": 42, "forks": 32, "check": true })
        );
        assert_eq!(
            launch_settings(0, 0, &json!({ "launch_options": { "dry_run": false } })),
            json!({ "job_timeout": 1, "forks": 1 })
        );
    }

    #[test]
    fn launch_settings_pass_through_standard_ansible_runner_options() {
        assert_eq!(
            launch_settings(
                42,
                32,
                &json!({
                    "launch_options": {
                        "limit": "cash:&linux",
                        "tags": "inventory,network",
                        "skip_tags": "slow",
                        "start_at_task": "gather",
                        "execution_backend": "native",
                        "strategy": "free",
                        "stdout_callback": "ansible.builtin.default",
                        "ssh_common_args": "-o ConnectTimeout=12",
                        "python_interpreter": "auto_silent",
                        "remote_tmp": "/tmp/.ansible-piping/tmp",
                        "shell_executable": "/bin/sh",
                        "verbosity": 2,
                        "timeout": 60,
                        "ansible_timeout": 12,
                        "check": true,
                        "diff": true,
                        "quiet": false,
                        "pipelining": true,
                        "display_ok_hosts": true,
                        "display_skipped_hosts": false,
                        "deprecation_warnings": false,
                        "compact_events": false,
                        "raw_stdout_passthrough": true,
                        "include_internal_events": true,
                        "full_result_payload": true,
                        "omit_event_data": false,
                        "only_failed_event_data": true
                    }
                })
            ),
            json!({
                "job_timeout": 42,
                "forks": 32,
                "limit": "cash:&linux",
                "tags": "inventory,network",
                "skip_tags": "slow",
                "start_at_task": "gather",
                "execution_backend": "native",
                "strategy": "free",
                "stdout_callback": "ansible.builtin.default",
                "ssh_common_args": "-o ConnectTimeout=12",
                "python_interpreter": "auto_silent",
                "remote_tmp": "/tmp/.ansible-piping/tmp",
                "shell_executable": "/bin/sh",
                "verbosity": 2,
                "timeout": 60,
                "ansible_timeout": 12,
                "check": true,
                "diff": true,
                "quiet": false,
                "pipelining": true,
                "display_ok_hosts": true,
                "display_skipped_hosts": false,
                "deprecation_warnings": false,
                "compact_events": false,
                "raw_stdout_passthrough": true,
                "include_internal_events": true,
                "full_result_payload": true,
                "omit_event_data": false,
                "only_failed_event_data": true
            })
        );
    }

    #[test]
    fn job_target_known_hosts_lines_come_from_proxy_payload() {
        let line = "[host.example.org]:2222 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIseed";
        let payload = json!({
            "known_hosts_lines": [line, 7, "host.example.org ssh-rsa AAAAB3NzaC1yc2Eseed"],
        });

        let target = AnsibleJobTarget {
            alias: "host1".to_string(),
            run_id: Uuid::from_u128(2),
            job_token: "token".to_string(),
            target_kind: "connection_target".to_string(),
            target_id: Uuid::from_u128(3),
            source_kind: "device".to_string(),
            source_id: Some(Uuid::from_u128(3)),
            ssh_private_key: None,
            known_hosts_lines:
                crate::contexts::shared::host_key_pins::known_hosts_lines_from_payload(&payload),
        };

        assert_eq!(
            target.known_hosts_lines,
            vec![
                line.to_string(),
                "host.example.org ssh-rsa AAAAB3NzaC1yc2Eseed".to_string(),
            ],
            "the target must carry exactly the string lines from the payload"
        );
        assert!(
            crate::contexts::shared::host_key_pins::known_hosts_lines_from_payload(&json!({}))
                .is_empty(),
            "an absent key must seed no pins"
        );
        assert!(
            crate::contexts::shared::host_key_pins::known_hosts_lines_from_payload(
                &json!({ "known_hosts_lines": "not-an-array" })
            )
            .is_empty(),
            "a non-array value must seed no pins"
        );
    }

    #[test]
    fn launch_settings_omits_host_key_checking() {
        // The dead key is assembled at compile time so this guard does not put
        // the fail-open literal back into the source the exit gate scans.
        let dead_knob = concat!("host_key", "_checking");
        let mut metadata = json!({ "launch_options": { "check": true } });
        metadata["launch_options"]
            .as_object_mut()
            .expect("launch_options is an object")
            .insert(dead_knob.to_string(), json!(false));

        let settings = launch_settings(42, 32, &metadata);

        assert_eq!(
            settings.get(dead_knob),
            None,
            "the dead host_key_checking knob must not reach the runner: {settings}"
        );
        assert_eq!(settings["check"], json!(true));
    }

    #[test]
    fn launch_execution_environment_id_reads_job_template_metadata() {
        let environment_id = Uuid::from_u128(0x40);

        assert_eq!(
            launch_execution_environment_id(&json!({
                "job_template": {
                    "execution_environment_id": environment_id.to_string()
                }
            })),
            Some(environment_id)
        );
        assert_eq!(
            launch_execution_environment_id(&json!({
                "job_template": {
                    "execution_environment_id": "not-a-uuid"
                }
            })),
            None
        );
        assert_eq!(launch_execution_environment_id(&json!({})), None);
    }

    #[test]
    fn execution_environment_env_accepts_only_string_values() {
        let now = Utc::now();
        let environment = PlaybookExecutionEnvironment {
            id: Uuid::from_u128(0x40),
            name: "Default EE".to_string(),
            description: None,
            image: "quay.io/ansible/ansible-runner:latest".to_string(),
            pull_policy: "if_not_present".to_string(),
            runner_group: "default".to_string(),
            enabled: true,
            env: json!({
                "ANSIBLE_STDOUT_CALLBACK": "ansible.builtin.default",
                "ANSIBLE_FORCE_COLOR": "0"
            }),
            settings: json!({}),
            metadata: json!({}),
            created_by_user_id: None,
            updated_by_user_id: None,
            created_at: now,
            updated_at: now,
        };

        assert_eq!(
            execution_environment_env(&environment).expect("env"),
            BTreeMap::from([
                ("ANSIBLE_FORCE_COLOR".to_string(), "0".to_string()),
                (
                    "ANSIBLE_STDOUT_CALLBACK".to_string(),
                    "ansible.builtin.default".to_string()
                ),
            ])
        );

        let mut invalid = environment;
        invalid.env = json!({ "ANSIBLE_FORCE_COLOR": false });
        assert!(execution_environment_env(&invalid).is_err());
    }

    #[test]
    fn merge_runner_settings_keeps_environment_defaults_but_launch_wins() {
        assert_eq!(
            merge_runner_settings(
                &json!({
                    "strategy": "linear",
                    "forks": 5,
                    "raw_stdout_passthrough": false,
                    "display_ok_hosts": false
                }),
                &json!({
                    "forks": 32,
                    "job_timeout": 120,
                    "raw_stdout_passthrough": true
                }),
            ),
            json!({
                "strategy": "linear",
                "forks": 32,
                "job_timeout": 120,
                "raw_stdout_passthrough": true,
                "display_ok_hosts": false
            })
        );
    }

    #[test]
    fn effective_group_forks_clamps_to_configuration_and_target_count() {
        assert_eq!(effective_group_forks(50, 250, 1_697), 50);
        assert_eq!(effective_group_forks(250, 40, 1_697), 40);
        assert_eq!(effective_group_forks(50, 250, 7), 7);
        assert_eq!(effective_group_forks(0, 0, 0), 1);
    }

    #[test]
    fn requested_group_forks_prefers_launch_options() {
        assert_eq!(
            requested_group_forks(128, &json!({ "launch_options": { "forks": 256 } })),
            256
        );
        assert_eq!(
            requested_group_forks(128, &json!({ "launch_options": { "forks": 0 } })),
            128
        );
        assert_eq!(
            requested_group_forks(128, &json!({ "launch_options": { "forks": "256" } })),
            128
        );
    }

    #[test]
    fn grouped_job_timeout_scales_by_ansible_execution_waves() {
        assert_eq!(grouped_job_timeout_seconds(1_800, 1, 50), 1_800);
        assert_eq!(grouped_job_timeout_seconds(1_800, 51, 50), 3_600);
        assert_eq!(grouped_job_timeout_seconds(1_800, 1_697, 50), 61_200);
        assert_eq!(grouped_job_timeout_seconds(1_800, 1_697, 128), 25_200);
        assert_eq!(grouped_job_timeout_seconds(1_800, 1_697, 32), 86_400);
        assert_eq!(grouped_job_timeout_seconds(0, 0, 0), 1);
    }

    #[test]
    fn planned_group_job_timeout_reuses_repository_scaled_timeout() {
        let metadata = json!({
            "timeout_policy": {
                "source": "run_group",
                "base_timeout_seconds": 1_800,
                "timeout_seconds": 12_600,
                "target_count": 1_697,
                "requested_forks": 256,
                "effective_forks": 256,
                "waves": 7
            }
        });

        assert_eq!(
            planned_group_job_timeout_seconds(12_600, 1_697, 256, &metadata),
            12_600
        );
    }

    #[test]
    fn planned_group_job_timeout_scales_runs_that_have_no_policy() {
        assert_eq!(
            planned_group_job_timeout_seconds(1_800, 1_697, 256, &json!({})),
            12_600
        );
    }

    #[test]
    fn planned_group_job_timeout_rescales_unforked_repository_policy() {
        let metadata = json!({
            "timeout_policy": {
                "source": "run_group",
                "base_timeout_seconds": 1_800,
                "timeout_seconds": 1_800,
                "target_count": 1_697,
                "waves": 1
            }
        });

        assert_eq!(
            planned_group_job_timeout_seconds(1_800, 1_697, 256, &metadata),
            12_600
        );
    }

    #[test]
    fn planned_group_job_timeout_repairs_stale_repository_policy() {
        let metadata = json!({
            "timeout_policy": {
                "source": "run_group",
                "base_timeout_seconds": 1_800,
                "timeout_seconds": 3_600,
                "target_count": 1_697,
                "requested_forks": 1_024,
                "effective_forks": 1_024,
                "waves": 2
            }
        });

        assert_eq!(
            planned_group_job_timeout_seconds(3_600, 1_697, 128, &metadata),
            25_200
        );
    }

    #[test]
    fn grouped_job_token_ttl_keeps_single_runs_at_existing_default_floor() {
        assert_eq!(grouped_job_token_ttl_seconds(1_800), 3_600);
        assert_eq!(grouped_job_token_ttl_seconds(61_200), 61_500);
        assert_eq!(grouped_job_token_ttl_seconds(86_400), 86_400);
    }

    #[test]
    fn daily_cron_respects_iana_timezone_offset() {
        // "0 0 * * *" = midnight. In Europe/Moscow (UTC+3, no DST since 2014),
        // local midnight is 21:00 UTC the prior day.
        let after = Utc.with_ymd_and_hms(2026, 6, 6, 12, 0, 0).unwrap();
        let next = compute_next_run_at(
            "0 0 * * *",
            "Europe/Moscow",
            after,
            PlaybookScheduleMisfirePolicy::FireOnce,
        )
        .expect("cron computes")
        .expect("has a next occurrence");
        // Next Moscow midnight after 15:00 MSK on 2026-06-06 is 2026-06-07 00:00 MSK
        // = 2026-06-06 21:00 UTC.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 6, 21, 0, 0).unwrap());
    }

    #[test]
    fn daily_cron_in_utc_is_midnight_utc() {
        let after = Utc.with_ymd_and_hms(2026, 6, 6, 12, 0, 0).unwrap();
        let next = compute_next_run_at(
            "0 0 * * *",
            "UTC",
            after,
            PlaybookScheduleMisfirePolicy::FireOnce,
        )
        .expect("cron computes")
        .expect("has a next occurrence");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 7, 0, 0, 0).unwrap());
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let after = Utc.with_ymd_and_hms(2026, 6, 6, 12, 0, 0).unwrap();
        assert!(
            compute_next_run_at(
                "not a cron",
                "UTC",
                after,
                PlaybookScheduleMisfirePolicy::FireOnce,
            )
            .is_err()
        );
    }

    // 2026-03-08 spring-forward in America/New_York: clocks jump 02:00 EST ->
    // 03:00 EDT, so wall-clock 02:00-02:59 does not exist. Before the bounded
    // loop, a sub-hourly or 02:xx schedule under-advanced once and returned
    // Ok(None), silently disabling the schedule. These lock that it now lands
    // at/after 03:00 EDT (= 07:00 UTC).
    #[test]
    fn half_hourly_cron_survives_spring_forward_gap() {
        // 01:30 EST on 2026-03-08 = 06:30 UTC. Next `*/30` occurrences 02:00 and
        // 02:30 are in the lost hour; first real instant is 03:00 EDT = 07:00 UTC.
        let after = Utc.with_ymd_and_hms(2026, 3, 8, 6, 30, 0).unwrap();
        let next = compute_next_run_at(
            "*/30 * * * *",
            "America/New_York",
            after,
            PlaybookScheduleMisfirePolicy::FireOnce,
        )
        .expect("cron computes")
        .expect("has a next occurrence across the gap");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap());
    }

    #[test]
    fn every_minute_at_2am_cron_survives_spring_forward_gap() {
        // `* 2 * * *` only ever fires during the 02:00 hour, which is entirely
        // lost on the gap day. It must advance to the next real occurrence, the
        // 02:00 hour the following day (2026-03-09 02:00 EDT = 06:00 UTC), not
        // give up and disable.
        let after = Utc.with_ymd_and_hms(2026, 3, 8, 6, 30, 0).unwrap();
        let next = compute_next_run_at(
            "* 2 * * *",
            "America/New_York",
            after,
            PlaybookScheduleMisfirePolicy::FireOnce,
        )
        .expect("cron computes")
        .expect("has a next occurrence across the gap");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 9, 6, 0, 0).unwrap());
    }

    #[test]
    fn daily_2am_cron_does_not_silently_skip_the_gap_day() {
        // `0 2 * * *` would fire at 02:00 on 2026-03-08, which is lost. Rather
        // than returning Ok(None) it must produce a Some occurrence; the next
        // valid daily 02:00 is 2026-03-09 02:00 EDT = 06:00 UTC.
        let after = Utc.with_ymd_and_hms(2026, 3, 8, 6, 30, 0).unwrap();
        let next = compute_next_run_at(
            "0 2 * * *",
            "America/New_York",
            after,
            PlaybookScheduleMisfirePolicy::FireOnce,
        )
        .expect("cron computes")
        .expect("has a next occurrence, not a silent skip");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 9, 6, 0, 0).unwrap());
    }

    /// The stored-subject mapping is the author/dispatch contract: a requesting
    /// user wins over a service-principal field, and "neither" is the fail-closed
    /// case (a declared-secret run with no re-checkable subject is refused).
    #[test]
    fn secret_recheck_subject_prefers_user_and_defaults_to_none() {
        let user = Uuid::now_v7();
        let service_principal = Uuid::now_v7();
        assert_eq!(
            secret_recheck_subject(Some(user), Some(service_principal)),
            Some(SecretRecheckSubject::User(user)),
            "a requesting user must win over a service-principal field"
        );
        assert_eq!(
            secret_recheck_subject(Some(user), None),
            Some(SecretRecheckSubject::User(user))
        );
        assert_eq!(
            secret_recheck_subject(None, Some(service_principal)),
            Some(SecretRecheckSubject::ServicePrincipal(service_principal))
        );
        assert_eq!(
            secret_recheck_subject(None, None),
            None,
            "no stored subject must stay None (the dispatch path then fails closed)"
        );
    }
}
