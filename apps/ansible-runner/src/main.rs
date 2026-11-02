//! Ansible-runner sidecar binary: runs dispatched Ansible playbook jobs and streams runner events to the control plane.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use piping_contracts::playbook_runner::{
    JOB_KNOWN_HOSTS_FILE, RUNNER_CONTRACT_VERSION, RUNNER_PER_HOST_KEY_DIR, RunnerDispatchReceipt,
    RunnerDispatchStatus, RunnerEventKind, RunnerGroupCompletionBatch, RunnerGroupCompletionItem,
    RunnerGroupEvent, RunnerGroupEventBatch, RunnerJobRequest, RunnerJobState, RunnerJobTarget,
    RunnerProjectFile, RunnerRunStatus, pinned_host_key_options,
};
use regex::Regex;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, interval, sleep, timeout};
use tracing::{error, info, warn};
use uuid::Uuid;

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_DATA_DIR: &str = "/var/lib/piping-runner";
const DEFAULT_CALLBACK_TIMEOUT: u64 = 15;
const DEFAULT_EVENT_QUEUE_SIZE: usize = 50_000;
const DEFAULT_EVENT_BATCH_SIZE: usize = 1_000;
const DEFAULT_EVENT_BATCH_FLUSH_MS: u64 = 100;
// Generous so the post-job tail always flushes: at job end the sender is
// dropped and the remaining queued events (up to the full 50k channel) are
// flushed in 1k batches. A slow callback endpoint can make each batch take up
// to `callback_timeout`, so keep a wide bound — but still bounded, so a dead
// backend cannot hang shutdown forever.
const DEFAULT_EVENT_DRAIN_TIMEOUT: u64 = 120;
const DEFAULT_RESULT_FORCE_COMPACT_PAYLOAD_BYTES: usize = 4_000;
const DEFAULT_ANSIBLE_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_JOB_TIMEOUT_SECONDS: u64 = 3_600;
const DEFAULT_OUTPUT_DRAIN_GRACE_SECONDS: u64 = 10;
const DEFAULT_DISPATCH_RECEIPT_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;
const DISPATCH_RECEIPT_PRUNE_INTERVAL_SECONDS: u64 = 60 * 60;
const DEFAULT_PROJECT_ENTRYPOINT: &str = "play.yml";
const DISPATCH_RECEIPT_DIR: &str = "_dispatch_receipts";
/// Completion message for a target that never showed up in the recap or in any
/// per-task result line: a group-level `rc=0` proves nothing about that host.
const MISSING_TARGET_OUTPUT_MESSAGE: &str = "target never appeared in ansible output";
/// Exact parent environment variable names inherited by the spawned
/// `ansible-playbook` process. Everything else from the runner process
/// environment is dropped so playbooks cannot read control-plane secrets such
/// as `PIPING_ANSIBLE_RUNNER__SHARED_SECRET` via `lookup('env', ...)`.
const INHERITED_ENV_NAMES: &[&str] = &["PATH", "HOME", "LANG", "TZ", "TMPDIR"];
/// Parent environment variable prefixes inherited by the spawned
/// `ansible-playbook` process: image-level Ansible defaults (`ANSIBLE_HOME`,
/// `ANSIBLE_LOCAL_TEMP`, ...), Python runtime knobs and locale categories
/// (`LC_ALL`, `LC_CTYPE`, ...).
const INHERITED_ENV_PREFIXES: &[&str] = &["ANSIBLE_", "PYTHON", "LC_"];
const MAX_PROJECT_FILES: usize = 2_048;
const MAX_PROJECT_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROJECT_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Wall-clock budget for the whole `ansible-galaxy` dependency install step
/// (all requirements files combined). Galaxy shells out to git/https and can
/// hang against an unreachable server, so the step is reaped like the play.
const DEFAULT_GALAXY_INSTALL_TIMEOUT_SECONDS: u64 = 600;
/// System collection/role search paths baked into the execution-environment
/// image (piping.space + vendored community collections live here). Galaxy
/// deps installed per job are *prepended* to these so the play still resolves
/// the image-level content — the cache path never replaces the system path.
const GALAXY_SYSTEM_COLLECTIONS_PATH: &str = "/usr/share/ansible/collections";
const GALAXY_SYSTEM_ROLES_PATH: &str = "/usr/share/ansible/roles:/etc/ansible/roles";
/// Number of trailing galaxy output lines retained for the failure message so
/// an operator sees why an install failed without scrolling the full stream.
const GALAXY_ERROR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone)]
struct AppConfig {
    bind: String,
    shared_secret: String,
    data_dir: PathBuf,
    callback_timeout: Duration,
    event_queue_size: usize,
    event_batch_size: usize,
    event_batch_flush: Duration,
    event_drain_timeout: Duration,
    result_force_compact_payload_bytes: usize,
    keep_data_dir: bool,
    default_job_timeout: Duration,
    output_drain_grace: Duration,
    dispatch_receipt_retention: Duration,
    galaxy_cache_dir: PathBuf,
    galaxy_install_timeout: Duration,
}

impl AppConfig {
    fn from_env() -> anyhow::Result<Self> {
        let shared_secret = required_env("PIPING_ANSIBLE_RUNNER__SHARED_SECRET")?;
        let data_dir = PathBuf::from(env_string(
            "PIPING_ANSIBLE_RUNNER__DATA_DIR",
            DEFAULT_DATA_DIR,
        ));
        // Galaxy cache defaults to a sibling of the per-run private data dirs
        // (which are wiped per job) so installed collections/roles survive
        // between jobs. Operators can point it at a mounted volume for
        // cross-restart persistence in air-gapped fleets.
        let galaxy_cache_dir = match env::var("PIPING_ANSIBLE_RUNNER__GALAXY_CACHE_DIR") {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            _ => data_dir.join("_galaxy_cache"),
        };
        Ok(Self {
            bind: env_string("PIPING_ANSIBLE_RUNNER__BIND", DEFAULT_BIND),
            shared_secret,
            data_dir,
            callback_timeout: Duration::from_secs(env_u64(
                "PIPING_ANSIBLE_RUNNER__CALLBACK_TIMEOUT_S",
                DEFAULT_CALLBACK_TIMEOUT,
            )?),
            event_queue_size: env_usize(
                "PIPING_ANSIBLE_RUNNER__EVENT_QUEUE_SIZE",
                DEFAULT_EVENT_QUEUE_SIZE,
            )?,
            event_batch_size: env_usize(
                "PIPING_ANSIBLE_RUNNER__EVENT_BATCH_SIZE",
                DEFAULT_EVENT_BATCH_SIZE,
            )?
            .max(1),
            event_batch_flush: Duration::from_millis(env_u64(
                "PIPING_ANSIBLE_RUNNER__EVENT_BATCH_FLUSH_MS",
                DEFAULT_EVENT_BATCH_FLUSH_MS,
            )?),
            event_drain_timeout: Duration::from_secs(env_u64(
                "PIPING_ANSIBLE_RUNNER__EVENT_DRAIN_TIMEOUT_S",
                DEFAULT_EVENT_DRAIN_TIMEOUT,
            )?),
            result_force_compact_payload_bytes: env_usize(
                "PIPING_ANSIBLE_RUNNER__RESULT_FORCE_COMPACT_PAYLOAD_BYTES",
                DEFAULT_RESULT_FORCE_COMPACT_PAYLOAD_BYTES,
            )?,
            keep_data_dir: env_bool("PIPING_ANSIBLE_RUNNER__KEEP_DATA_DIR", false)?,
            default_job_timeout: Duration::from_secs(env_u64(
                "PIPING_ANSIBLE_RUNNER__DEFAULT_JOB_TIMEOUT_S",
                DEFAULT_JOB_TIMEOUT_SECONDS,
            )?),
            output_drain_grace: Duration::from_secs(env_u64(
                "PIPING_ANSIBLE_RUNNER__OUTPUT_DRAIN_GRACE_S",
                DEFAULT_OUTPUT_DRAIN_GRACE_SECONDS,
            )?),
            dispatch_receipt_retention: Duration::from_secs(env_u64(
                "PIPING_ANSIBLE_RUNNER__DISPATCH_RECEIPT_RETENTION_S",
                DEFAULT_DISPATCH_RECEIPT_RETENTION_SECONDS,
            )?),
            galaxy_cache_dir,
            galaxy_install_timeout: Duration::from_secs(env_u64(
                "PIPING_ANSIBLE_RUNNER__GALAXY_INSTALL_TIMEOUT_S",
                DEFAULT_GALAXY_INSTALL_TIMEOUT_SECONDS,
            )?),
        })
    }
}

#[derive(Debug)]
struct AppState {
    config: AppConfig,
    jobs: Mutex<HashMap<Uuid, JobControl>>,
}

#[derive(Debug, Clone)]
struct JobControl {
    receipt: RunnerDispatchReceipt,
    status: Arc<RwLock<RunnerJobState>>,
    rc: Arc<RwLock<Option<i32>>>,
    cancel_requested: Arc<AtomicBool>,
    process_id: Arc<RwLock<Option<u32>>>,
    terminal_transition_at_unix_s: Arc<RwLock<Option<u64>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedDispatch {
    #[serde(flatten)]
    receipt: RunnerDispatchReceipt,
    status: RunnerJobState,
    rc: Option<i32>,
    process_id: Option<u32>,
    #[serde(default)]
    terminal_transition_at_unix_s: Option<u64>,
}

#[derive(Debug, Error)]
enum RunnerError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Internal(String),
}

impl RunnerError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for RunnerError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({"detail": self.to_string()}));
        (status, body).into_response()
    }
}

// Every bool here mirrors one independent JSON knob of the typed runner
// settings contract 1:1; folding them into two-variant enums would obscure
// that wire mapping without making any invalid state unrepresentable.
#[allow(
    clippy::struct_excessive_bools,
    reason = "every bool mirrors one independent JSON knob of the typed runner settings contract 1:1; folding them into two-variant enums would obscure that wire mapping without making any invalid state unrepresentable"
)]
#[derive(Debug, Clone)]
struct RunnerSettings {
    forks: Option<u32>,
    verbosity: Option<u8>,
    job_timeout: Option<Duration>,
    ansible_timeout_seconds: Option<u64>,
    limit: Option<String>,
    tags: Option<String>,
    skip_tags: Option<String>,
    start_at_task: Option<String>,
    check: bool,
    diff: bool,
    compact_events: bool,
    raw_stdout_passthrough: bool,
    omit_event_data: bool,
    only_failed_event_data: bool,
    include_internal_events: bool,
    full_result_payload: bool,
    deprecation_warnings: Option<bool>,
    display_ok_hosts: Option<bool>,
    display_skipped_hosts: Option<bool>,
    pipelining: Option<bool>,
    python_interpreter: Option<String>,
    remote_tmp: Option<String>,
    shell_executable: Option<String>,
    ssh_args: Option<String>,
    ssh_common_args: Option<String>,
    strategy: Option<String>,
    stdout_callback: Option<String>,
}

impl RunnerSettings {
    fn parse(value: &Value) -> Result<Self, RunnerError> {
        let map = match value {
            Value::Null => Map::new(),
            Value::Object(map) => map.clone(),
            _ => {
                return Err(RunnerError::BadRequest(
                    "settings must be a JSON object".to_string(),
                ));
            }
        };

        if map.contains_key("cmdline") {
            return Err(RunnerError::BadRequest(
                "cmdline is intentionally unsupported; use typed settings".to_string(),
            ));
        }

        if let Some(backend) = optional_string(&map, "execution_backend")?
            && backend != "native"
        {
            return Err(RunnerError::BadRequest(format!(
                "execution_backend={backend:?} is not supported; the Rust runner executes native ansible-playbook only"
            )));
        }

        Ok(Self {
            forks: optional_u64(&map, "forks")?
                // The clamp bounds the value into u32 range, so the fallback
                // arm is unreachable; try_from keeps the conversion lossless
                // without a pedantic-flagged `as` cast.
                .map(|value| {
                    u32::try_from(value.clamp(1, u64::from(u32::MAX))).unwrap_or(u32::MAX)
                }),
            verbosity: optional_u64(&map, "verbosity")?.map(|value| value.min(6) as u8),
            job_timeout: optional_u64(&map, "job_timeout")?.map(Duration::from_secs),
            ansible_timeout_seconds: optional_u64(&map, "ansible_timeout")?,
            limit: optional_string(&map, "limit")?,
            tags: optional_string(&map, "tags")?,
            skip_tags: optional_string(&map, "skip_tags")?,
            start_at_task: optional_string(&map, "start_at_task")?,
            check: optional_bool(&map, "check")?.unwrap_or(false),
            diff: optional_bool(&map, "diff")?.unwrap_or(false),
            compact_events: optional_bool(&map, "compact_events")?.unwrap_or(false),
            raw_stdout_passthrough: optional_bool(&map, "raw_stdout_passthrough")?.unwrap_or(true),
            omit_event_data: optional_bool(&map, "omit_event_data")?.unwrap_or(false),
            only_failed_event_data: optional_bool(&map, "only_failed_event_data")?.unwrap_or(false),
            include_internal_events: optional_bool(&map, "include_internal_events")?
                .unwrap_or(false),
            full_result_payload: optional_bool(&map, "full_result_payload")?.unwrap_or(false),
            deprecation_warnings: optional_bool(&map, "deprecation_warnings")?,
            display_ok_hosts: optional_bool(&map, "display_ok_hosts")?,
            display_skipped_hosts: optional_bool(&map, "display_skipped_hosts")?,
            pipelining: optional_bool(&map, "pipelining")?,
            python_interpreter: optional_string(&map, "python_interpreter")?,
            remote_tmp: optional_string(&map, "remote_tmp")?,
            shell_executable: optional_string(&map, "shell_executable")?,
            ssh_args: optional_string(&map, "ssh_args")?,
            ssh_common_args: optional_string(&map, "ssh_common_args")?,
            strategy: optional_string(&map, "strategy")?,
            stdout_callback: optional_string(&map, "stdout_callback")?,
        })
    }
}

#[derive(Debug)]
struct PrivateDataDir {
    root: PathBuf,
    project: PathBuf,
    inventory: PathBuf,
    env: PathBuf,
    local_tmp: PathBuf,
    entrypoint: PathBuf,
    /// Job-level `known_hosts` seed file the pinned ssh options point at.
    known_hosts: PathBuf,
}

#[derive(Debug)]
struct SshAgentGuard {
    env: BTreeMap<String, String>,
}

impl Drop for SshAgentGuard {
    fn drop(&mut self) {
        let Some(pid) = self.env.get("SSH_AGENT_PID") else {
            return;
        };
        let mut command = StdCommand::new("ssh-agent");
        command.arg("-k").env("SSH_AGENT_PID", pid);
        if let Some(sock) = self.env.get("SSH_AUTH_SOCK") {
            command.env("SSH_AUTH_SOCK", sock);
        }
        let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

#[derive(Debug, Clone)]
struct EventEnvelope {
    event: RunnerGroupEvent,
}

struct EventDispatcher {
    sender: mpsc::Sender<EventEnvelope>,
    handle: JoinHandle<()>,
    dropped: Arc<AtomicU64>,
    drain_timeout: Duration,
}

impl EventDispatcher {
    fn new(job: &RunnerJobRequest, config: &AppConfig) -> anyhow::Result<Self> {
        let callback_base_url = Url::parse(&job.callback_base_url)
            .with_context(|| format!("invalid callback_base_url {}", job.callback_base_url))?;
        let client = reqwest::Client::builder()
            .timeout(config.callback_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .context("build callback HTTP client")?;
        let (sender, mut receiver) = mpsc::channel(config.event_queue_size.max(1));
        let batch_size = config.event_batch_size;
        let flush_every = config.event_batch_flush;
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_for_task = Arc::clone(&dropped);
        let group_callback_token = job.group_callback_token.clone();
        let run_group_id = job.run_group_id;
        let handle = tokio::spawn(async move {
            let mut batch: Vec<EventEnvelope> = Vec::with_capacity(batch_size.min(1024));
            let mut ticker = interval(flush_every);
            loop {
                tokio::select! {
                    Some(event) = receiver.recv() => {
                        batch.push(event);
                        if batch.len() >= batch_size {
                            flush_event_batch(&client, &callback_base_url, run_group_id, &group_callback_token, &mut batch).await;
                        }
                    }
                    _ = ticker.tick() => {
                        flush_event_batch(&client, &callback_base_url, run_group_id, &group_callback_token, &mut batch).await;
                    }
                    else => break,
                }
            }
            flush_event_batch(
                &client,
                &callback_base_url,
                run_group_id,
                &group_callback_token,
                &mut batch,
            )
            .await;
            let dropped = dropped_for_task.load(Ordering::Relaxed);
            if dropped > 0 {
                warn!(
                    dropped,
                    "runner dropped non-critical callback events under backpressure"
                );
            }
        });
        Ok(Self {
            sender,
            handle,
            dropped,
            drain_timeout: config.event_drain_timeout,
        })
    }

    async fn enqueue(&self, event: RunnerGroupEvent, must_keep: bool) {
        let envelope = EventEnvelope { event };
        if must_keep {
            if self.sender.send(envelope).await.is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        if self.sender.try_send(envelope).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    async fn finish(self) {
        drop(self.sender);
        let mut handle = self.handle;
        if timeout(self.drain_timeout, &mut handle).await.is_err() {
            warn!("timed out while draining ansible runner callback events");
            handle.abort();
            let _ = handle.await;
        }
    }
}

#[derive(Debug, Default)]
struct ParseState {
    current_task: Option<String>,
    suppress_next_continuation: bool,
    recap: HashMap<String, RecapStats>,
    observed: HashMap<String, ObservedOutcome>,
}

#[derive(Debug, Default, Clone, Copy)]
struct RecapStats {
    ok: u32,
    changed: u32,
    unreachable: u32,
    failed: u32,
    skipped: u32,
    rescued: u32,
    ignored: u32,
}

#[derive(Debug, Clone)]
struct ObservedOutcome {
    status: RunnerRunStatus,
    exit_code: i32,
    error_message: Option<String>,
}

#[derive(Debug)]
enum ProcessTerminalStatus {
    Exited(Option<i32>),
    Canceled,
    TimedOut,
    LaunchFailed(String),
}

fn dispatch_receipt_dir(config: &AppConfig) -> PathBuf {
    config.data_dir.join(DISPATCH_RECEIPT_DIR)
}

fn dispatch_receipt_path(config: &AppConfig, dispatch_id: Uuid) -> PathBuf {
    dispatch_receipt_dir(config).join(format!("{dispatch_id}.json"))
}

fn persist_dispatch(config: &AppConfig, dispatch: &PersistedDispatch) -> anyhow::Result<()> {
    let directory = dispatch_receipt_dir(config);
    fs::create_dir_all(&directory)
        .with_context(|| format!("create dispatch receipt directory {}", directory.display()))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("protect dispatch receipt directory {}", directory.display()))?;

    let final_path = dispatch_receipt_path(config, dispatch.receipt.dispatch_id);
    let temporary_path = directory.join(format!(
        ".{}.{}.tmp",
        dispatch.receipt.dispatch_id,
        Uuid::now_v7()
    ));
    let bytes = serde_json::to_vec(dispatch).context("serialize dispatch receipt")?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary_path)
        .with_context(|| format!("create dispatch receipt {}", temporary_path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("write dispatch receipt {}", temporary_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync dispatch receipt {}", temporary_path.display()))?;
    fs::rename(&temporary_path, &final_path).with_context(|| {
        format!(
            "commit dispatch receipt {} as {}",
            temporary_path.display(),
            final_path.display()
        )
    })?;
    FileSync::sync_directory(&directory)?;
    Ok(())
}

struct FileSync;

impl FileSync {
    fn sync_directory(path: &Path) -> anyhow::Result<()> {
        let directory = fs::File::open(path)
            .with_context(|| format!("open dispatch receipt directory {}", path.display()))?;
        directory
            .sync_all()
            .with_context(|| format!("sync dispatch receipt directory {}", path.display()))
    }
}

fn runner_job_state_is_terminal(status: RunnerJobState) -> bool {
    matches!(
        status,
        RunnerJobState::Succeeded
            | RunnerJobState::Failed
            | RunnerJobState::TimedOut
            | RunnerJobState::Canceled
    )
}

fn current_unix_time_seconds() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")
        .map(|duration| duration.as_secs())
}

fn dispatch_receipt_is_expired(
    dispatch: &PersistedDispatch,
    file_modified_at: Option<SystemTime>,
    now: SystemTime,
    retention: Duration,
) -> bool {
    if !runner_job_state_is_terminal(dispatch.status) {
        return false;
    }

    let transitioned_at = dispatch
        .terminal_transition_at_unix_s
        .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
        .or(file_modified_at);
    transitioned_at
        .and_then(|transitioned_at| now.duration_since(transitioned_at).ok())
        .is_some_and(|age| age > retention)
}

enum DispatchReceiptEntry {
    Ignored,
    Pruned,
    Retained(PersistedDispatch),
}

fn read_dispatch_receipt_entry(
    config: &AppConfig,
    entry: fs::DirEntry,
    now: SystemTime,
) -> anyhow::Result<DispatchReceiptEntry> {
    let path = entry.path();
    if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return Ok(DispatchReceiptEntry::Ignored);
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("read persisted dispatch receipt {}", path.display()))?;
    let dispatch: PersistedDispatch = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse persisted dispatch receipt {}", path.display()))?;
    let file_modified_at = entry
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    if !dispatch_receipt_is_expired(
        &dispatch,
        file_modified_at,
        now,
        config.dispatch_receipt_retention,
    ) {
        return Ok(DispatchReceiptEntry::Retained(dispatch));
    }
    fs::remove_file(&path)
        .with_context(|| format!("remove expired dispatch receipt {}", path.display()))?;
    Ok(DispatchReceiptEntry::Pruned)
}

fn finish_dispatch_receipt_prune(
    config: &AppConfig,
    directory: &Path,
    pruned_receipts: u64,
) -> anyhow::Result<()> {
    if pruned_receipts == 0 {
        return Ok(());
    }
    FileSync::sync_directory(directory)?;
    info!(
        pruned_receipts,
        retention_seconds = config.dispatch_receipt_retention.as_secs(),
        "pruned expired terminal dispatch receipts"
    );
    Ok(())
}

fn prune_expired_dispatch_receipts(config: &AppConfig) -> anyhow::Result<()> {
    let directory = dispatch_receipt_dir(config);
    let now = SystemTime::now();
    let mut pruned_receipts = 0_u64;
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("read dispatch receipt directory {}", directory.display()))?
    {
        let entry = entry.context("read dispatch receipt entry")?;
        if matches!(
            read_dispatch_receipt_entry(config, entry, now)?,
            DispatchReceiptEntry::Pruned
        ) {
            pruned_receipts += 1;
        }
    }
    finish_dispatch_receipt_prune(config, &directory, pruned_receipts)
}

fn spawn_dispatch_receipt_pruner(config: AppConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut prune_interval =
            interval(Duration::from_secs(DISPATCH_RECEIPT_PRUNE_INTERVAL_SECONDS));
        // Startup already prunes while loading, so the first background sweep
        // waits for one complete interval instead of immediately scanning twice.
        prune_interval.tick().await;
        loop {
            prune_interval.tick().await;
            let config = config.clone();
            match tokio::task::spawn_blocking(move || prune_expired_dispatch_receipts(&config))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(error = %error, "failed to prune expired dispatch receipts");
                }
                Err(error) => {
                    warn!(error = %error, "dispatch receipt pruning task failed");
                }
            }
        }
    })
}

fn process_is_alive(process_id: u32) -> bool {
    i32::try_from(process_id)
        .ok()
        .is_some_and(|pid| kill(Pid::from_raw(pid), None).is_ok())
}

fn load_persisted_dispatches(config: &AppConfig) -> anyhow::Result<HashMap<Uuid, JobControl>> {
    let directory = dispatch_receipt_dir(config);
    fs::create_dir_all(&directory)
        .with_context(|| format!("create dispatch receipt directory {}", directory.display()))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("protect dispatch receipt directory {}", directory.display()))?;

    let mut jobs = HashMap::new();
    let now = SystemTime::now();
    let mut pruned_receipts = 0_u64;
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("read dispatch receipt directory {}", directory.display()))?
    {
        let entry = entry.context("read dispatch receipt entry")?;
        let mut dispatch = match read_dispatch_receipt_entry(config, entry, now)? {
            DispatchReceiptEntry::Ignored => continue,
            DispatchReceiptEntry::Pruned => {
                pruned_receipts += 1;
                continue;
            }
            DispatchReceiptEntry::Retained(dispatch) => dispatch,
        };

        if !runner_job_state_is_terminal(dispatch.status)
            && !dispatch.process_id.is_some_and(process_is_alive)
        {
            // Acceptance remains durable, so this dispatch identity can never
            // spawn again. A missing process after restart is terminally failed
            // rather than silently forgotten or replayed with stale credentials.
            dispatch.status = RunnerJobState::Failed;
            dispatch.process_id = None;
            dispatch.terminal_transition_at_unix_s = Some(current_unix_time_seconds()?);
            persist_dispatch(config, &dispatch)?;
        }

        jobs.insert(
            dispatch.receipt.dispatch_id,
            JobControl {
                receipt: dispatch.receipt,
                status: Arc::new(RwLock::new(dispatch.status)),
                rc: Arc::new(RwLock::new(dispatch.rc)),
                cancel_requested: Arc::new(AtomicBool::new(false)),
                process_id: Arc::new(RwLock::new(dispatch.process_id)),
                terminal_transition_at_unix_s: Arc::new(RwLock::new(
                    dispatch.terminal_transition_at_unix_s,
                )),
            },
        );
    }
    finish_dispatch_receipt_prune(config, &directory, pruned_receipts)?;
    Ok(jobs)
}

async fn persist_job_control(config: &AppConfig, control: &JobControl) -> anyhow::Result<()> {
    let status = *control.status.read().await;
    let terminal_transition_at_unix_s = {
        let mut transitioned_at = control.terminal_transition_at_unix_s.write().await;
        if runner_job_state_is_terminal(status) {
            if transitioned_at.is_none() {
                *transitioned_at = Some(current_unix_time_seconds()?);
            }
        } else {
            *transitioned_at = None;
        }
        *transitioned_at
    };
    persist_dispatch(
        config,
        &PersistedDispatch {
            receipt: control.receipt.clone(),
            status,
            rc: *control.rc.read().await,
            process_id: *control.process_id.read().await,
            terminal_transition_at_unix_s,
        },
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if env::args().any(|arg| arg == "--health-check") {
        return run_health_check().await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "piping_ansible_runner=info,tower_http=warn".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    fs::create_dir_all(&config.data_dir).with_context(|| {
        format!(
            "create ansible runner data dir {}",
            config.data_dir.display()
        )
    })?;
    let jobs = load_persisted_dispatches(&config)?;
    let state = Arc::new(AppState {
        config,
        jobs: Mutex::new(jobs),
    });
    let _dispatch_receipt_pruner = spawn_dispatch_receipt_pruner(state.config.clone());

    let router = Router::new()
        .route("/health", get(health))
        .route("/jobs", post(create_job))
        .route("/dispatches/{dispatch_id}", get(get_dispatch))
        .route("/dispatches/{dispatch_id}/cancel", post(cancel_dispatch))
        .with_state(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind(&state.config.bind)
        .await
        .with_context(|| format!("bind {}", state.config.bind))?;
    info!(bind = %state.config.bind, "starting Rust ansible runner sidecar");
    axum::serve(listener, router).await.context("serve HTTP")?;
    Ok(())
}

async fn run_health_check() -> anyhow::Result<()> {
    let bind = env_string("PIPING_ANSIBLE_RUNNER__BIND", DEFAULT_BIND);
    let endpoint = format!("http://{}/health", bind.replace("0.0.0.0", "127.0.0.1"));
    let response = reqwest::get(endpoint)
        .await
        .context("health request failed")?;
    if !response.status().is_success() {
        bail!("health check returned {}", response.status());
    }
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(job): Json<RunnerJobRequest>,
) -> Result<(StatusCode, Json<RunnerDispatchReceipt>), RunnerError> {
    require_auth(&headers, &state.config.shared_secret)?;
    let receipt = RunnerDispatchReceipt {
        dispatch_id: job.dispatch_id,
        run_group_id: job.run_group_id,
        dispatch_attempt: job.dispatch_attempt,
    };

    let mut jobs = state.jobs.lock().await;
    if let Some(existing) = jobs.get(&job.dispatch_id) {
        if existing.receipt != receipt {
            return Err(RunnerError::Conflict(format!(
                "dispatch {} is already bound to a different run group or attempt",
                job.dispatch_id
            )));
        }
        return Ok((StatusCode::ACCEPTED, Json(existing.receipt.clone())));
    }
    for existing in jobs.values() {
        if existing.receipt.run_group_id == job.run_group_id
            && !runner_job_state_is_terminal(*existing.status.read().await)
        {
            return Err(RunnerError::Conflict(format!(
                "run group {} already has an accepted active dispatch",
                job.run_group_id
            )));
        }
    }
    validate_job(&job)?;
    let settings = RunnerSettings::parse(&job.settings)?;

    let status = Arc::new(RwLock::new(RunnerJobState::Accepted));
    let rc = Arc::new(RwLock::new(None));
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let process_id = Arc::new(RwLock::new(None));
    let control = JobControl {
        receipt: receipt.clone(),
        status: Arc::clone(&status),
        rc: Arc::clone(&rc),
        cancel_requested: Arc::clone(&cancel_requested),
        process_id: Arc::clone(&process_id),
        terminal_transition_at_unix_s: Arc::new(RwLock::new(None)),
    };
    persist_job_control(&state.config, &control)
        .await
        .map_err(|error| RunnerError::Internal(format!("persist dispatch receipt: {error:#}")))?;
    jobs.insert(job.dispatch_id, control.clone());
    drop(jobs);

    let run_group_id = job.run_group_id;
    let state_for_task = Arc::clone(&state);
    tokio::spawn(async move {
        let final_status = execute_job(Arc::clone(&state_for_task), job, settings, control).await;
        if let Err(error) = &final_status {
            error!(run_group_id = %run_group_id, error = %error, "ansible runner job failed during startup; a failed completion callback was attempted");
        }
    });

    Ok((StatusCode::ACCEPTED, Json(receipt)))
}

async fn get_dispatch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(dispatch_id): AxumPath<Uuid>,
) -> Result<Json<RunnerDispatchStatus>, RunnerError> {
    require_auth(&headers, &state.config.shared_secret)?;
    let job = {
        let jobs = state.jobs.lock().await;
        jobs.get(&dispatch_id).cloned()
    }
    .ok_or_else(|| RunnerError::NotFound(format!("dispatch {dispatch_id} was never accepted")))?;
    let current_status = *job.status.read().await;
    let process_id = *job.process_id.read().await;
    if matches!(
        current_status,
        RunnerJobState::Running | RunnerJobState::Canceling
    ) && process_id.is_some_and(|pid| !process_is_alive(pid))
    {
        *job.status.write().await = if current_status == RunnerJobState::Canceling {
            RunnerJobState::Canceled
        } else {
            RunnerJobState::Failed
        };
        *job.process_id.write().await = None;
        persist_job_control(&state.config, &job)
            .await
            .map_err(|error| {
                RunnerError::Internal(format!("persist reconciled dispatch status: {error:#}"))
            })?;
    }
    Ok(Json(RunnerDispatchStatus {
        receipt: job.receipt,
        status: *job.status.read().await,
        rc: *job.rc.read().await,
    }))
}

async fn cancel_dispatch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(dispatch_id): AxumPath<Uuid>,
) -> Result<Json<RunnerDispatchStatus>, RunnerError> {
    require_auth(&headers, &state.config.shared_secret)?;
    let job = {
        let jobs = state.jobs.lock().await;
        jobs.get(&dispatch_id).cloned()
    }
    .ok_or_else(|| RunnerError::NotFound(format!("dispatch {dispatch_id} was never accepted")))?;

    if !runner_job_state_is_terminal(*job.status.read().await) {
        job.cancel_requested.store(true, Ordering::SeqCst);
        *job.status.write().await = RunnerJobState::Canceling;
        persist_job_control(&state.config, &job)
            .await
            .map_err(|error| {
                RunnerError::Internal(format!("persist cancellation state: {error:#}"))
            })?;
        if let Some(pid) = *job.process_id.read().await {
            terminate_process_group(pid).await;
        }
    }

    Ok(Json(RunnerDispatchStatus {
        receipt: job.receipt,
        status: *job.status.read().await,
        rc: *job.rc.read().await,
    }))
}

/// Terminal completion fallback for a job whose targets produced no parse
/// evidence: the run-level status, exit code and error message. Bundled so the
/// completion helpers stay within the argument budget.
struct CompletionFallback<'a> {
    status: RunnerRunStatus,
    exit_code: Option<i32>,
    error_message: Option<&'a str>,
}

/// Mark a pre-launch job failure on the control state and flush the terminal
/// completion batch. Used once the dispatcher exists; the caller owns the
/// data-dir cleanup that follows.
async fn fail_job_after_launch_error(
    config: &AppConfig,
    job: &RunnerJobRequest,
    control: &JobControl,
    parse_state: &ParseState,
    message: &str,
    dispatcher: EventDispatcher,
) -> anyhow::Result<()> {
    emit_launch_error(&dispatcher, job, message).await;
    *control.status.write().await = RunnerJobState::Failed;
    persist_job_control(config, control).await?;
    drain_events_before_completion(
        config,
        job,
        parse_state,
        CompletionFallback {
            status: RunnerRunStatus::Failed,
            exit_code: None,
            error_message: Some(message),
        },
        dispatcher,
    )
    .await;
    Ok(())
}

/// Mark a job failed and flush its terminal completion batch when the callback
/// dispatcher could not even be constructed (so no events can be emitted).
async fn fail_job_without_dispatcher(
    config: &AppConfig,
    job: &RunnerJobRequest,
    control: &JobControl,
    parse_state: &ParseState,
    message: &str,
) -> anyhow::Result<()> {
    *control.status.write().await = RunnerJobState::Failed;
    persist_job_control(config, control).await?;
    send_completion_batch(
        config,
        job,
        parse_state,
        RunnerRunStatus::Failed,
        None,
        Some(message),
    )
    .await;
    Ok(())
}

async fn start_job_execution(
    config: &AppConfig,
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    control: &JobControl,
    parse_state: &ParseState,
) -> anyhow::Result<(EventDispatcher, PrivateDataDir, Option<SshAgentGuard>)> {
    let dispatcher = match EventDispatcher::new(job, config) {
        Ok(dispatcher) => dispatcher,
        Err(error) => {
            let message = format!("Failed to initialize runner callback dispatcher: {error:#}");
            fail_job_without_dispatcher(config, job, control, parse_state, &message).await?;
            return Err(error);
        }
    };
    match prepare_job_execution(config, job, settings) {
        Ok((pdd, ssh_agent)) => Ok((dispatcher, pdd, ssh_agent)),
        Err(error) => {
            // A pre-launch failure still needs a terminal completion callback.
            let message = format!("Failed to prepare ansible job: {error:#}");
            fail_job_after_launch_error(config, job, control, parse_state, &message, dispatcher)
                .await?;
            remove_job_data_dir(config, job.run_group_id);
            Err(error)
        }
    }
}

async fn begin_job(
    config: &AppConfig,
    job: &RunnerJobRequest,
    control: &JobControl,
    parse_state: &ParseState,
) -> anyhow::Result<bool> {
    if control.cancel_requested.load(Ordering::SeqCst) {
        *control.status.write().await = RunnerJobState::Canceled;
        persist_job_control(config, control).await?;
        send_completion_batch(
            config,
            job,
            parse_state,
            RunnerRunStatus::Canceled,
            None,
            Some("Run was canceled by operator"),
        )
        .await;
        return Ok(false);
    }
    *control.status.write().await = RunnerJobState::Running;
    persist_job_control(config, control).await?;
    Ok(true)
}

async fn execute_job(
    state: Arc<AppState>,
    job: RunnerJobRequest,
    settings: RunnerSettings,
    control: JobControl,
) -> anyhow::Result<()> {
    let mut parse_state = ParseState::default();
    if !begin_job(&state.config, &job, &control, &parse_state).await? {
        return Ok(());
    }
    let cancel_requested = Arc::clone(&control.cancel_requested);
    let process_id = Arc::clone(&control.process_id);
    let (dispatcher, pdd, ssh_agent) =
        start_job_execution(&state.config, &job, &settings, &control, &parse_state).await?;

    // Install any project-declared Galaxy requirements before the play. A
    // failure here (unreachable server, bad requirements) must still emit a
    // terminal completion — the same reason `prepare_job_execution` does.
    let galaxy = if cancel_requested.load(Ordering::SeqCst) {
        None
    } else {
        match install_galaxy_requirements(&state.config, &job, &pdd, &dispatcher).await {
            Ok(galaxy) => galaxy,
            Err(error) => {
                let message = format!("Failed to install Ansible Galaxy requirements: {error:#}");
                fail_job_after_launch_error(
                    &state.config,
                    &job,
                    &control,
                    &parse_state,
                    &message,
                    dispatcher,
                )
                .await?;
                drop(ssh_agent);
                cleanup_private_data_dir(&state.config, &pdd);
                return Err(error);
            }
        }
    };

    let terminal_status = run_prepared_job(ProcessRunContext {
        job: &job,
        settings: &settings,
        config: &state.config,
        pdd: &pdd,
        ssh_agent: ssh_agent.as_ref(),
        galaxy: galaxy.as_ref(),
        dispatcher: &dispatcher,
        parse_state: &mut parse_state,
        cancel_requested: Arc::clone(&cancel_requested),
        process_id: Arc::clone(&process_id),
    })
    .await;

    let (job_state, run_status, exit_code, error_message) = terminal_outcome(terminal_status);

    *control.status.write().await = job_state;
    *control.rc.write().await = exit_code;
    *process_id.write().await = None;
    persist_job_control(&state.config, &control).await?;

    drain_events_before_completion(
        &state.config,
        &job,
        &parse_state,
        CompletionFallback {
            status: run_status,
            exit_code,
            error_message: error_message.as_deref(),
        },
        dispatcher,
    )
    .await;

    drop(ssh_agent);
    cleanup_private_data_dir(&state.config, &pdd);
    Ok(())
}

async fn drain_events_before_completion(
    config: &AppConfig,
    job: &RunnerJobRequest,
    parse_state: &ParseState,
    fallback: CompletionFallback<'_>,
    dispatcher: EventDispatcher,
) {
    // Every event POST receives its backend acknowledgement before completion
    // revokes the dedicated group callback token. This removes the late-event
    // authority window instead of preserving it in backend token validation.
    dispatcher.finish().await;
    send_completion_batch(
        config,
        job,
        parse_state,
        fallback.status,
        fallback.exit_code,
        fallback.error_message,
    )
    .await;
}

/// Announce the dispatched job on the group stream: a typed status event plus
/// a human console line summarizing the effective launch parameters.
async fn announce_job_start(
    dispatcher: &EventDispatcher,
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
) {
    send_group_status(
        dispatcher,
        job,
        "ansible_job_dispatched",
        Some("Ansible job dispatched; waiting for runner events".to_string()),
        json!({"backend": "native", "contract_version": RUNNER_CONTRACT_VERSION}),
    )
    .await;
    send_group_console_line(
        dispatcher,
        job,
        format!(
            "Starting ansible-playbook: {} targets, forks={}, strategy={}, ssh_timeout={}s, connect_timeout={}",
            job.targets.len().max(1),
            settings
                .forks
                .map_or_else(|| "default".to_string(), |forks| forks.to_string()),
            settings.strategy.as_deref().unwrap_or("default"),
            settings
                .ansible_timeout_seconds
                .unwrap_or(DEFAULT_ANSIBLE_TIMEOUT_SECONDS),
            ssh_connect_timeout_label(settings),
        ),
    )
    .await;
}

/// Map the process-level terminal status onto the `(job state, run status,
/// exit code, error message)` tuple reported to the backend.
fn terminal_outcome(
    terminal_status: ProcessTerminalStatus,
) -> (RunnerJobState, RunnerRunStatus, Option<i32>, Option<String>) {
    match terminal_status {
        ProcessTerminalStatus::Exited(code) if code.unwrap_or(1) == 0 => (
            RunnerJobState::Succeeded,
            RunnerRunStatus::Succeeded,
            code,
            None,
        ),
        ProcessTerminalStatus::Exited(code) => (
            RunnerJobState::Failed,
            RunnerRunStatus::Failed,
            code,
            Some(format!(
                "ansible-playbook status=failed{}",
                code.map(|rc| format!(" rc={rc}")).unwrap_or_default()
            )),
        ),
        ProcessTerminalStatus::Canceled => (
            RunnerJobState::Canceled,
            RunnerRunStatus::Canceled,
            None,
            Some("Run was canceled by operator".to_string()),
        ),
        ProcessTerminalStatus::TimedOut => (
            RunnerJobState::TimedOut,
            RunnerRunStatus::TimedOut,
            None,
            Some("Run exceeded job_timeout and was reaped by the executor".to_string()),
        ),
        ProcessTerminalStatus::LaunchFailed(message) => (
            RunnerJobState::Failed,
            RunnerRunStatus::Failed,
            None,
            Some(message),
        ),
    }
}

/// Materialize the private data dir and seed the ephemeral ssh-agent.
///
/// Split out of [`execute_job`] so every pre-launch failure funnels through a
/// single completion-reporting error path instead of aborting the job task.
fn prepare_job_execution(
    config: &AppConfig,
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
) -> anyhow::Result<(PrivateDataDir, Option<SshAgentGuard>)> {
    let pdd = materialize_private_data_dir(config, job, settings)?;
    let ssh_agent = match job.ssh_key.as_deref() {
        Some(key) if !key.trim().is_empty() => {
            Some(start_ssh_agent(key).context("start ssh-agent")?)
        }
        _ => None,
    };
    Ok((pdd, ssh_agent))
}

/// Best-effort removal of a (possibly partially materialized) private data dir
/// after an early failure, before a [`PrivateDataDir`] handle exists.
fn remove_job_data_dir(config: &AppConfig, run_group_id: Uuid) {
    if config.keep_data_dir {
        return;
    }
    let root = config.data_dir.join(run_group_id.to_string());
    if !root.exists() {
        return;
    }
    if let Err(error) = fs::remove_dir_all(&root) {
        warn!(path = %root.display(), error = %error, "failed to remove ansible private data dir after early failure");
    }
}

struct ProcessRunContext<'a> {
    job: &'a RunnerJobRequest,
    settings: &'a RunnerSettings,
    config: &'a AppConfig,
    pdd: &'a PrivateDataDir,
    ssh_agent: Option<&'a SshAgentGuard>,
    galaxy: Option<&'a GalaxyPaths>,
    dispatcher: &'a EventDispatcher,
    parse_state: &'a mut ParseState,
    cancel_requested: Arc<AtomicBool>,
    process_id: Arc<RwLock<Option<u32>>>,
}

async fn run_prepared_job(context: ProcessRunContext<'_>) -> ProcessTerminalStatus {
    if context.cancel_requested.load(Ordering::SeqCst) {
        return ProcessTerminalStatus::Canceled;
    }
    announce_job_start(context.dispatcher, context.job, context.settings).await;
    run_ansible_process(context).await
}

async fn run_ansible_process(context: ProcessRunContext<'_>) -> ProcessTerminalStatus {
    let ProcessRunContext {
        job,
        settings,
        config,
        pdd,
        ssh_agent,
        galaxy,
        dispatcher,
        parse_state,
        cancel_requested,
        process_id,
    } = context;
    let mut command = build_ansible_command(job, settings, pdd);
    command.current_dir(&pdd.project);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Never leak the runner process environment (shared secret, config knobs)
    // into user-authored playbooks: start from a clean slate and pass an
    // explicit allowlist plus the typed per-job environment.
    command.env_clear();
    command.envs(build_process_env(
        env::vars_os(),
        job,
        settings,
        pdd,
        ssh_agent.map(|agent| &agent.env),
        galaxy,
    ));
    configure_process_group(&mut command);

    if cancel_requested.load(Ordering::SeqCst) {
        return ProcessTerminalStatus::Canceled;
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("Failed to launch ansible-playbook: {error}");
            emit_launch_error(dispatcher, job, &message).await;
            return ProcessTerminalStatus::LaunchFailed(message);
        }
    };

    supervise_child(
        child,
        ChildSupervisionContext {
            job,
            settings,
            config,
            dispatcher,
            parse_state,
            cancel_requested,
            process_id,
        },
    )
    .await
}

struct ChildSupervisionContext<'a> {
    job: &'a RunnerJobRequest,
    settings: &'a RunnerSettings,
    config: &'a AppConfig,
    dispatcher: &'a EventDispatcher,
    parse_state: &'a mut ParseState,
    cancel_requested: Arc<AtomicBool>,
    process_id: Arc<RwLock<Option<u32>>>,
}

/// Persist the OS process identity of a freshly spawned child so a later
/// reconcile pass can detect a runner that died without reporting.
async fn record_running_process_identity(
    job: &RunnerJobRequest,
    config: &AppConfig,
    pid: u32,
    process_id: &RwLock<Option<u32>>,
) {
    *process_id.write().await = Some(pid);
    let dispatch = PersistedDispatch {
        receipt: RunnerDispatchReceipt {
            dispatch_id: job.dispatch_id,
            run_group_id: job.run_group_id,
            dispatch_attempt: job.dispatch_attempt,
        },
        status: RunnerJobState::Running,
        rc: None,
        process_id: Some(pid),
        terminal_transition_at_unix_s: None,
    };
    if let Err(error) = persist_dispatch(config, &dispatch) {
        warn!(
            dispatch_id = %job.dispatch_id,
            error = %error,
            "failed to persist running dispatch process identity"
        );
    }
}

/// Take the child's stdout and stderr pipes, or report which capture is missing.
fn take_child_pipes(
    child: &mut tokio::process::Child,
) -> Result<(tokio::process::ChildStdout, tokio::process::ChildStderr), ProcessTerminalStatus> {
    let Some(stdout) = child.stdout.take() else {
        return Err(ProcessTerminalStatus::LaunchFailed(
            "ansible-playbook stdout pipe was not captured".to_string(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        return Err(ProcessTerminalStatus::LaunchFailed(
            "ansible-playbook stderr pipe was not captured".to_string(),
        ));
    };
    Ok((stdout, stderr))
}

/// Decision for one supervision poll tick, kept pure so the select loop owns
/// only the side effects.
#[derive(Clone, Copy)]
enum SupervisionTick {
    Running,
    CancelRequested,
    DeadlineExceeded,
    ForceCloseOutput,
}

fn supervision_tick(
    terminal_status: Option<&ProcessTerminalStatus>,
    cancel_requested: bool,
    past_deadline: bool,
    output_closed: bool,
    drain_expired: bool,
) -> SupervisionTick {
    if terminal_status.is_none() {
        if cancel_requested {
            return SupervisionTick::CancelRequested;
        }
        if past_deadline {
            return SupervisionTick::DeadlineExceeded;
        }
        return SupervisionTick::Running;
    }
    if !output_closed && drain_expired {
        return SupervisionTick::ForceCloseOutput;
    }
    SupervisionTick::Running
}

#[derive(Default)]
struct OutputProgress {
    seen: bool,
    closed: bool,
}

struct ChildOutputContext<'a> {
    job: &'a RunnerJobRequest,
    settings: &'a RunnerSettings,
    config: &'a AppConfig,
    parse_state: &'a mut ParseState,
    dispatcher: &'a EventDispatcher,
}

async fn handle_child_output(
    received: Option<(OutputStream, String)>,
    context: ChildOutputContext<'_>,
    progress: &mut OutputProgress,
) {
    if let Some((stream, line)) = received {
        progress.seen = true;
        forward_child_output_line(
            context.job,
            context.settings,
            context.config,
            context.parse_state,
            context.dispatcher,
            ChildOutputLine {
                stream,
                line: &line,
            },
        )
        .await;
    } else {
        progress.closed = true;
    }
}

async fn interrupted_child_status(
    tick: SupervisionTick,
    process_id: &RwLock<Option<u32>>,
) -> Option<ProcessTerminalStatus> {
    let status = match tick {
        SupervisionTick::CancelRequested => ProcessTerminalStatus::Canceled,
        SupervisionTick::DeadlineExceeded => ProcessTerminalStatus::TimedOut,
        SupervisionTick::ForceCloseOutput | SupervisionTick::Running => return None,
    };
    if let Some(pid) = *process_id.read().await {
        terminate_process_group(pid).await;
    }
    Some(status)
}

fn child_wait_status(status: std::io::Result<std::process::ExitStatus>) -> ProcessTerminalStatus {
    match status {
        Ok(exit) => ProcessTerminalStatus::Exited(exit.code()),
        Err(error) => {
            ProcessTerminalStatus::LaunchFailed(format!("ansible-playbook wait failed: {error}"))
        }
    }
}

fn completed_child_status(
    progress: &OutputProgress,
    terminal_status: &mut Option<ProcessTerminalStatus>,
) -> Option<ProcessTerminalStatus> {
    if progress.closed {
        terminal_status.take()
    } else {
        None
    }
}

struct SupervisionPollContext<'a> {
    terminal_status: Option<&'a ProcessTerminalStatus>,
    cancel_requested: &'a AtomicBool,
    deadline: Instant,
    output_drain_deadline: Option<Instant>,
    job: &'a RunnerJobRequest,
    config: &'a AppConfig,
    dispatcher: &'a EventDispatcher,
    process_id: &'a RwLock<Option<u32>>,
    stdout_reader: &'a JoinHandle<()>,
    stderr_reader: &'a JoinHandle<()>,
    progress: &'a mut OutputProgress,
}

async fn handle_supervision_poll(
    context: SupervisionPollContext<'_>,
) -> Option<ProcessTerminalStatus> {
    let tick = supervision_tick(
        context.terminal_status,
        context.cancel_requested.load(Ordering::SeqCst),
        Instant::now() >= context.deadline,
        context.progress.closed,
        context
            .output_drain_deadline
            .is_some_and(|drain| Instant::now() >= drain),
    );
    if let Some(status) = interrupted_child_status(tick, context.process_id).await {
        return Some(status);
    }
    if matches!(tick, SupervisionTick::ForceCloseOutput) {
        announce_forced_output_close(context.dispatcher, context.job, context.config).await;
        context.stdout_reader.abort();
        context.stderr_reader.abort();
        context.progress.closed = true;
    }
    None
}

struct OutputSelectContext<'a, W> {
    wait: &'a mut W,
    line_receiver: &'a mut mpsc::Receiver<(OutputStream, String)>,
    poll_cancel: &'a mut Interval,
    first_output_notice: &'a mut Interval,
    progress: &'a mut OutputProgress,
    terminal_status: &'a mut Option<ProcessTerminalStatus>,
    output_drain_deadline: &'a mut Option<Instant>,
    deadline: Instant,
    started_at: Instant,
    stdout_reader: &'a JoinHandle<()>,
    stderr_reader: &'a JoinHandle<()>,
    cancel_requested: &'a AtomicBool,
    process_id: &'a RwLock<Option<u32>>,
    output: ChildOutputContext<'a>,
}

async fn select_child_output_event<W>(
    context: OutputSelectContext<'_, W>,
) -> Option<ProcessTerminalStatus>
where
    W: Future<Output = std::io::Result<std::process::ExitStatus>> + Unpin,
{
    let OutputSelectContext {
        wait,
        line_receiver,
        poll_cancel,
        first_output_notice,
        progress,
        terminal_status,
        output_drain_deadline,
        deadline,
        started_at,
        stdout_reader,
        stderr_reader,
        cancel_requested,
        process_id,
        output,
    } = context;
    let ChildOutputContext {
        job,
        settings,
        config,
        parse_state,
        dispatcher,
    } = output;

    tokio::select! {
        received = line_receiver.recv(), if !progress.closed => {
            handle_child_output(
                received,
                ChildOutputContext { job, settings, config, parse_state, dispatcher },
                progress,
            ).await;
            None
        }
        status = wait, if terminal_status.is_none() => {
            *terminal_status = Some(child_wait_status(status));
            *output_drain_deadline = Some(Instant::now() + config.output_drain_grace);
            None
        }
        _ = first_output_notice.tick(), if !progress.seen && terminal_status.is_none() => {
            announce_waiting_for_first_output(dispatcher, job, settings, started_at).await;
            None
        }
        _ = poll_cancel.tick() => {
            handle_supervision_poll(SupervisionPollContext {
                terminal_status: terminal_status.as_ref(),
                cancel_requested,
                deadline,
                output_drain_deadline: *output_drain_deadline,
                job,
                config,
                dispatcher,
                process_id,
                stdout_reader,
                stderr_reader,
                progress,
            }).await
        }
    }
}

async fn supervise_child(
    mut child: tokio::process::Child,
    context: ChildSupervisionContext<'_>,
) -> ProcessTerminalStatus {
    if let Some(pid) = child.id() {
        record_running_process_identity(context.job, context.config, pid, &context.process_id)
            .await;
    }
    let (stdout, stderr) = match take_child_pipes(&mut child) {
        Ok(pipes) => pipes,
        Err(status) => return status,
    };
    supervise_child_output(child, stdout, stderr, context).await
}

async fn supervise_child_output(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    context: ChildSupervisionContext<'_>,
) -> ProcessTerminalStatus {
    let ChildSupervisionContext {
        job,
        settings,
        config,
        dispatcher,
        parse_state,
        cancel_requested,
        process_id,
    } = context;

    let (line_sender, mut line_receiver) = mpsc::channel::<(OutputStream, String)>(4096);
    let stdout_reader = spawn_line_reader(OutputStream::Stdout, stdout, line_sender.clone());
    let stderr_reader = spawn_line_reader(OutputStream::Stderr, stderr, line_sender);
    let mut wait = Box::pin(child.wait());
    let mut poll_cancel = interval(Duration::from_millis(100));
    let mut first_output_notice = interval(Duration::from_secs(15));
    first_output_notice.tick().await;
    let started_at = Instant::now();
    let mut progress = OutputProgress::default();
    let mut terminal_status = None;
    // Bounded drain after process exit: a grandchild inheriting the stdout or
    // stderr pipe (`shell: ... &`) would otherwise keep the line readers open
    // forever and pin the job in Running.
    let mut output_drain_deadline: Option<Instant> = None;
    let deadline = Instant::now() + effective_job_timeout(settings, config);

    let final_status = loop {
        let interrupted = select_child_output_event(OutputSelectContext {
            wait: &mut wait,
            line_receiver: &mut line_receiver,
            poll_cancel: &mut poll_cancel,
            first_output_notice: &mut first_output_notice,
            progress: &mut progress,
            terminal_status: &mut terminal_status,
            output_drain_deadline: &mut output_drain_deadline,
            deadline,
            started_at,
            stdout_reader: &stdout_reader,
            stderr_reader: &stderr_reader,
            cancel_requested: &cancel_requested,
            process_id: &process_id,
            output: ChildOutputContext {
                job,
                settings,
                config,
                parse_state: &mut *parse_state,
                dispatcher,
            },
        })
        .await;
        if let Some(status) = interrupted {
            let _ = wait.await;
            return status;
        }
        if let Some(status) = completed_child_status(&progress, &mut terminal_status) {
            break status;
        }
    };
    let _ = stdout_reader.await;
    let _ = stderr_reader.await;
    final_status
}

/// One captured child output line together with the stream it arrived on.
struct ChildOutputLine<'a> {
    stream: OutputStream,
    line: &'a str,
}

/// Relay one captured child output line: optional raw console passthrough,
/// then the typed event mapping + dispatch. Split out of [`supervise_child`]'s
/// select loop, which owns only the stream bookkeeping.
async fn forward_child_output_line(
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    config: &AppConfig,
    parse_state: &mut ParseState,
    dispatcher: &EventDispatcher,
    output: ChildOutputLine<'_>,
) {
    let ChildOutputLine { stream, line } = output;
    if settings.raw_stdout_passthrough {
        send_raw_console_line(dispatcher, job, stream, line).await;
    }
    if let Some(mut event) = map_output_line(job, settings, config, parse_state, stream, line) {
        enrich_event_run_id(job, &mut event);
        // The mapped output IS the console the operator watches, so it must never
        // be silently dropped: enqueue with backpressure (`must_keep`) so a slow
        // callback endpoint stalls the stdout reader (and thereby ansible) instead
        // of truncating the output. A hard-down backend still drains via
        // flush-on-error, so this cannot hang the job.
        dispatcher.enqueue(event, true).await;
    }
}

/// Console heads-up while `ansible-playbook` has produced no stdout yet —
/// long silent SSH connect storms look like a hang without it.
async fn announce_waiting_for_first_output(
    dispatcher: &EventDispatcher,
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    started_at: Instant,
) {
    send_group_console_line(
        dispatcher,
        job,
        format!(
            "ansible-playbook still running after {}s; waiting for first Ansible stdout. SSH connects may be occupying up to {} forks.",
            started_at.elapsed().as_secs(),
            settings
                .forks
                .map_or_else(|| "the configured".to_string(), |forks| forks.to_string()),
        ),
    )
    .await;
}

/// Log + console notice that the exited process left its output pipes open
/// (a leftover grandchild) and the readers are being force-closed.
async fn announce_forced_output_close(
    dispatcher: &EventDispatcher,
    job: &RunnerJobRequest,
    config: &AppConfig,
) {
    warn!(
        run_group_id = %job.run_group_id,
        "ansible-playbook exited but its output pipes are still open; force-closing readers after drain grace"
    );
    send_group_console_line(
        dispatcher,
        job,
        format!(
            "ansible-playbook exited but a leftover child process still holds its output stream; stopped reading output after {}s drain grace",
            config.output_drain_grace.as_secs(),
        ),
    )
    .await;
}

/// Job timeout requested by the backend, falling back to the runner-wide
/// default so a hung `ansible-playbook` can never live forever.
fn effective_job_timeout(settings: &RunnerSettings, config: &AppConfig) -> Duration {
    settings.job_timeout.unwrap_or(config.default_job_timeout)
}

#[derive(Debug, Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

fn spawn_line_reader<R>(
    stream: OutputStream,
    reader: R,
    sender: mpsc::Sender<(OutputStream, String)>,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if sender.send((stream, line)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(error = %error, "failed to read ansible output line");
                    break;
                }
            }
        }
    })
}

fn materialize_private_data_dir(
    config: &AppConfig,
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
) -> anyhow::Result<PrivateDataDir> {
    let root = config.data_dir.join(job.run_group_id.to_string());
    if root.exists() {
        fs::remove_dir_all(&root).with_context(|| format!("remove old {}", root.display()))?;
    }
    let project = root.join("project");
    let inventory = root.join("inventory");
    let env_dir = root.join("env");
    let local_tmp = root.join("tmp").join("local");
    for dir in [&project, &inventory, &env_dir, &local_tmp] {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let entrypoint = validated_project_path(
        job.entrypoint
            .as_deref()
            .unwrap_or(DEFAULT_PROJECT_ENTRYPOINT),
        "entrypoint",
    )?;
    if job.project_files.is_empty() {
        write_file_private(&project.join(&entrypoint), &job.playbook, false)?;
    } else {
        write_project_bundle(&project, &job.project_files)?;
        if !project_bundle_contains_path(&job.project_files, &entrypoint)? {
            if job.playbook.trim().is_empty() {
                bail!(
                    "project entrypoint {} is not present in project_files",
                    entrypoint.display()
                );
            }
            write_file_private(&project.join(&entrypoint), &job.playbook, false)?;
        }
    }
    write_file_private(&inventory.join("hosts"), &job.inventory, false)?;
    let mut extravars = job.extravars.clone();
    neutralise_extravars_ssh_args(&mut extravars);
    write_file_private(
        &env_dir.join("extravars"),
        &serde_json::to_string_pretty(&extravars).context("serialize extravars")?,
        false,
    )?;
    if !job.passwords.is_empty() {
        write_file_private(
            &env_dir.join("passwords"),
            &serde_json::to_string_pretty(&job.passwords).context("serialize passwords")?,
            false,
        )?;
    }
    let known_hosts = write_job_known_hosts_file(&project, &job.targets)?;
    write_file_private(
        &project.join("ansible.cfg"),
        &build_ansible_config(&local_tmp, settings, &known_hosts)?,
        false,
    )?;
    write_per_host_key_files(&project, &job.targets)?;
    Ok(PrivateDataDir {
        root,
        project,
        inventory,
        env: env_dir,
        local_tmp,
        entrypoint: entrypoint.clone(),
        known_hosts,
    })
}

/// Union every target's pinned `known_hosts` lines into one job-level seed file
/// at `<project>/known_hosts` (0600). An empty seed still materializes the file
/// so the pinned ssh options can TOFU-harvest observed keys into it. The file is
/// the only known_hosts ssh consults for this job (see `pinned_host_key_options`).
fn write_job_known_hosts_file(
    project: &Path,
    targets: &[RunnerJobTarget],
) -> anyhow::Result<PathBuf> {
    let path = project.join(JOB_KNOWN_HOSTS_FILE);
    let mut lines: Vec<String> = Vec::new();
    for target in targets {
        for line in &target.known_hosts_lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.contains('\n') || line.contains('\r') || line.contains('\0') {
                bail!("known_hosts lines must be single-line entries");
            }
            lines.push(line.to_string());
        }
    }
    lines.sort();
    lines.dedup();
    let mut contents = lines.join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }
    write_file_private(&path, &contents, false)?;
    Ok(path)
}

/// Host-key option keys that, supplied by a caller, could re-enable fail-open
/// host verification. The runner strips every caller occurrence instead of
/// relying on which repeated option OpenSSH consults, so the pins cannot be
/// displaced by caller ordering.
const HOST_KEY_OPTION_KEYS: [&str; 4] = [
    "StrictHostKeyChecking",
    "UserKnownHostsFile",
    "GlobalKnownHostsFile",
    "HashKnownHosts",
];

/// Split an ssh-args string into shell-like tokens on unquoted whitespace,
/// keeping quote characters so a quoted value with inner spaces survives a
/// round trip unchanged.
fn ssh_arg_tokens(raw: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    let mut quote: Option<char> = None;
    for (index, ch) in raw.char_indices() {
        match quote {
            Some(open) => {
                if ch == open {
                    quote = None;
                }
            }
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                start.get_or_insert(index);
            }
            None if ch.is_whitespace() => {
                if let Some(begin) = start.take() {
                    tokens.push(&raw[begin..index]);
                }
            }
            None => {
                start.get_or_insert(index);
            }
        }
    }
    if let Some(begin) = start {
        tokens.push(&raw[begin..]);
    }
    tokens
}

/// True when `token` assigns one of [`HOST_KEY_OPTION_KEYS`], in the
/// `Key=value`, `-oKey=value` or quoted `"Key=value"` spellings. The separated
/// `-o Key=value` form is handled by the caller.
fn is_host_key_option_assignment(token: &str) -> bool {
    let unquoted = token
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            token
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(token);
    let assignment = unquoted.strip_prefix("-o").unwrap_or(unquoted);
    match assignment.split_once('=') {
        // OpenSSH option names are case-insensitive, so match them that way.
        Some((key, _)) => HOST_KEY_OPTION_KEYS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(key)),
        None => false,
    }
}

/// Remove every caller-supplied host-key option from an ssh-args string so the
/// runner's pins stay authoritative whichever repeated option OpenSSH
/// consults. Recognizes `-o Key=value`, `-oKey=value`, bare `Key=value` and
/// quoted `"Key=value"`; every other token, including quoted values, is
/// preserved verbatim.
fn sanitize_caller_ssh_args(raw: &str) -> String {
    let tokens = ssh_arg_tokens(raw);
    let mut kept: Vec<&str> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token == "-o" {
            if tokens
                .get(index + 1)
                .is_some_and(|value| is_host_key_option_assignment(value))
            {
                index += 2;
                continue;
            }
        } else if is_host_key_option_assignment(token) {
            index += 1;
            continue;
        }
        kept.push(token);
        index += 1;
    }
    kept.join(" ")
}

/// Effective `ssh_args` for this job: caller-supplied args (with any host-key
/// options stripped) followed by the pinned host-key options, so a caller
/// cannot drop or override the pins. Emitted even without caller args: the pins
/// are never optional.
fn effective_ssh_args(settings: &RunnerSettings, known_hosts: &Path) -> String {
    pinned_with_caller_args(settings.ssh_args.as_deref(), known_hosts)
}

/// Effective `ssh_common_args`: same contract as [`effective_ssh_args`]. The
/// pins are emitted on this channel as well as `ssh_args`, so they survive
/// whichever occurrence Ansible and OpenSSH consult; the sanitizer drops any
/// caller host-key option from the caller-supplied remainder.
fn effective_ssh_common_args(settings: &RunnerSettings, known_hosts: &Path) -> String {
    pinned_with_caller_args(settings.ssh_common_args.as_deref(), known_hosts)
}

fn pinned_with_caller_args(caller_args: Option<&str>, known_hosts: &Path) -> String {
    let pins = pinned_host_key_options(known_hosts);
    match caller_args {
        Some(existing) if !existing.trim().is_empty() => {
            let sanitized = sanitize_caller_ssh_args(existing);
            if sanitized.is_empty() {
                pins
            } else {
                format!("{sanitized} {pins}")
            }
        }
        _ => pins,
    }
}

/// Drop caller-smuggled ssh overrides from playbook extravars. Ansible's ssh
/// connection plugin reads three families of variable and variables outrank
/// both ansible.cfg and the `ANSIBLE_*` environment:
///
/// * option strings that would re-enable fail-open verification
///   (`ansible_ssh_common_args`, `ansible_ssh_args`, `ansible_ssh_extra_args`,
///   `ansible_scp_extra_args`, `ansible_sftp_extra_args`);
/// * executable selectors that substitute a wrapper for ssh/scp/sftp
///   (`ansible_ssh_executable`, `ansible_scp_executable`,
///   `ansible_sftp_executable`);
/// * the host-key-checking toggle, whose accepted spellings include
///   `ansible_host_key_checking` and `ansible_ssh_host_key_checking`.
///
/// The runner removes them outright rather than merging non-host-key tokens: a
/// variable value is not shell-tokenized until Ansible consumes it, and
/// dropping is the only fail-closed choice that cannot be re-bypassed through
/// these surfaces.
fn neutralise_extravars_ssh_args(extravars: &mut Value) {
    let Some(map) = extravars.as_object_mut() else {
        return;
    };
    for key in [
        "ansible_ssh_common_args",
        "ansible_ssh_args",
        "ansible_ssh_extra_args",
        "ansible_scp_extra_args",
        "ansible_sftp_extra_args",
        "ansible_ssh_executable",
        "ansible_scp_executable",
        "ansible_sftp_executable",
        "ansible_host_key_checking",
        "ansible_ssh_host_key_checking",
    ] {
        map.remove(key);
    }
}

/// Lines observed in the job-level known_hosts file that were not part of the
/// seed. Read after the play so the backend can persist newly learned (TOFU)
/// host keys. Comment and blank lines are ignored.
fn harvest_new_host_keys(known_hosts: &Path, targets: &[RunnerJobTarget]) -> Vec<String> {
    let seed: HashSet<&str> = targets
        .iter()
        .flat_map(|target| target.known_hosts_lines.iter())
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    let Ok(contents) = fs::read_to_string(known_hosts) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut new_keys = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || seed.contains(line) {
            continue;
        }
        if seen.insert(line.to_string()) {
            new_keys.push(line.to_string());
        }
    }
    new_keys.sort();
    new_keys
}

/// Harvest the job-level known_hosts file for the completion callback. The path
/// mirrors `materialize_private_data_dir`; a job that never materialized (early
/// cancel or dispatcher failure) simply yields no new keys.
fn new_host_keys_for_job(config: &AppConfig, job: &RunnerJobRequest) -> Vec<String> {
    let known_hosts = config
        .data_dir
        .join(job.run_group_id.to_string())
        .join("project")
        .join(JOB_KNOWN_HOSTS_FILE);
    harvest_new_host_keys(&known_hosts, &job.targets)
}

/// Materialize per-host SSH key files under the reserved
/// `<project>/.piping_keys/<alias>` directory (0600). These serve grouped runs
/// whose hosts use distinct key material; the backend inventory references each
/// as `ansible_ssh_private_key_file=.piping_keys/<alias>`, relative to the play
/// working directory (the project root). Aliases are backend-sanitized, but the
/// runner validates defensively so a crafted alias cannot escape the reserved
/// directory.
fn write_per_host_key_files(project: &Path, targets: &[RunnerJobTarget]) -> anyhow::Result<()> {
    for target in targets {
        let Some(key) = target.ssh_private_key.as_deref() else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        let alias = validated_key_alias(&target.alias)?;
        let path = project.join(RUNNER_PER_HOST_KEY_DIR).join(alias);
        write_file_private(&path, key, false)?;
    }
    Ok(())
}

/// Validate a per-host key alias is a single safe path component so it cannot
/// escape `.piping_keys/`. Reuses the same traversal guards as project paths.
fn validated_key_alias(alias: &str) -> anyhow::Result<PathBuf> {
    let path = validated_project_path(alias, "per-host key alias")?;
    if path.components().count() != 1 {
        bail!("per-host key alias must be a single path component");
    }
    Ok(path)
}

fn write_project_bundle(project: &Path, files: &[RunnerProjectFile]) -> anyhow::Result<()> {
    validate_project_bundle(files)?;
    for file in files {
        let path = validated_project_path(&file.path, "project file")?;
        write_file_private(&project.join(path), &file.content, file.executable)?;
    }
    Ok(())
}

fn validate_project_bundle(files: &[RunnerProjectFile]) -> anyhow::Result<()> {
    if files.len() > MAX_PROJECT_FILES {
        bail!("project bundle has too many files");
    }
    let mut seen = HashSet::new();
    let mut total_bytes = 0usize;
    for file in files {
        let path = validated_project_path(&file.path, "project file")?;
        if path == Path::new("ansible.cfg") {
            bail!("project bundle must not include ansible.cfg; use typed runner settings");
        }
        if path.starts_with(RUNNER_PER_HOST_KEY_DIR) {
            bail!(
                "project bundle must not include files under the runner-reserved {RUNNER_PER_HOST_KEY_DIR}/ directory"
            );
        }
        if !seen.insert(path.clone()) {
            bail!("project bundle contains duplicate path {}", path.display());
        }
        if file.content.len() > MAX_PROJECT_FILE_BYTES {
            bail!("project file {} is too large", path.display());
        }
        total_bytes = total_bytes.saturating_add(file.content.len());
        if total_bytes > MAX_PROJECT_TOTAL_BYTES {
            bail!("project bundle is too large");
        }
    }
    Ok(())
}

fn project_bundle_contains_path(
    files: &[RunnerProjectFile],
    needle: &Path,
) -> anyhow::Result<bool> {
    for file in files {
        if validated_project_path(&file.path, "project file")? == needle {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validated_project_path(value: &str, field: &str) -> anyhow::Result<PathBuf> {
    if value.trim().is_empty() {
        bail!("{field} path must not be empty");
    }
    if value.contains('\\') || value.contains('\0') {
        bail!("{field} path must be a relative POSIX path");
    }
    let path = Path::new(value);
    if path.is_absolute() {
        bail!("{field} path must be relative");
    }
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => output.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("{field} path must not contain traversal")
            }
        }
    }
    if output.as_os_str().is_empty() {
        bail!("{field} path must contain a file name");
    }
    Ok(output)
}

fn write_file_private(path: &Path, contents: &str, executable: bool) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}

fn build_ansible_config(
    local_tmp: &Path,
    settings: &RunnerSettings,
    known_hosts: &Path,
) -> anyhow::Result<String> {
    let ansible_timeout = settings
        .ansible_timeout_seconds
        .unwrap_or(DEFAULT_ANSIBLE_TIMEOUT_SECONDS)
        .to_string();
    let defaults = ansible_defaults(local_tmp, settings, &ansible_timeout)?;
    let ssh_connection = ansible_ssh_connection(settings, ansible_timeout, known_hosts)?;

    let mut lines = vec!["[defaults]".to_string()];
    for (name, value) in defaults {
        lines.push(format!("{name} = {value}"));
    }

    if !ssh_connection.is_empty() {
        lines.push(String::new());
        lines.push("[ssh_connection]".to_string());
        for (name, value) in ssh_connection {
            lines.push(format!("{name} = {value}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

/// `[defaults]` key/value pairs in emission order for `build_ansible_config`.
fn ansible_defaults(
    local_tmp: &Path,
    settings: &RunnerSettings,
    ansible_timeout: &str,
) -> anyhow::Result<Vec<(&'static str, String)>> {
    let mut defaults = vec![
        ("fact_caching", "memory".to_string()),
        // Emit ANSI colour so the UI renders ansible-native colouring; the runner
        // strips it before its own structured parsing (see `strip_ansi_codes`).
        ("force_color", "True".to_string()),
        ("local_tmp", local_tmp.to_string_lossy().to_string()),
        ("retry_files_enabled", "False".to_string()),
        ("timeout", ansible_timeout.to_string()),
    ];
    if let Some(strategy) = &settings.strategy {
        defaults.push(("strategy", config_value(strategy)?));
    }
    if let Some(stdout_callback) = &settings.stdout_callback {
        defaults.push(("stdout_callback", config_value(stdout_callback)?));
    }
    if let Some(remote_tmp) = &settings.remote_tmp {
        defaults.push(("remote_tmp", config_value(remote_tmp)?));
        defaults.push(("system_tmpdirs", config_value(remote_tmp)?));
    }
    if let Some(shell_executable) = &settings.shell_executable {
        defaults.push(("executable", config_value(shell_executable)?));
    }
    if let Some(deprecation) = settings.deprecation_warnings {
        defaults.push(("deprecation_warnings", bool_string(deprecation)));
    }
    if let Some(display_ok) = settings.display_ok_hosts {
        defaults.push(("display_ok_hosts", bool_string(display_ok)));
    }
    if let Some(display_skipped) = settings.display_skipped_hosts {
        defaults.push(("display_skipped_hosts", bool_string(display_skipped)));
    }
    if let Some(pipelining) = settings.pipelining {
        defaults.push(("pipelining", bool_string(pipelining)));
    }
    Ok(defaults)
}

/// `[ssh_connection]` key/value pairs in emission order for `build_ansible_config`.
fn ansible_ssh_connection(
    settings: &RunnerSettings,
    ansible_timeout: String,
    known_hosts: &Path,
) -> anyhow::Result<Vec<(&'static str, String)>> {
    let mut ssh_connection = Vec::new();
    if let Some(pipelining) = settings.pipelining {
        ssh_connection.push(("pipelining", bool_string(pipelining)));
    }
    // Pin every ssh child to the job-level known_hosts file. Emitted even when
    // the caller supplied no ssh_args: the pins must not be optional.
    ssh_connection.push((
        "ssh_args",
        config_value(&effective_ssh_args(settings, known_hosts))?,
    ));
    // Emit the pins on ssh_common_args as well as ssh_args, sanitizing any
    // caller host-key option, so the pin holds whichever occurrence Ansible
    // and OpenSSH consult.
    ssh_connection.push((
        "ssh_common_args",
        config_value(&effective_ssh_common_args(settings, known_hosts))?,
    ));
    ssh_connection.push(("timeout", ansible_timeout));
    Ok(ssh_connection)
}

fn config_value(value: &str) -> anyhow::Result<String> {
    if value.contains('\n') || value.contains('\r') {
        bail!("ansible setting values must be single-line");
    }
    Ok(value.to_string())
}

/// Which `ansible-galaxy` sub-command a detected requirements file feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GalaxyReqKind {
    Collection,
    Role,
}

/// One `ansible-galaxy` install invocation: a requirements file (relative to
/// the project root) and the sub-command that consumes it. A root
/// `requirements.yml` yields both a collection and a role install; each
/// sub-command silently skips the section it does not own.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GalaxyInstall {
    kind: GalaxyReqKind,
    requirements: PathBuf,
}

/// Consume-side search paths handed to the `ansible-playbook` process so it
/// resolves the collections/roles installed for this job. Each is the job
/// cache dir *prepended* to the image system path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GalaxyPaths {
    collections_path: OsString,
    roles_path: OsString,
}

/// Detect Galaxy requirements in the materialized project bundle, mapping each
/// to its install sub-command by Ansible's directory convention:
/// `collections/requirements.y{a,}ml` -> collections, `roles/...` -> roles, and
/// a root `requirements.y{a,}ml` -> whichever section(s) it actually declares.
/// The root file is content-classified because `ansible-galaxy collection
/// install` *errors* (not skips) on the classic pre-2.10 bare-list role file,
/// so running it unconditionally would break every repo using that format.
/// Returns an empty vector when the project declares no dependencies (the
/// common, no-network, no-latency case).
fn plan_galaxy_installs(project: &Path) -> Vec<GalaxyInstall> {
    let mut plan = Vec::new();
    if let Some(root) = first_existing_requirements(project, "") {
        let content = fs::read_to_string(project.join(&root)).unwrap_or_default();
        for kind in detect_root_galaxy_kinds(&content) {
            plan.push(GalaxyInstall {
                kind,
                requirements: root.clone(),
            });
        }
    }
    if let Some(collections) = first_existing_requirements(project, "collections") {
        plan.push(GalaxyInstall {
            kind: GalaxyReqKind::Collection,
            requirements: collections,
        });
    }
    if let Some(roles) = first_existing_requirements(project, "roles") {
        plan.push(GalaxyInstall {
            kind: GalaxyReqKind::Role,
            requirements: roles,
        });
    }
    plan
}

/// Classify a root `requirements.yml` by its top-level YAML shape so only the
/// applicable `ansible-galaxy` sub-command runs. A bare top-level sequence is
/// the classic role list (roles only); a mapping declares collections and/or
/// roles per the keys actually present. A mapping with neither key installs
/// nothing (malformed/empty declares no installable dependency — the play
/// surfaces any real miss on its own).
fn detect_root_galaxy_kinds(content: &str) -> Vec<GalaxyReqKind> {
    let mut top_level_sequence = false;
    let mut saw_mapping_key = false;
    let mut has_collections = false;
    let mut has_roles = false;
    for line in content.lines() {
        match classify_root_galaxy_line(line) {
            Some(RootGalaxyLine::Sequence) => top_level_sequence = true,
            Some(RootGalaxyLine::Collections) => {
                saw_mapping_key = true;
                has_collections = true;
            }
            Some(RootGalaxyLine::Roles) => {
                saw_mapping_key = true;
                has_roles = true;
            }
            Some(RootGalaxyLine::OtherKey) => saw_mapping_key = true,
            None => {}
        }
    }
    if top_level_sequence && !saw_mapping_key {
        return vec![GalaxyReqKind::Role];
    }
    let mut kinds = Vec::new();
    if has_collections {
        kinds.push(GalaxyReqKind::Collection);
    }
    if has_roles {
        kinds.push(GalaxyReqKind::Role);
    }
    kinds
}

/// The shape a single top-level line of a root `requirements.yml` contributes:
/// a sequence entry, a recognised mapping key, an unrecognised mapping key, or
/// nothing (blank/comment/document-marker/indented line).
enum RootGalaxyLine {
    Sequence,
    Collections,
    Roles,
    OtherKey,
}

/// Classify one raw line for `detect_root_galaxy_kinds`. Only zero-indentation
/// lines determine the document shape; everything else yields `None`.
fn classify_root_galaxy_line(line: &str) -> Option<RootGalaxyLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
        return None;
    }
    // Only the top level (zero indentation) determines the document shape.
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    if trimmed == "-" || trimmed.starts_with("- ") {
        return Some(RootGalaxyLine::Sequence);
    }
    match top_level_mapping_key(trimmed) {
        Some("collections") => Some(RootGalaxyLine::Collections),
        Some("roles") => Some(RootGalaxyLine::Roles),
        Some(_) => Some(RootGalaxyLine::OtherKey),
        None => None,
    }
}

/// The bare word before the first `:` of a top-level `key:` line, or `None`
/// when the line is not a simple block-mapping key.
fn top_level_mapping_key(trimmed: &str) -> Option<&str> {
    let (key, _rest) = trimmed.split_once(':')?;
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some(key)
}

/// First existing `requirements.yml` then `requirements.yaml` under `subdir`
/// (empty = project root), returned as a path relative to the project root.
fn first_existing_requirements(project: &Path, subdir: &str) -> Option<PathBuf> {
    for name in ["requirements.yml", "requirements.yaml"] {
        let relative = if subdir.is_empty() {
            PathBuf::from(name)
        } else {
            Path::new(subdir).join(name)
        };
        if project.join(&relative).is_file() {
            return Some(relative);
        }
    }
    None
}

/// Content-address the plan so identical requirements share a cache dir across
/// jobs. FNV-1a over each `(relative path, file bytes)` pair keeps the key
/// dependency-free and stable; a distinct requirements set yields a distinct
/// directory, so a cache hit can never serve the wrong dependencies.
fn galaxy_requirements_hash(project: &Path, plan: &[GalaxyInstall]) -> anyhow::Result<String> {
    let mut relatives: Vec<&Path> = plan
        .iter()
        .map(|item| item.requirements.as_path())
        .collect();
    relatives.sort_unstable();
    relatives.dedup();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for relative in relatives {
        let bytes = fs::read(project.join(relative))
            .with_context(|| format!("read galaxy requirements {}", relative.display()))?;
        fnv1a_update(&mut hash, relative.to_string_lossy().as_bytes());
        fnv1a_update(&mut hash, &[0]);
        fnv1a_update(&mut hash, &bytes);
        fnv1a_update(&mut hash, &[0]);
    }
    Ok(format!("{hash:016x}"))
}

fn fnv1a_update(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Program + argv for one install, targeting explicit `-p` destinations. `-p`
/// is version-stable across ansible-core releases (unlike the combined
/// `ansible-galaxy install` meta-command, which honors `-p` for roles only).
fn galaxy_install_argv(
    install: &GalaxyInstall,
    collections_dir: &Path,
    roles_dir: &Path,
) -> (&'static str, Vec<OsString>) {
    let (subcommand, dest) = match install.kind {
        GalaxyReqKind::Collection => ("collection", collections_dir),
        GalaxyReqKind::Role => ("role", roles_dir),
    };
    let args = vec![
        OsString::from(subcommand),
        OsString::from("install"),
        OsString::from("-r"),
        install.requirements.clone().into_os_string(),
        OsString::from("-p"),
        dest.to_path_buf().into_os_string(),
    ];
    ("ansible-galaxy", args)
}

/// Build the consume-side search paths: job cache first, image system path
/// second, so per-job deps override but never hide the baked-in collections.
fn galaxy_consume_paths(collections_dir: &Path, roles_dir: &Path) -> GalaxyPaths {
    GalaxyPaths {
        collections_path: prepend_path(collections_dir, GALAXY_SYSTEM_COLLECTIONS_PATH),
        roles_path: prepend_path(roles_dir, GALAXY_SYSTEM_ROLES_PATH),
    }
}

fn prepend_path(head: &Path, tail: &str) -> OsString {
    let mut value = head.as_os_str().to_os_string();
    value.push(":");
    value.push(tail);
    value
}

/// Redact `//user:secret@host` userinfo from a URL so a tokened git/https
/// source in requirements output or an error tail never reaches the console
/// events verbatim.
fn mask_url_userinfo(line: &str) -> std::borrow::Cow<'_, str> {
    static USERINFO_RE: std::sync::OnceLock<Option<Regex>> = std::sync::OnceLock::new();
    let Some(regex) = USERINFO_RE
        .get_or_init(|| Regex::new(r"//[^/@\s]+@").ok())
        .as_ref()
    else {
        return std::borrow::Cow::Borrowed(line);
    };
    regex.replace_all(line, "//***@")
}

/// Secret-free environment for an `ansible-galaxy` invocation. Mirrors the
/// play process: `env_clear` + the inherited allowlist so the control-plane
/// shared secret never reaches galaxy's git/https child processes, plus the
/// generated `ansible.cfg` and the per-job local temp dir.
fn build_galaxy_env(
    parent_env: impl IntoIterator<Item = (OsString, OsString)>,
    pdd: &PrivateDataDir,
) -> BTreeMap<String, OsString> {
    let mut envs = inherited_env_allowlist(parent_env);
    envs.insert(
        "ANSIBLE_CONFIG".to_string(),
        pdd.project.join("ansible.cfg").into_os_string(),
    );
    envs.insert("ANSIBLE_FORCE_COLOR".to_string(), OsString::from("1"));
    envs.insert(
        "ANSIBLE_LOCAL_TEMP".to_string(),
        pdd.local_tmp.clone().into_os_string(),
    );
    envs
}

/// Install any Galaxy requirements the project declares, returning the
/// consume-side search paths (or `None` when there are none). A cache hit
/// requires no network; a miss runs `ansible-galaxy` and surfaces its failure
/// (e.g. an unreachable server in an air-gapped fleet) as a hard error rather
/// than a silent success.
async fn install_galaxy_requirements(
    config: &AppConfig,
    job: &RunnerJobRequest,
    pdd: &PrivateDataDir,
    dispatcher: &EventDispatcher,
) -> anyhow::Result<Option<GalaxyPaths>> {
    let plan = plan_galaxy_installs(&pdd.project);
    if plan.is_empty() {
        return Ok(None);
    }
    let hash = galaxy_requirements_hash(&pdd.project, &plan)?;
    let cache_dir = config.galaxy_cache_dir.join(&hash);
    let collections_dir = cache_dir.join("collections");
    let roles_dir = cache_dir.join("roles");
    let paths = galaxy_consume_paths(&collections_dir, &roles_dir);

    if cache_dir.exists() {
        send_group_console_line(
            dispatcher,
            job,
            format!("ansible-galaxy: reusing cached requirements ({hash}); no install needed"),
        )
        .await;
        return Ok(Some(paths));
    }

    fs::create_dir_all(&config.galaxy_cache_dir).with_context(|| {
        format!(
            "create galaxy cache root {}",
            config.galaxy_cache_dir.display()
        )
    })?;
    // Populate a sibling temp dir (same parent = same filesystem, so the
    // publish rename is atomic and never hits EXDEV), then swap it into place.
    let staging = config
        .galaxy_cache_dir
        .join(format!(".staging-{}", Uuid::now_v7()));
    let result = populate_galaxy_cache(config, job, pdd, dispatcher, &plan, &staging).await;
    match result {
        Ok(()) => publish_galaxy_cache(&staging, &cache_dir).map(|()| Some(paths)),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            Err(error)
        }
    }
}

/// Move the fully-populated staging dir into its content-addressed home. A
/// concurrent job with identical requirements may have published first; that
/// is a win, not an error — discard staging and reuse the peer's cache.
fn publish_galaxy_cache(staging: &Path, cache_dir: &Path) -> anyhow::Result<()> {
    match fs::rename(staging, cache_dir) {
        Ok(()) => Ok(()),
        Err(_) if cache_dir.exists() => {
            let _ = fs::remove_dir_all(staging);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_dir_all(staging);
            Err(error).with_context(|| format!("publish galaxy cache {}", cache_dir.display()))
        }
    }
}

async fn populate_galaxy_cache(
    config: &AppConfig,
    job: &RunnerJobRequest,
    pdd: &PrivateDataDir,
    dispatcher: &EventDispatcher,
    plan: &[GalaxyInstall],
    staging: &Path,
) -> anyhow::Result<()> {
    let collections_dir = staging.join("collections");
    let roles_dir = staging.join("roles");
    for dir in [&collections_dir, &roles_dir] {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    send_group_console_line(
        dispatcher,
        job,
        format!(
            "ansible-galaxy: installing project requirements ({} file(s))",
            plan.len()
        ),
    )
    .await;
    for install in plan {
        let (program, args) = galaxy_install_argv(install, &collections_dir, &roles_dir);
        run_galaxy_command(config, job, pdd, dispatcher, program, &args).await?;
    }
    Ok(())
}

/// Spawn one `ansible-galaxy` process, stream its output to the console events
/// verbatim (userinfo-masked), and enforce the install timeout with a process
/// group kill so a git child cannot outlive the reap.
/// Append one masked `ansible-galaxy` output line to the bounded error tail,
/// returning the masked line for the console passthrough.
fn record_galaxy_output_tail(tail: &mut std::collections::VecDeque<String>, line: &str) -> String {
    let masked = mask_url_userinfo(line).into_owned();
    if tail.len() >= GALAXY_ERROR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(masked.clone());
    masked
}

/// Decision for one `ansible-galaxy` poll tick, kept pure so the select loop
/// owns only the side effects.
#[derive(Clone, Copy)]
enum GalaxyTick {
    Running,
    TimedOut,
    ForceCloseOutput,
}

fn galaxy_tick(
    exit_pending: bool,
    past_deadline: bool,
    output_closed: bool,
    drain_expired: bool,
) -> GalaxyTick {
    if exit_pending {
        if past_deadline {
            return GalaxyTick::TimedOut;
        }
        return GalaxyTick::Running;
    }
    if !output_closed && drain_expired {
        return GalaxyTick::ForceCloseOutput;
    }
    GalaxyTick::Running
}

async fn forward_galaxy_output(
    received: Option<(OutputStream, String)>,
    dispatcher: &EventDispatcher,
    job: &RunnerJobRequest,
    tail: &mut std::collections::VecDeque<String>,
    output_closed: &mut bool,
) {
    if let Some((stream, line)) = received {
        let masked = record_galaxy_output_tail(tail, &line);
        send_raw_console_line(dispatcher, job, stream, &masked).await;
    } else {
        *output_closed = true;
    }
}

struct GalaxyPollContext<'a> {
    pid: Option<u32>,
    job: &'a RunnerJobRequest,
    stdout_reader: &'a JoinHandle<()>,
    stderr_reader: &'a JoinHandle<()>,
    output_closed: &'a mut bool,
}

async fn handle_galaxy_poll(tick: GalaxyTick, context: GalaxyPollContext<'_>) -> bool {
    match tick {
        GalaxyTick::TimedOut => {
            if let Some(pid) = context.pid {
                terminate_process_group(pid).await;
            }
            true
        }
        GalaxyTick::ForceCloseOutput => {
            warn!(
                run_group_id = %context.job.run_group_id,
                "ansible-galaxy exited but its output pipes are still open; force-closing readers after drain grace"
            );
            context.stdout_reader.abort();
            context.stderr_reader.abort();
            *context.output_closed = true;
            false
        }
        GalaxyTick::Running => false,
    }
}

async fn finish_timed_out_galaxy(
    wait: impl Future<Output = std::io::Result<std::process::ExitStatus>>,
    stdout_reader: JoinHandle<()>,
    stderr_reader: JoinHandle<()>,
    limit: Duration,
) -> anyhow::Result<()> {
    let _ = wait.await;
    stdout_reader.abort();
    stderr_reader.abort();
    let _ = stdout_reader.await;
    let _ = stderr_reader.await;
    bail!(
        "ansible-galaxy timed out after {}s while installing requirements",
        limit.as_secs()
    );
}

fn launch_galaxy_command(
    pdd: &PrivateDataDir,
    program: &str,
    args: &[OsString],
) -> anyhow::Result<(
    tokio::process::Child,
    tokio::process::ChildStdout,
    tokio::process::ChildStderr,
)> {
    let mut command = Command::new(program);
    command.args(args);
    command.current_dir(&pdd.project);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.env_clear();
    command.envs(build_galaxy_env(env::vars_os(), pdd));
    configure_process_group(&mut command);

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {program}"))?;
    let stdout = child
        .stdout
        .take()
        .context("ansible-galaxy stdout pipe was not captured")?;
    let stderr = child
        .stderr
        .take()
        .context("ansible-galaxy stderr pipe was not captured")?;
    Ok((child, stdout, stderr))
}

struct GalaxyCommandContext<'a> {
    config: &'a AppConfig,
    job: &'a RunnerJobRequest,
    dispatcher: &'a EventDispatcher,
    program: &'a str,
    args: &'a [OsString],
}

async fn run_galaxy_command(
    config: &AppConfig,
    job: &RunnerJobRequest,
    pdd: &PrivateDataDir,
    dispatcher: &EventDispatcher,
    program: &str,
    args: &[OsString],
) -> anyhow::Result<()> {
    let (child, stdout, stderr) = launch_galaxy_command(pdd, program, args)?;
    supervise_galaxy_output(
        child,
        stdout,
        stderr,
        GalaxyCommandContext {
            config,
            job,
            dispatcher,
            program,
            args,
        },
    )
    .await
}

async fn supervise_galaxy_output(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    context: GalaxyCommandContext<'_>,
) -> anyhow::Result<()> {
    let GalaxyCommandContext {
        config,
        job,
        dispatcher,
        program,
        args,
    } = context;
    let pid = child.id();
    let (line_sender, mut line_receiver) = mpsc::channel::<(OutputStream, String)>(1024);
    let stdout_reader = spawn_line_reader(OutputStream::Stdout, stdout, line_sender.clone());
    let stderr_reader = spawn_line_reader(OutputStream::Stderr, stderr, line_sender);

    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut wait = Box::pin(child.wait());
    let mut poll = interval(Duration::from_millis(100));
    let deadline = Instant::now() + config.galaxy_install_timeout;
    let mut exit: Option<std::io::Result<std::process::ExitStatus>> = None;
    let mut output_closed = false;
    // Bounded drain after the process exits: a git subprocess inheriting the
    // stdout/stderr pipe would otherwise keep the readers open forever and hang
    // the install step — the same hazard supervise_child guards against.
    let mut output_drain_deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            received = line_receiver.recv(), if !output_closed => {
                forward_galaxy_output(received, dispatcher, job, &mut tail, &mut output_closed).await;
            },
            status = &mut wait, if exit.is_none() => {
                exit = Some(status);
                output_drain_deadline = Some(Instant::now() + config.output_drain_grace);
            }
            _ = poll.tick() => {
                let tick = galaxy_tick(
                    exit.is_none(),
                    Instant::now() >= deadline,
                    output_closed,
                    output_drain_deadline.is_some_and(|drain| Instant::now() >= drain),
                );
                if handle_galaxy_poll(
                    tick,
                    GalaxyPollContext {
                        pid,
                        job,
                        stdout_reader: &stdout_reader,
                        stderr_reader: &stderr_reader,
                        output_closed: &mut output_closed,
                    },
                ).await {
                    return finish_timed_out_galaxy(
                        &mut wait,
                        stdout_reader,
                        stderr_reader,
                        config.galaxy_install_timeout,
                    ).await;
                }
            }
        }
        if exit.is_some() && output_closed {
            break;
        }
    }

    let _ = stdout_reader.await;
    let _ = stderr_reader.await;
    check_galaxy_exit(program, args, exit, tail)
}

/// Translate the awaited `ansible-galaxy` exit into a result, folding the
/// masked output tail into the failure message for operator context.
fn check_galaxy_exit(
    program: &str,
    args: &[OsString],
    exit: Option<std::io::Result<std::process::ExitStatus>>,
    tail: std::collections::VecDeque<String>,
) -> anyhow::Result<()> {
    let status = match exit {
        Some(result) => result.with_context(|| format!("wait for {program}"))?,
        None => bail!("ansible-galaxy exited without a reported status"),
    };
    if !status.success() {
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!(": {}", tail.into_iter().collect::<Vec<_>>().join(" | "))
        };
        bail!(
            "ansible-galaxy {} exited with status {}{detail}",
            args.first()
                .map(|arg| arg.to_string_lossy().into_owned())
                .unwrap_or_default(),
            status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
        );
    }
    Ok(())
}

fn build_ansible_command(
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    pdd: &PrivateDataDir,
) -> Command {
    let mut command = Command::new("ansible-playbook");
    command
        .arg("-i")
        .arg(pdd.inventory.join("hosts"))
        .arg("-e")
        .arg(format!("@{}", pdd.env.join("extravars").display()))
        .arg(pdd.project.join(&pdd.entrypoint));
    if let Some(limit) = &settings.limit {
        command.arg("--limit").arg(limit);
    } else if !job.targets.is_empty() {
        command.arg("--limit").arg(
            job.targets
                .iter()
                .map(|target| target.alias.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    if let Some(tags) = &settings.tags {
        command.arg("--tags").arg(tags);
    }
    if let Some(skip_tags) = &settings.skip_tags {
        command.arg("--skip-tags").arg(skip_tags);
    }
    if let Some(start_at_task) = &settings.start_at_task {
        command.arg("--start-at-task").arg(start_at_task);
    }
    if settings.check {
        command.arg("--check");
    }
    if settings.diff {
        command.arg("--diff");
    }
    if let Some(verbosity) = settings.verbosity
        && verbosity > 0
    {
        command.arg(format!("-{}", "v".repeat(usize::from(verbosity))));
    }
    command
}

/// Compose the complete environment for the spawned `ansible-playbook` process.
///
/// The child never inherits the runner environment wholesale: ansible-core
/// evaluates all templating and `lookup('env', ...)` on the controller, so a
/// full inherit would hand user playbooks the runner control-plane secret.
/// Only [`INHERITED_ENV_NAMES`] / [`INHERITED_ENV_PREFIXES`] pass through from
/// the parent; everything else comes from typed settings, the per-job
/// ssh-agent and the explicit request `playbook_env`, which take precedence
/// over inherited values.
fn build_process_env(
    parent_env: impl IntoIterator<Item = (OsString, OsString)>,
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    pdd: &PrivateDataDir,
    ssh_agent_env: Option<&BTreeMap<String, String>>,
    galaxy: Option<&GalaxyPaths>,
) -> BTreeMap<String, OsString> {
    let mut envs = inherited_env_allowlist(parent_env);
    envs.extend(build_playbook_envvars(
        job,
        settings,
        pdd,
        ssh_agent_env,
        galaxy,
    ));
    envs
}

fn inherited_env_allowlist(
    parent_env: impl IntoIterator<Item = (OsString, OsString)>,
) -> BTreeMap<String, OsString> {
    parent_env
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name.into_string().ok()?;
            let inherited = INHERITED_ENV_NAMES.contains(&name.as_str())
                || INHERITED_ENV_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix));
            inherited.then_some((name, value))
        })
        .collect()
}

fn build_playbook_envvars(
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    pdd: &PrivateDataDir,
    ssh_agent_env: Option<&BTreeMap<String, String>>,
    galaxy: Option<&GalaxyPaths>,
) -> BTreeMap<String, OsString> {
    let mut envs: BTreeMap<String, OsString> = BTreeMap::new();
    envs.insert(
        "ANSIBLE_CONFIG".to_string(),
        pdd.project.join("ansible.cfg").into_os_string(),
    );
    // Point the play at the per-job Galaxy cache (prepended to the image
    // system path) so `import_role`/`collections:` resolve the deps installed
    // in the galaxy step. Set before `playbook_env` so a request override wins.
    if let Some(galaxy) = galaxy {
        envs.insert(
            "ANSIBLE_COLLECTIONS_PATH".to_string(),
            galaxy.collections_path.clone(),
        );
        envs.insert("ANSIBLE_ROLES_PATH".to_string(), galaxy.roles_path.clone());
    }
    envs.insert("ANSIBLE_FORCE_COLOR".to_string(), OsString::from("1"));
    envs.insert(
        "ANSIBLE_LOCAL_TEMP".to_string(),
        pdd.local_tmp.clone().into_os_string(),
    );
    if let Some(forks) = settings.forks {
        envs.insert(
            "ANSIBLE_FORKS".to_string(),
            OsString::from(forks.to_string()),
        );
    }
    if let Some(ansible_timeout) = settings.ansible_timeout_seconds {
        envs.insert(
            "ANSIBLE_TIMEOUT".to_string(),
            OsString::from(ansible_timeout.to_string()),
        );
    }
    set_bool_env(
        &mut envs,
        "ANSIBLE_DEPRECATION_WARNINGS",
        settings.deprecation_warnings,
    );
    set_bool_env(
        &mut envs,
        "ANSIBLE_DISPLAY_OK_HOSTS",
        settings.display_ok_hosts,
    );
    set_bool_env(
        &mut envs,
        "ANSIBLE_DISPLAY_SKIPPED_HOSTS",
        settings.display_skipped_hosts,
    );
    set_bool_env(&mut envs, "ANSIBLE_PIPELINING", settings.pipelining);
    set_string_env(
        &mut envs,
        "ANSIBLE_PYTHON_INTERPRETER",
        settings.python_interpreter.as_deref(),
    );
    set_string_env(
        &mut envs,
        "ANSIBLE_REMOTE_TEMP",
        settings.remote_tmp.as_deref(),
    );
    set_string_env(
        &mut envs,
        "ANSIBLE_REMOTE_TMP",
        settings.remote_tmp.as_deref(),
    );
    set_string_env(
        &mut envs,
        "ANSIBLE_EXECUTABLE",
        settings.shell_executable.as_deref(),
    );
    set_string_env(&mut envs, "ANSIBLE_STRATEGY", settings.strategy.as_deref());
    set_string_env(
        &mut envs,
        "ANSIBLE_STDOUT_CALLBACK",
        settings.stdout_callback.as_deref(),
    );
    for (name, value) in &job.playbook_env {
        envs.insert(name.clone(), OsString::from(value));
    }
    // The environment outranks ansible.cfg, so the runner re-asserts both
    // pinned arg channels *after* `playbook_env` and drops every other
    // caller-controlled ssh surface. The sanitizer removes caller host-key
    // options and the pins are emitted on both `ssh_args` and
    // `ssh_common_args`, so the pin holds whichever occurrence OpenSSH
    // consults. Dropping the extra-args channels, the executable selectors and
    // the host-key-checking toggle keeps that guarantee independent of both
    // Ansible's argument ordering and OpenSSH's option handling.
    envs.insert(
        "ANSIBLE_SSH_ARGS".to_string(),
        OsString::from(effective_ssh_args(settings, &pdd.known_hosts)),
    );
    envs.insert(
        "ANSIBLE_SSH_COMMON_ARGS".to_string(),
        OsString::from(effective_ssh_common_args(settings, &pdd.known_hosts)),
    );
    for name in [
        "ANSIBLE_SSH_EXTRA_ARGS",
        "ANSIBLE_SCP_EXTRA_ARGS",
        "ANSIBLE_SFTP_EXTRA_ARGS",
        "ANSIBLE_SSH_EXECUTABLE",
        "ANSIBLE_SCP_EXECUTABLE",
        "ANSIBLE_SFTP_EXECUTABLE",
        "ANSIBLE_HOST_KEY_CHECKING",
        "ANSIBLE_SSH_HOST_KEY_CHECKING",
    ] {
        envs.remove(name);
    }
    if let Some(agent_env) = ssh_agent_env {
        for (name, value) in agent_env {
            envs.insert(name.clone(), OsString::from(value));
        }
    }
    envs
}

fn set_bool_env(envs: &mut BTreeMap<String, OsString>, name: &str, value: Option<bool>) {
    if let Some(value) = value {
        envs.insert(name.to_string(), OsString::from(bool_string(value)));
    }
}

fn set_string_env(envs: &mut BTreeMap<String, OsString>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        envs.insert(name.to_string(), OsString::from(value));
    }
}

fn bool_string(value: bool) -> String {
    if value {
        "True".to_string()
    } else {
        "False".to_string()
    }
}

fn start_ssh_agent(ssh_key: &str) -> anyhow::Result<SshAgentGuard> {
    let output = StdCommand::new("ssh-agent")
        .arg("-s")
        .output()
        .context("run ssh-agent")?;
    if !output.status.success() {
        bail!(
            "ssh-agent failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8(output.stdout).context("ssh-agent output must be utf-8")?;
    let mut agent_env = parse_ssh_agent_env(&stdout);
    if !agent_env.contains_key("SSH_AUTH_SOCK") || !agent_env.contains_key("SSH_AGENT_PID") {
        bail!("ssh-agent did not return SSH_AUTH_SOCK and SSH_AGENT_PID");
    }

    let mut child = StdCommand::new("ssh-add")
        .arg("-")
        .envs(agent_env.iter())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn ssh-add")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .context("ssh-add stdin was not captured")?;
        stdin
            .write_all(ssh_key.as_bytes())
            .context("write private key to ssh-add stdin")?;
        stdin.write_all(b"\n").context("finish ssh-add stdin")?;
    }
    let output = child.wait_with_output().context("wait ssh-add")?;
    if !output.status.success() {
        bail!(
            "ssh-add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    agent_env.insert(
        "SSH_AGENT_PID".to_string(),
        agent_env["SSH_AGENT_PID"].clone(),
    );
    Ok(SshAgentGuard { env: agent_env })
}

fn parse_ssh_agent_env(output: &str) -> BTreeMap<String, String> {
    let mut envs = BTreeMap::new();
    for segment in output.split(';') {
        let segment = segment.trim();
        for name in ["SSH_AUTH_SOCK", "SSH_AGENT_PID"] {
            if let Some(rest) = segment.strip_prefix(&format!("{name}=")) {
                envs.insert(name.to_string(), rest.trim().to_string());
            }
        }
    }
    envs
}

/// Remove ANSI/CSI escape sequences (`ESC '[' … <final byte 0x40..=0x7e>`) so the
/// structured stdout parser matches ansible's text regardless of colouring.
/// Borrows unchanged when there is nothing to strip. The raw passthrough keeps
/// the original coloured line — only the parser sees the stripped copy.
fn strip_ansi_codes(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // For a CSI sequence, drop params up to and including the final byte; a
        // non-CSI escape just drops the lone ESC.
        if chars.peek() == Some(&'[') {
            chars.next();
            for p in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&p) {
                    break;
                }
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

fn map_output_line(
    job: &RunnerJobRequest,
    settings: &RunnerSettings,
    config: &AppConfig,
    state: &mut ParseState,
    stream: OutputStream,
    line: &str,
) -> Option<RunnerGroupEvent> {
    // Ansible now emits ANSI colour (ANSIBLE_FORCE_COLOR); the raw passthrough
    // carries the coloured line to the UI, but this structured parser must match
    // against clean text, so strip escape sequences before any matching.
    let clean = strip_ansi_codes(line);
    let line = clean.as_ref();
    if line.trim().is_empty() {
        return None;
    }
    if settings.compact_events && state.suppress_next_continuation && is_continuation_line(line) {
        return None;
    }
    state.suppress_next_continuation = false;

    match stream {
        OutputStream::Stderr => map_stderr_line(job, settings, state, line),
        OutputStream::Stdout => map_stdout_line(job, settings, config, state, line),
    }
}

fn map_stderr_line(
    _job: &RunnerJobRequest,
    settings: &RunnerSettings,
    _state: &mut ParseState,
    line: &str,
) -> Option<RunnerGroupEvent> {
    if settings.compact_events && is_interpreter_warning(line) {
        return None;
    }
    let host = warning_host(line);
    Some(RunnerGroupEvent {
        run_id: None,
        host,
        event_kind: RunnerEventKind::Stderr,
        event_type: "stderr".to_string(),
        message: Some(compact_message(line)),
        payload: (!settings.omit_event_data).then(|| json!({"stream": "stderr"})),
    })
}

fn map_stdout_line(
    _job: &RunnerJobRequest,
    settings: &RunnerSettings,
    config: &AppConfig,
    state: &mut ParseState,
    line: &str,
) -> Option<RunnerGroupEvent> {
    if settings.compact_events && is_interpreter_warning(line) {
        return None;
    }
    if let Some(task) = task_name(line) {
        state.current_task = Some(task.clone());
        return Some(RunnerGroupEvent {
            run_id: None,
            host: None,
            event_kind: RunnerEventKind::Status,
            event_type: "task".to_string(),
            message: Some(format!("TASK [{task}]")),
            payload: None,
        });
    }
    if is_play_header(line) {
        return Some(RunnerGroupEvent {
            run_id: None,
            host: None,
            event_kind: RunnerEventKind::Status,
            event_type: "play".to_string(),
            message: Some(compact_message(line)),
            payload: None,
        });
    }
    if let Some((host, recap)) = recap_stats(line) {
        state.recap.insert(host.clone(), recap);
        return Some(RunnerGroupEvent {
            run_id: None,
            host: Some(host),
            event_kind: RunnerEventKind::Result,
            event_type: "playbook_on_stats".to_string(),
            message: Some("PLAY RECAP".to_string()),
            payload: (!settings.omit_event_data).then(|| {
                json!({
                    "ok": recap.ok,
                    "changed": recap.changed,
                    "unreachable": recap.unreachable,
                    "failed": recap.failed,
                    "skipped": recap.skipped,
                    "rescued": recap.rescued,
                    "ignored": recap.ignored,
                })
            }),
        });
    }
    if let Some((status, host, tail)) = ansible_result_line(line) {
        return Some(map_result_line(
            settings,
            config,
            state,
            line,
            ParsedResultLine {
                status: &status,
                host,
                tail: &tail,
            },
        ));
    }

    if settings.compact_events && !settings.include_internal_events && is_low_signal_line(line) {
        return None;
    }
    Some(RunnerGroupEvent {
        run_id: None,
        host: host_from_stdout(line),
        event_kind: RunnerEventKind::Stdout,
        event_type: "stdout".to_string(),
        message: Some(compact_message(line)),
        payload: (!settings.omit_event_data && !settings.compact_events)
            .then(|| json!({"stream": "stdout", "line": line})),
    })
}

/// Map one parsed per-host result line (`ok:`, `changed:`, `fatal:`, …) into a
/// typed result event, recording the observed outcome for the completion batch
/// and honoring the compact/omit payload settings. Split out of
/// [`map_stdout_line`] — it is the largest single parse branch.
/// One parsed per-host ansible result line: the status keyword, the host it
/// belongs to and the payload tail.
struct ParsedResultLine<'a> {
    status: &'a str,
    host: String,
    tail: &'a str,
}

fn map_result_line(
    settings: &RunnerSettings,
    config: &AppConfig,
    state: &mut ParseState,
    line: &str,
    result: ParsedResultLine<'_>,
) -> RunnerGroupEvent {
    let ParsedResultLine { status, host, tail } = result;
    let failure = matches!(status, "fatal" | "failed" | "unreachable");
    let run_status = if failure {
        RunnerRunStatus::Failed
    } else {
        RunnerRunStatus::Succeeded
    };
    state.observed.insert(
        host.clone(),
        ObservedOutcome {
            status: run_status,
            exit_code: if failure { 2 } else { 0 },
            error_message: failure.then(|| compact_message(line)),
        },
    );
    state.suppress_next_continuation = settings.compact_events
        && (tail.len() > config.result_force_compact_payload_bytes
            || tail.contains("ansible_facts"));
    if settings.only_failed_event_data && !failure {
        return RunnerGroupEvent {
            run_id: None,
            host: Some(host),
            event_kind: RunnerEventKind::Result,
            event_type: format!("runner_on_{status}"),
            message: Some(format!(
                "{status} [{}]",
                state.current_task.as_deref().unwrap_or("")
            )),
            payload: None,
        };
    }
    RunnerGroupEvent {
        run_id: None,
        host: Some(host),
        event_kind: RunnerEventKind::Result,
        event_type: format!("runner_on_{status}"),
        message: Some(compact_message(line)),
        payload: if settings.omit_event_data {
            None
        } else if settings.compact_events && !settings.full_result_payload {
            Some(json!({
                "stream": "stdout",
                "status": status,
                "task": state.current_task,
                "compacted": state.suppress_next_continuation,
            }))
        } else {
            Some(json!({"stream": "stdout", "line": line}))
        },
    }
}

async fn emit_launch_error(dispatcher: &EventDispatcher, _job: &RunnerJobRequest, message: &str) {
    dispatcher
        .enqueue(
            RunnerGroupEvent {
                run_id: None,
                host: None,
                event_kind: RunnerEventKind::System,
                event_type: "launch_failed".to_string(),
                message: Some(message.to_string()),
                payload: None,
            },
            true,
        )
        .await;
}

async fn send_group_status(
    dispatcher: &EventDispatcher,
    _job: &RunnerJobRequest,
    event_type: &str,
    message: Option<String>,
    payload: Value,
) {
    dispatcher
        .enqueue(
            RunnerGroupEvent {
                run_id: None,
                host: None,
                event_kind: RunnerEventKind::Status,
                event_type: event_type.to_string(),
                message,
                payload: Some(payload),
            },
            true,
        )
        .await;
}

async fn send_group_console_line(
    dispatcher: &EventDispatcher,
    _job: &RunnerJobRequest,
    message: String,
) {
    dispatcher
        .enqueue(
            RunnerGroupEvent {
                run_id: None,
                host: None,
                event_kind: RunnerEventKind::Stdout,
                event_type: "stdout".to_string(),
                message: Some(message),
                payload: Some(json!({"stream": "runner"})),
            },
            true,
        )
        .await;
}

async fn send_raw_console_line(
    dispatcher: &EventDispatcher,
    _job: &RunnerJobRequest,
    stream: OutputStream,
    line: &str,
) {
    let (event_kind, stream_name) = match stream {
        OutputStream::Stdout => (RunnerEventKind::Stdout, "stdout"),
        OutputStream::Stderr => (RunnerEventKind::Stderr, "stderr"),
    };
    dispatcher
        .enqueue(
            RunnerGroupEvent {
                run_id: None,
                host: None,
                event_kind,
                event_type: "console_line".to_string(),
                message: Some(line.to_string()),
                payload: Some(json!({"stream": stream_name, "raw": true, "scope": "global"})),
            },
            true,
        )
        .await;
}

fn ssh_connect_timeout_label(settings: &RunnerSettings) -> String {
    let Some(args) = &settings.ssh_common_args else {
        return "default".to_string();
    };
    let Some(start) = args.find("ConnectTimeout=") else {
        return "default".to_string();
    };
    let value = args[start + "ConnectTimeout=".len()..]
        .split_whitespace()
        .next()
        .unwrap_or("default")
        .trim_matches('"');
    format!("{value}s")
}

/// Why the completion-batch HTTP target could not be built.
enum CompletionTarget {
    Ready(reqwest::Client, Url),
    InvalidBaseUrl,
    ClientBuildFailed,
}

/// Build the completion-batch HTTP client and endpoint, or classify the
/// unusable callback base URL / client so the caller can log the exact cause.
fn completion_target(config: &AppConfig, job: &RunnerJobRequest) -> CompletionTarget {
    let Ok(base_url) = Url::parse(&job.callback_base_url) else {
        return CompletionTarget::InvalidBaseUrl;
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(config.callback_timeout)
        .build()
    else {
        return CompletionTarget::ClientBuildFailed;
    };
    let endpoint = join_url(
        &base_url,
        &format!(
            "/api/v1/playbooks/run-groups/{}/completion-batch",
            job.run_group_id
        ),
    );
    CompletionTarget::Ready(client, endpoint)
}

fn ready_completion_target(
    config: &AppConfig,
    job: &RunnerJobRequest,
) -> Option<(reqwest::Client, Url)> {
    let reason = match completion_target(config, job) {
        CompletionTarget::Ready(client, endpoint) => return Some((client, endpoint)),
        CompletionTarget::InvalidBaseUrl => "invalid callback_base_url",
        CompletionTarget::ClientBuildFailed => "failed to build HTTP client",
    };
    warn!("cannot send completion batch: {reason}");
    None
}

async fn send_completion_batch(
    config: &AppConfig,
    job: &RunnerJobRequest,
    parse_state: &ParseState,
    fallback_status: RunnerRunStatus,
    fallback_exit_code: Option<i32>,
    fallback_error_message: Option<&str>,
) {
    let runs = completion_items(
        job,
        parse_state,
        fallback_status,
        fallback_exit_code,
        fallback_error_message,
    );
    let Some((client, endpoint)) = ready_completion_target(config, job) else {
        return;
    };
    let body = RunnerGroupCompletionBatch {
        lease_owner: job.lease_owner.clone(),
        runs,
        new_host_keys: new_host_keys_for_job(config, job),
    };
    if let Err(error) =
        post_json_with_retry(&client, endpoint, &job.group_callback_token, &body).await
    {
        warn!(error = %error, "failed to post runner completion batch");
    }
}

fn completion_items(
    job: &RunnerJobRequest,
    parse_state: &ParseState,
    fallback_status: RunnerRunStatus,
    fallback_exit_code: Option<i32>,
    fallback_error_message: Option<&str>,
) -> Vec<RunnerGroupCompletionItem> {
    let mut items = Vec::new();
    for target in &job.targets {
        let observed = parse_state.observed.get(&target.alias);
        let recap = parse_state.recap.get(&target.alias);
        let (status, exit_code, error_message) = completion_from_observation(
            observed,
            recap,
            fallback_status,
            fallback_exit_code,
            fallback_error_message,
        );
        items.push(RunnerGroupCompletionItem {
            run_id: target.run_id,
            status,
            exit_code,
            error_message,
        });
    }
    items
}

fn completion_from_observation(
    observed: Option<&ObservedOutcome>,
    recap: Option<&RecapStats>,
    fallback_status: RunnerRunStatus,
    fallback_exit_code: Option<i32>,
    fallback_error_message: Option<&str>,
) -> (RunnerRunStatus, Option<i32>, Option<String>) {
    if let Some(recap) = recap {
        if recap.failed > 0 || recap.unreachable > 0 {
            return (
                RunnerRunStatus::Failed,
                fallback_exit_code.or(Some(2)),
                fallback_error_message.map(ToOwned::to_owned),
            );
        }
        if recap.ok > 0
            || recap.changed > 0
            || recap.skipped > 0
            || recap.rescued > 0
            || recap.ignored > 0
        {
            return (RunnerRunStatus::Succeeded, Some(0), None);
        }
    }
    if let Some(observed) = observed {
        return (
            observed.status,
            Some(observed.exit_code),
            observed.error_message.clone(),
        );
    }
    // A target that never appeared in the recap or in any per-task result must
    // not inherit a group-level success: `rc=0` only proves the playbook ran,
    // not that this host was part of it (limit mismatch, unparsable callback
    // output, host silently skipped by the play).
    if recap.is_none()
        && observed.is_none()
        && matches!(fallback_status, RunnerRunStatus::Succeeded)
    {
        return (
            RunnerRunStatus::Failed,
            None,
            Some(MISSING_TARGET_OUTPUT_MESSAGE.to_string()),
        );
    }
    (
        fallback_status,
        fallback_exit_code,
        fallback_error_message.map(ToOwned::to_owned),
    )
}

async fn flush_event_batch(
    client: &reqwest::Client,
    base_url: &Url,
    run_group_id: Uuid,
    group_callback_token: &str,
    batch: &mut Vec<EventEnvelope>,
) {
    if batch.is_empty() {
        return;
    }
    let events = std::mem::take(batch);
    let endpoint = group_event_batch_endpoint(base_url, run_group_id);
    // Assign the idempotency key once here, outside `post_json_with_retry`, so
    // every retry of this batch carries the same `batch_id` and the backend
    // dedups a lost-ACK re-delivery instead of appending duplicate events.
    let body = RunnerGroupEventBatch {
        batch_id: Uuid::now_v7(),
        events: events.into_iter().map(|event| event.event).collect(),
    };
    if let Err(error) = post_json_with_retry(client, endpoint, group_callback_token, &body).await {
        warn!(error = %error, "failed to post runner event batch");
    }
}

async fn post_json_with_retry<T: Serialize + ?Sized>(
    client: &reqwest::Client,
    url: Url,
    token: &str,
    body: &T,
) -> anyhow::Result<()> {
    let mut last_error = None;
    for attempt in 1..=3 {
        match client
            .post(url.clone())
            .bearer_auth(token)
            .json(body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                last_error = Some(anyhow!("callback returned {status}: {text}"));
            }
            Err(error) => last_error = Some(anyhow!(error)),
        }
        if attempt < 3 {
            sleep(Duration::from_millis(100 * attempt)).await;
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("callback failed")))
}

fn group_event_batch_endpoint(base: &Url, run_group_id: Uuid) -> Url {
    join_url(
        base,
        &format!("/api/v1/playbooks/run-groups/{run_group_id}/events/batch"),
    )
}

fn join_url(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_path(path);
    url.set_query(None);
    url
}

fn enrich_event_run_id(job: &RunnerJobRequest, event: &mut RunnerGroupEvent) {
    if event.run_id.is_some() {
        return;
    }
    let Some(host) = event.host.as_deref() else {
        return;
    };
    if let Some(target) = job.targets.iter().find(|target| target.alias == host) {
        event.run_id = Some(target.run_id);
    }
}

fn validate_job(job: &RunnerJobRequest) -> Result<(), RunnerError> {
    if job.protocol_version != RUNNER_CONTRACT_VERSION {
        return Err(RunnerError::BadRequest(format!(
            "unsupported protocol_version {}; expected {RUNNER_CONTRACT_VERSION}",
            job.protocol_version
        )));
    }
    if job.dispatch_attempt < 1 {
        return Err(RunnerError::BadRequest(
            "dispatch_attempt must be at least 1".to_string(),
        ));
    }
    validated_project_path(
        job.entrypoint
            .as_deref()
            .unwrap_or(DEFAULT_PROJECT_ENTRYPOINT),
        "entrypoint",
    )
    .map_err(|error| RunnerError::BadRequest(error.to_string()))?;
    if job.project_files.is_empty() && job.playbook.trim().is_empty() {
        return Err(RunnerError::BadRequest(
            "playbook must not be empty when project_files is empty".to_string(),
        ));
    }
    validate_project_bundle(&job.project_files)
        .map_err(|error| RunnerError::BadRequest(error.to_string()))?;
    if !job.project_files.is_empty()
        && !project_bundle_contains_path(
            &job.project_files,
            &validated_project_path(
                job.entrypoint
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ENTRYPOINT),
                "entrypoint",
            )
            .map_err(|error| RunnerError::BadRequest(error.to_string()))?,
        )
        .map_err(|error| RunnerError::BadRequest(error.to_string()))?
        && job.playbook.trim().is_empty()
    {
        return Err(RunnerError::BadRequest(format!(
            "project entrypoint {} is not present in project_files",
            job.entrypoint
                .as_deref()
                .unwrap_or(DEFAULT_PROJECT_ENTRYPOINT)
        )));
    }
    if job.inventory.trim().is_empty() {
        return Err(RunnerError::BadRequest(
            "inventory must not be empty".to_string(),
        ));
    }
    if job.targets.is_empty() {
        return Err(RunnerError::BadRequest(
            "targets must contain at least one run".to_string(),
        ));
    }
    if job.callback_base_url.trim().is_empty() {
        return Err(RunnerError::BadRequest(
            "callback_base_url must not be empty".to_string(),
        ));
    }
    Url::parse(&job.callback_base_url).map_err(|error| {
        RunnerError::BadRequest(format!("callback_base_url is not a valid URL: {error}"))
    })?;
    if job.group_callback_token.trim().is_empty() {
        return Err(RunnerError::BadRequest(
            "group_callback_token must not be empty".to_string(),
        ));
    }
    Ok(())
}

/// Compare a presented credential against the expected one without leaking how
/// far the match got.
///
/// The length check is deliberately outside the constant-time part: the length
/// of the expected value is not the secret, and `ct_eq` requires equal-length
/// slices anyway.
fn secret_eq(presented: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    presented.len() == expected.len() && bool::from(presented.as_bytes().ct_eq(expected.as_bytes()))
}

fn require_auth(headers: &HeaderMap, shared_secret: &str) -> Result<(), RunnerError> {
    let expected = format!("Bearer {shared_secret}");
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if authorization.is_some_and(|value| secret_eq(value, &expected)) {
        return Ok(());
    }
    Err(RunnerError::Unauthorized(
        "missing or invalid runner authorization".to_string(),
    ))
}

fn cleanup_private_data_dir(config: &AppConfig, pdd: &PrivateDataDir) {
    if config.keep_data_dir {
        return;
    }
    if let Err(error) = fs::remove_dir_all(&pdd.root) {
        warn!(path = %pdd.root.display(), error = %error, "failed to remove ansible private data dir");
    }
}

fn optional_bool(map: &Map<String, Value>, name: &str) -> Result<Option<bool>, RunnerError> {
    match map.get(name) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(RunnerError::BadRequest(format!(
            "settings.{name} must be boolean"
        ))),
    }
}

fn optional_u64(map: &Map<String, Value>, name: &str) -> Result<Option<u64>, RunnerError> {
    match map.get(name) {
        Some(Value::Number(value)) => value.as_u64().map(Some).ok_or_else(|| {
            RunnerError::BadRequest(format!("settings.{name} must be a positive integer"))
        }),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(RunnerError::BadRequest(format!(
            "settings.{name} must be an integer"
        ))),
    }
}

fn optional_string(map: &Map<String, Value>, name: &str) -> Result<Option<String>, RunnerError> {
    match map.get(name) {
        Some(Value::String(value)) => Ok((!value.trim().is_empty()).then(|| value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(RunnerError::BadRequest(format!(
            "settings.{name} must be a string"
        ))),
    }
}

fn env_string(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn required_env(name: &str) -> anyhow::Result<String> {
    env::var(name)
        .with_context(|| format!("{name} is required"))
        .and_then(|value| {
            if value.trim().is_empty() {
                bail!("{name} must not be empty");
            }
            Ok(value)
        })
}

fn env_u64(name: &str, default: u64) -> anyhow::Result<u64> {
    match env::var(name) {
        Ok(value) => value.parse().with_context(|| format!("{name} must be u64")),
        Err(_) => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> anyhow::Result<usize> {
    match env::var(name) {
        Ok(value) => value
            .parse()
            .with_context(|| format!("{name} must be usize")),
        Err(_) => Ok(default),
    }
}

fn env_bool(name: &str, default: bool) -> anyhow::Result<bool> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => bail!("{name} must be boolean"),
        },
        Err(_) => Ok(default),
    }
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

async fn terminate_process_group(pid: u32) {
    // Negative pid addresses the whole process group; cast_signed keeps the
    // u32→i32 reinterpretation explicit for pedantic clippy.
    let process_group = Pid::from_raw(-pid.cast_signed());
    if let Err(error) = kill(process_group, Signal::SIGTERM) {
        warn!(pid, error = %error, "failed to send SIGTERM to ansible process group");
    }
    sleep(Duration::from_millis(700)).await;
    let _ = kill(process_group, Signal::SIGKILL);
}

fn task_name(line: &str) -> Option<String> {
    static TASK_RE: std::sync::OnceLock<Option<Regex>> = std::sync::OnceLock::new();
    let regex = TASK_RE
        .get_or_init(|| Regex::new(r"^\s*(TASK|RUNNING HANDLER)\s+\[(?P<task>.+?)\]\s+\*+").ok())
        .as_ref()?;
    regex
        .captures(line)
        .and_then(|caps| caps.name("task"))
        .map(|task| task.as_str().to_string())
}

fn is_play_header(line: &str) -> bool {
    line.trim_start().starts_with("PLAY [")
}

fn host_from_stdout(line: &str) -> Option<String> {
    static HOST_RE: std::sync::OnceLock<Option<Regex>> = std::sync::OnceLock::new();
    let regex = HOST_RE
        .get_or_init(|| Regex::new(r"(?:fatal|failed|unreachable|ok|changed|skipping|included|included_file|included_vars):\s+\[([^\]]+)\]").ok())
        .as_ref()?;
    regex
        .captures(line)
        .and_then(|caps| caps.get(1))
        .map(|host| normalize_host_alias(host.as_str()))
}

fn warning_host(line: &str) -> Option<String> {
    static WARNING_HOST_RE: std::sync::OnceLock<Option<Regex>> = std::sync::OnceLock::new();
    let regex = WARNING_HOST_RE
        .get_or_init(|| Regex::new(r"\[WARNING\]:\s+Host\s+'([^']+)'").ok())
        .as_ref()?;
    regex
        .captures(line)
        .and_then(|caps| caps.get(1))
        .map(|host| normalize_host_alias(host.as_str()))
}

fn ansible_result_line(line: &str) -> Option<(String, String, String)> {
    static RESULT_RE: std::sync::OnceLock<Option<Regex>> = std::sync::OnceLock::new();
    let regex = RESULT_RE
        .get_or_init(|| {
            Regex::new(
                r"^\s*(ok|changed|skipping|fatal|failed|unreachable):\s+\[([^\]]+)\](?P<tail>.*)$",
            )
            .ok()
        })
        .as_ref()?;
    let captures = regex.captures(line)?;
    Some((
        captures.get(1)?.as_str().to_string(),
        normalize_host_alias(captures.get(2)?.as_str()),
        captures
            .name("tail")
            .map(|tail| tail.as_str().to_string())
            .unwrap_or_default(),
    ))
}

fn recap_stats(line: &str) -> Option<(String, RecapStats)> {
    static RECAP_RE: std::sync::OnceLock<Option<Regex>> = std::sync::OnceLock::new();
    let regex = RECAP_RE
        .get_or_init(|| {
            Regex::new(
                r"^\s*(?P<host>\S+)\s*:\s*ok=(?P<ok>\d+)\s+changed=(?P<changed>\d+)\s+unreachable=(?P<unreachable>\d+)\s+failed=(?P<failed>\d+)\s+skipped=(?P<skipped>\d+)\s+rescued=(?P<rescued>\d+)\s+ignored=(?P<ignored>\d+)",
            )
            .ok()
        })
        .as_ref()?;
    let captures = regex.captures(line)?;
    Some((
        normalize_host_alias(captures.name("host")?.as_str()),
        RecapStats {
            ok: capture_u32(&captures, "ok")?,
            changed: capture_u32(&captures, "changed")?,
            unreachable: capture_u32(&captures, "unreachable")?,
            failed: capture_u32(&captures, "failed")?,
            skipped: capture_u32(&captures, "skipped")?,
            rescued: capture_u32(&captures, "rescued")?,
            ignored: capture_u32(&captures, "ignored")?,
        },
    ))
}

fn capture_u32(captures: &regex::Captures<'_>, name: &str) -> Option<u32> {
    captures.name(name)?.as_str().parse().ok()
}

fn normalize_host_alias(host: &str) -> String {
    host.split(" -> ").next().unwrap_or(host).trim().to_string()
}

fn compact_message(line: &str) -> String {
    const LIMIT: usize = 2_000;
    if line.len() <= LIMIT {
        return line.to_string();
    }
    let mut value = line.chars().take(LIMIT).collect::<String>();
    value.push('…');
    value
}

fn is_continuation_line(line: &str) -> bool {
    line.starts_with(' ')
        || line.starts_with('\t')
        || line.trim_start().starts_with('"')
        || line.trim_start().starts_with('{')
        || line.trim_start().starts_with('}')
        || line.trim_start().starts_with('[')
        || line.trim_start().starts_with(']')
}

fn is_interpreter_warning(line: &str) -> bool {
    line.contains("interpreter") && line.contains("discovered")
}

fn is_low_signal_line(line: &str) -> bool {
    line.trim_start().starts_with("runner_on_") || is_continuation_line(line)
}

#[cfg(test)]
mod tests {

    /// The property this file cares about is timing, and no runtime assertion can
    /// observe it: `==` and `secret_eq` return the same answers, so a behavioural
    /// test stays green when the comparison is reverted. Planting that revert
    /// proved it — three green tests over a leaking comparison. This reads the
    /// source instead, which is the only thing here that can actually fail.
    #[test]
    fn require_auth_compares_the_bearer_token_in_constant_time() {
        let source = include_str!("main.rs");
        let body = source
            .split_once("fn require_auth(")
            .expect("require_auth must exist")
            .1
            .split_once("\n}")
            .expect("require_auth must be a complete function")
            .0;
        assert!(
            body.contains("secret_eq("),
            "require_auth must compare the bearer token with secret_eq; a plain \
             `==` short-circuits on the first differing byte and leaks how much \
             of the shared secret a caller guessed"
        );
    }

    #[test]
    fn secret_eq_matches_only_the_exact_value() {
        assert!(secret_eq("Bearer abc123", "Bearer abc123"));
        // Same length, differing in the last byte: the case a short-circuiting
        // `==` would answer fastest and a constant-time compare must not.
        assert!(!secret_eq("Bearer abc123", "Bearer abc124"));
        // Differing in the first byte, same length.
        assert!(!secret_eq("Bearer abc123", "bearer abc123"));
        assert!(!secret_eq("Bearer abc123", "Bearer abc1234"));
        assert!(!secret_eq("", "Bearer abc123"));
        assert!(secret_eq("", ""));
    }

    #[test]
    fn require_auth_rejects_a_same_length_near_miss() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer 0123456789abcdef".parse().expect("header value"),
        );
        require_auth(&headers, "0123456789abcdef").expect("the exact secret is accepted");
        assert!(require_auth(&headers, "0123456789abcdee").is_err());
    }
    use super::*;

    #[test]
    fn strip_ansi_codes_removes_sgr_but_preserves_text_and_no_ops_plain() {
        // Plain text is borrowed unchanged (all pre-colour stored runs).
        assert!(matches!(
            strip_ansi_codes("ok: [host] => changed"),
            std::borrow::Cow::Borrowed(_)
        ));
        // Multi-param SGR + reset, both around the ansible status prefix, so the
        // parser can still match `ok:` / `PLAY RECAP` after stripping.
        assert_eq!(
            strip_ansi_codes("\u{1b}[0;32mok: [host]\u{1b}[0m"),
            "ok: [host]"
        );
        assert_eq!(
            strip_ansi_codes("\u{1b}[1;35mchanged: [host]\u{1b}[m"),
            "changed: [host]"
        );
        // A non-SGR CSI (cursor move) must not leak a stray final byte.
        assert_eq!(strip_ansi_codes("a\u{1b}[2Kb"), "ab");
    }

    #[test]
    fn ansible_text_parsers_recognise_tasks_plays_and_results() {
        assert_eq!(
            task_name("TASK [Install package] ********************************"),
            Some("Install package".to_string())
        );
        assert_eq!(
            task_name("RUNNING HANDLER [Restart service] *********************"),
            Some("Restart service".to_string())
        );
        assert!(task_name("TASK malformed").is_none());
        assert!(is_play_header("   PLAY [edge] ****"));
        assert!(!is_play_header("PLAYBOOK: site.yml"));

        let delegated = "changed: [host1 -> localhost] => {\"changed\": true}";
        assert_eq!(host_from_stdout(delegated).as_deref(), Some("host1"));
        assert_eq!(
            warning_host("[WARNING]: Host 'host2 -> bastion' discovered interpreter").as_deref(),
            Some("host2")
        );
        let (status, host, tail) = ansible_result_line(delegated).expect("result line");
        assert_eq!(status, "changed");
        assert_eq!(host, "host1");
        assert!(tail.contains("changed"));
        assert!(ansible_result_line("ordinary stdout").is_none());
    }

    #[test]
    fn recap_stats_parse_all_outcome_counters_and_reject_overflow() {
        let (recap_host, recap) = recap_stats(
            "host1 : ok=3 changed=2 unreachable=1 failed=4 skipped=5 rescued=6 ignored=7",
        )
        .expect("recap");
        assert_eq!(recap_host, "host1");
        assert_eq!(recap.ok, 3);
        assert_eq!(recap.changed, 2);
        assert_eq!(recap.unreachable, 1);
        assert_eq!(recap.failed, 4);
        assert_eq!(recap.skipped, 5);
        assert_eq!(recap.rescued, 6);
        assert_eq!(recap.ignored, 7);
        assert!(recap_stats(
            "host1 : ok=4294967296 changed=0 unreachable=0 failed=0 skipped=0 rescued=0 ignored=0"
        )
        .is_none());
    }

    #[test]
    fn ansible_text_parsers_handle_compact_messages_and_agent_env() {
        assert_eq!(compact_message("short"), "short");
        let compacted = compact_message(&"x".repeat(2_001));
        assert_eq!(compacted.chars().count(), 2_001);
        assert!(compacted.ends_with('…'));
        assert!(is_continuation_line("  detail"));
        assert!(is_continuation_line("{\"detail\": true}"));
        assert!(is_interpreter_warning(
            "Python interpreter was discovered on host"
        ));
        assert!(is_low_signal_line("runner_on_ok"));
        assert!(!is_low_signal_line("changed: [host1]"));

        let envs = parse_ssh_agent_env(
            "SSH_AUTH_SOCK=/tmp/agent.sock; export SSH_AUTH_SOCK; SSH_AGENT_PID=42; export SSH_AGENT_PID;",
        );
        assert_eq!(
            envs.get("SSH_AUTH_SOCK").map(String::as_str),
            Some("/tmp/agent.sock")
        );
        assert_eq!(envs.get("SSH_AGENT_PID").map(String::as_str), Some("42"));

        let settings = RunnerSettings::parse(&json!({})).expect("settings");
        let config = test_config(PathBuf::from("/unused"));
        let job = sample_job(json!({}));
        let mut state = ParseState::default();
        let stderr = map_output_line(
            &job,
            &settings,
            &config,
            &mut state,
            OutputStream::Stderr,
            "[WARNING]: Host 'host1' uses a fallback interpreter",
        )
        .expect("stderr event");
        assert_eq!(stderr.host.as_deref(), Some("host1"));
        assert_eq!(stderr.event_kind, RunnerEventKind::Stderr);
        assert_eq!(stderr.payload.expect("stderr payload")["stream"], "stderr");

        assert!(
            map_output_line(
                &job,
                &settings,
                &config,
                &mut ParseState::default(),
                OutputStream::Stdout,
                "   ",
            )
            .is_none()
        );
    }

    #[test]
    fn map_output_line_compacts_failure_and_success_events() {
        let config = test_config(PathBuf::from("/unused"));
        let job = sample_job(json!({}));
        let compact = RunnerSettings::parse(&json!({
            "compact_events": true,
            "only_failed_event_data": true
        }))
        .expect("compact settings");
        let mut compact_state = ParseState::default();
        let task = map_output_line(
            &job,
            &compact,
            &config,
            &mut compact_state,
            OutputStream::Stdout,
            "TASK [Deploy] ****",
        )
        .expect("task event");
        assert_eq!(task.event_type, "task");
        assert_eq!(compact_state.current_task.as_deref(), Some("Deploy"));

        let play = map_output_line(
            &job,
            &compact,
            &config,
            &mut compact_state,
            OutputStream::Stdout,
            "PLAY [edge] ****",
        )
        .expect("play event");
        assert_eq!(play.event_type, "play");

        let failure = map_output_line(
            &job,
            &compact,
            &config,
            &mut compact_state,
            OutputStream::Stdout,
            "fatal: [host1]: FAILED! => {\"ansible_facts\": {\"secret\": true}}",
        )
        .expect("failure event");
        assert_eq!(failure.event_type, "runner_on_fatal");
        assert_eq!(
            failure.payload.expect("compact failure payload")["status"],
            "fatal"
        );
        let observed = compact_state
            .observed
            .get("host1")
            .expect("failure recorded");
        assert_eq!(observed.status, RunnerRunStatus::Failed);
        assert_eq!(observed.exit_code, 2);
        assert!(observed.error_message.is_some());
        assert!(compact_state.suppress_next_continuation);
        assert!(
            map_output_line(
                &job,
                &compact,
                &config,
                &mut compact_state,
                OutputStream::Stdout,
                "  hidden continuation",
            )
            .is_none()
        );

        let successful = map_output_line(
            &job,
            &compact,
            &config,
            &mut compact_state,
            OutputStream::Stdout,
            "ok: [host2] => {\"changed\": false}",
        )
        .expect("successful compact result");
        assert!(successful.payload.is_none());
        assert!(
            map_output_line(
                &job,
                &compact,
                &config,
                &mut compact_state,
                OutputStream::Stdout,
                "runner_on_ok",
            )
            .is_none()
        );
        assert!(
            map_output_line(
                &job,
                &compact,
                &config,
                &mut compact_state,
                OutputStream::Stderr,
                "[WARNING]: Python interpreter was discovered on host1",
            )
            .is_none()
        );
    }

    fn test_config(data_dir: PathBuf) -> AppConfig {
        AppConfig {
            bind: DEFAULT_BIND.to_string(),
            shared_secret: "secret".to_string(),
            data_dir,
            callback_timeout: Duration::from_secs(1),
            event_queue_size: 10,
            event_batch_size: 10,
            event_batch_flush: Duration::from_millis(10),
            event_drain_timeout: Duration::from_secs(1),
            result_force_compact_payload_bytes: 32,
            keep_data_dir: true,
            default_job_timeout: Duration::from_secs(DEFAULT_JOB_TIMEOUT_SECONDS),
            output_drain_grace: Duration::from_secs(DEFAULT_OUTPUT_DRAIN_GRACE_SECONDS),
            dispatch_receipt_retention: Duration::from_secs(
                DEFAULT_DISPATCH_RECEIPT_RETENTION_SECONDS,
            ),
            galaxy_cache_dir: data_dir_for_galaxy_cache(),
            galaxy_install_timeout: Duration::from_secs(DEFAULT_GALAXY_INSTALL_TIMEOUT_SECONDS),
        }
    }

    fn data_dir_for_galaxy_cache() -> PathBuf {
        env::temp_dir().join(format!("piping-galaxy-cache-{}", Uuid::now_v7()))
    }

    fn sample_job(settings: Value) -> RunnerJobRequest {
        RunnerJobRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            dispatch_id: Uuid::from_u128(1),
            dispatch_attempt: 1,
            run_group_id: Uuid::nil(),
            group_callback_token: "group-token".to_string(),
            lease_owner: "worker".to_string(),
            callback_base_url: "http://backend:7000".to_string(),
            playbook: "---\n- hosts: all\n  tasks: []\n".to_string(),
            entrypoint: None,
            project_files: Vec::new(),
            inventory: "host1 ansible_connection=local\n".to_string(),
            targets: vec![RunnerJobTarget {
                alias: "host1".to_string(),
                run_id: Uuid::from_u128(2),
                job_token: "target-token".to_string(),
                target_kind: "connection_target".to_string(),
                target_id: Uuid::from_u128(3),
                source_kind: "device".to_string(),
                source_id: Some(Uuid::from_u128(4)),
                ssh_private_key: None,
                known_hosts_lines: Vec::new(),
            }],
            extravars: json!({"example": true}),
            ssh_key: None,
            passwords: BTreeMap::new(),
            settings,
            playbook_env: BTreeMap::new(),
        }
    }

    fn authorized_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer secret".parse().expect("authorization header"),
        );
        headers
    }

    fn write_fake_ansible_playbook(root: &Path) -> PathBuf {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create fake ansible bin");
        let executable = bin.join("ansible-playbook");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'TASK [Probe] ****\\n'\nprintf 'changed: [host1] => {\"changed\": true}\\n'\nprintf 'host1 : ok=1 changed=1 unreachable=0 failed=0 skipped=0 rescued=0 ignored=0\\n'\nprintf 'diagnostic\\n' >&2\n",
        )
        .expect("write fake ansible-playbook");
        let mut permissions = fs::metadata(&executable)
            .expect("fake executable metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("make fake ansible executable");
        bin
    }

    fn job_control(
        receipt: RunnerDispatchReceipt,
        status: RunnerJobState,
        rc: Option<i32>,
        process_id: Option<u32>,
    ) -> JobControl {
        JobControl {
            receipt,
            status: Arc::new(RwLock::new(status)),
            rc: Arc::new(RwLock::new(rc)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            process_id: Arc::new(RwLock::new(process_id)),
            terminal_transition_at_unix_s: Arc::new(RwLock::new(
                runner_job_state_is_terminal(status)
                    .then(|| current_unix_time_seconds().expect("current Unix time")),
            )),
        }
    }

    fn typed_settings() -> RunnerSettings {
        RunnerSettings::parse(&json!({
            "execution_backend": "native",
            "forks": 0,
            "verbosity": 99,
            "job_timeout": 0,
            "ansible_timeout": 17,
            "limit": "edge",
            "tags": "safe",
            "skip_tags": "slow",
            "start_at_task": "Deploy",
            "check": true,
            "diff": true,
            "compact_events": true,
            "raw_stdout_passthrough": false,
            "omit_event_data": true,
            "only_failed_event_data": true,
            "include_internal_events": true,
            "full_result_payload": true,
            "deprecation_warnings": false,
            "display_ok_hosts": false,
            "display_skipped_hosts": true,
            "pipelining": true,
            "python_interpreter": "/usr/bin/python3",
            "remote_tmp": "/tmp/remote",
            "shell_executable": "/bin/sh",
            "ssh_args": "-o BatchMode=yes",
            "ssh_common_args": "-o ConnectTimeout=17",
            "strategy": "free",
            "stdout_callback": "default"
        }))
        .expect("typed settings")
    }

    #[test]
    fn settings_parser_clamps_numeric_boundaries() {
        let minimum = typed_settings();
        assert_eq!(minimum.forks, Some(1));
        assert_eq!(minimum.verbosity, Some(6));
        assert_eq!(minimum.job_timeout, Some(Duration::ZERO));
        assert_eq!(minimum.ansible_timeout_seconds, Some(17));
        assert!(minimum.check && minimum.diff && minimum.compact_events);
        assert!(!minimum.raw_stdout_passthrough);
        assert!(minimum.omit_event_data && minimum.only_failed_event_data);
        assert!(minimum.include_internal_events && minimum.full_result_payload);

        let maximum = RunnerSettings::parse(&json!({"forks": u64::MAX}))
            .expect("oversized forks are clamped");
        assert_eq!(maximum.forks, Some(u32::MAX));
        assert!(RunnerSettings::parse(&Value::Null).is_ok());
    }

    #[test]
    fn settings_parser_rejects_invalid_shapes() {
        for (value, message) in [
            (json!([]), "JSON object"),
            (json!({"cmdline": "--check"}), "intentionally unsupported"),
            (json!({"check": "yes"}), "must be boolean"),
            (json!({"forks": -1}), "positive integer"),
            (json!({"strategy": 1}), "must be a string"),
        ] {
            let error = RunnerSettings::parse(&value).expect_err("invalid settings rejected");
            assert!(
                error.to_string().contains(message),
                "expected {message:?} in {error}"
            );
        }
    }

    #[test]
    fn ansible_config_renders_typed_settings() {
        let minimum = typed_settings();
        let config = build_ansible_config(
            Path::new("/tmp/local"),
            &minimum,
            Path::new("/tmp/job/known_hosts"),
        )
        .expect("typed settings render as ansible.cfg");
        for expected in [
            "strategy = free",
            "stdout_callback = default",
            "remote_tmp = /tmp/remote",
            "system_tmpdirs = /tmp/remote",
            "executable = /bin/sh",
            "deprecation_warnings = False",
            "display_ok_hosts = False",
            "display_skipped_hosts = True",
            "pipelining = True",
            "ssh_args = -o BatchMode=yes",
            "ssh_common_args = -o ConnectTimeout=17",
        ] {
            assert!(
                config.contains(expected),
                "missing {expected:?} in {config}"
            );
        }
        assert!(config_value("unsafe\nsetting").is_err());
    }

    #[test]
    fn ansible_config_never_sets_host_key_checking_false() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        // A request may still carry the legacy toggle; the runner must ignore it
        // and never emit a fail-open host-key switch.
        let job = sample_job(json!({
            "host_key_checking": false,
            "ssh_args": "-o BatchMode=yes",
        }));
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let ansible_cfg = fs::read_to_string(pdd.project.join("ansible.cfg")).expect("ansible cfg");

        assert!(
            !ansible_cfg.contains("host_key_checking"),
            "ansible.cfg must not emit host_key_checking: {ansible_cfg}"
        );
        let env = build_playbook_envvars(&job, &settings, &pdd, None, None);
        assert!(
            !env.contains_key("ANSIBLE_HOST_KEY_CHECKING"),
            "runner must not export ANSIBLE_HOST_KEY_CHECKING: {env:?}"
        );
    }

    #[test]
    fn known_hosts_file_is_materialized_and_referenced_in_ssh_args() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.targets[0].known_hosts_lines = vec![
            "host1 ssh-ed25519 AAAASEEDONE".to_string(),
            "host1 ssh-rsa AAAASEEDTWO".to_string(),
        ];
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");

        let seed = fs::read_to_string(&pdd.known_hosts).expect("job known_hosts");
        assert!(seed.contains("host1 ssh-ed25519 AAAASEEDONE"));
        assert!(seed.contains("host1 ssh-rsa AAAASEEDTWO"));
        let mode = fs::metadata(&pdd.known_hosts)
            .expect("known_hosts metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "job known_hosts must be 0600");

        let ansible_cfg = fs::read_to_string(pdd.project.join("ansible.cfg")).expect("ansible cfg");
        let pin = format!("UserKnownHostsFile={}", pdd.known_hosts.display());
        assert!(
            ansible_cfg.contains(&pin),
            "missing {pin:?} in {ansible_cfg}"
        );
        for expected in [
            "StrictHostKeyChecking=accept-new",
            "GlobalKnownHostsFile=/dev/null",
            "HashKnownHosts=no",
        ] {
            assert!(
                ansible_cfg.contains(expected),
                "missing {expected:?} in {ansible_cfg}"
            );
        }
        assert!(!ansible_cfg.contains("StrictHostKeyChecking=no"));
        assert!(!ansible_cfg.contains("UserKnownHostsFile=/dev/null"));

        // Every ssh child reads the environment first, so the pins must survive
        // there too, next to any caller-supplied args.
        let env = build_playbook_envvars(&job, &settings, &pdd, None, None);
        let ssh_args = env
            .get("ANSIBLE_SSH_ARGS")
            .expect("ANSIBLE_SSH_ARGS is always pinned")
            .to_string_lossy();
        assert!(ssh_args.contains(&pin), "missing {pin:?} in {ssh_args}");
        assert!(ssh_args.contains("StrictHostKeyChecking=accept-new"));
    }

    #[test]
    fn caller_ssh_common_args_cannot_reenable_fail_open_host_key_checking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let fail_open =
            "-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null";
        let job = sample_job(json!({
            "ssh_args": fail_open,
            "ssh_common_args": fail_open,
        }));
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let ansible_cfg = fs::read_to_string(pdd.project.join("ansible.cfg")).expect("ansible cfg");
        let pin = format!("UserKnownHostsFile={}", pdd.known_hosts.display());

        assert!(
            ansible_cfg.contains("ssh_common_args = "),
            "ssh_common_args must always be emitted: {ansible_cfg}"
        );
        assert!(
            ansible_cfg.contains("StrictHostKeyChecking=accept-new"),
            "pins must survive in cfg: {ansible_cfg}"
        );
        assert!(
            ansible_cfg.contains(&pin),
            "missing {pin:?} in {ansible_cfg}"
        );
        assert!(
            ansible_cfg.contains("BatchMode=yes"),
            "caller token must be preserved: {ansible_cfg}"
        );
        assert!(
            !ansible_cfg.contains("StrictHostKeyChecking=no"),
            "fail-open host-key checking must not reach cfg: {ansible_cfg}"
        );
        assert!(
            !ansible_cfg.contains("UserKnownHostsFile=/dev/null"),
            "known_hosts must not be disabled in cfg: {ansible_cfg}"
        );

        let env = build_playbook_envvars(&job, &settings, &pdd, None, None);
        let common = env
            .get("ANSIBLE_SSH_COMMON_ARGS")
            .expect("ANSIBLE_SSH_COMMON_ARGS is always pinned")
            .to_string_lossy()
            .into_owned();
        assert!(
            common.contains("StrictHostKeyChecking=accept-new"),
            "pins must survive in ANSIBLE_SSH_COMMON_ARGS: {common}"
        );
        assert!(common.contains(&pin), "missing {pin:?} in {common}");
        assert!(
            common.contains("BatchMode=yes"),
            "caller token lost: {common}"
        );
        assert!(
            !common.contains("StrictHostKeyChecking=no"),
            "fail-open host-key checking must not reach the env: {common}"
        );
        assert!(
            !common.contains("UserKnownHostsFile=/dev/null"),
            "known_hosts must not be disabled in the env: {common}"
        );

        let ssh_args = env
            .get("ANSIBLE_SSH_ARGS")
            .expect("ANSIBLE_SSH_ARGS is always pinned")
            .to_string_lossy()
            .into_owned();
        assert!(
            !ssh_args.contains("StrictHostKeyChecking=no"),
            "fail-open host-key checking must not reach ANSIBLE_SSH_ARGS: {ssh_args}"
        );
        assert!(
            !ssh_args.contains("UserKnownHostsFile=/dev/null"),
            "known_hosts must not be disabled in ANSIBLE_SSH_ARGS: {ssh_args}"
        );
    }

    #[test]
    fn extravars_ssh_common_args_are_neutralised() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.extravars = json!({
            "keep": true,
            "ansible_ssh_common_args": "-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null",
            "ansible_ssh_args": "-o StrictHostKeyChecking=no",
            "ansible_ssh_extra_args": "-o UserKnownHostsFile=/dev/null",
        });
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let rendered = fs::read_to_string(pdd.env.join("extravars")).expect("extravars file");
        let extravars: Value = serde_json::from_str(&rendered).expect("extravars JSON");

        assert_eq!(extravars.get("keep"), Some(&Value::Bool(true)));
        for key in [
            "ansible_ssh_common_args",
            "ansible_ssh_args",
            "ansible_ssh_extra_args",
        ] {
            assert!(
                extravars.get(key).is_none(),
                "{key} must be dropped from extravars: {rendered}"
            );
        }
        assert!(
            !rendered.contains("StrictHostKeyChecking=no"),
            "extravars must not carry fail-open checking: {rendered}"
        );
        assert!(
            !rendered.contains("UserKnownHostsFile=/dev/null"),
            "extravars must not disable known_hosts: {rendered}"
        );
    }

    #[test]
    fn caller_cannot_select_a_custom_ssh_executable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.extravars = json!({
            "keep": true,
            "ansible_ssh_executable": "/tmp/wrapper-ssh",
            "ansible_scp_executable": "/tmp/wrapper-scp",
            "ansible_sftp_executable": "/tmp/wrapper-sftp",
        });
        job.playbook_env.insert(
            "ANSIBLE_SSH_EXECUTABLE".to_string(),
            "/tmp/wrapper-ssh".to_string(),
        );
        job.playbook_env.insert(
            "ANSIBLE_SCP_EXECUTABLE".to_string(),
            "/tmp/wrapper-scp".to_string(),
        );
        job.playbook_env.insert(
            "ANSIBLE_SFTP_EXECUTABLE".to_string(),
            "/tmp/wrapper-sftp".to_string(),
        );
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let rendered = fs::read_to_string(pdd.env.join("extravars")).expect("extravars file");
        let extravars: Value = serde_json::from_str(&rendered).expect("extravars JSON");
        assert_eq!(extravars.get("keep"), Some(&Value::Bool(true)));
        for key in [
            "ansible_ssh_executable",
            "ansible_scp_executable",
            "ansible_sftp_executable",
        ] {
            assert!(
                extravars.get(key).is_none(),
                "{key} must be dropped from extravars: {rendered}"
            );
        }

        let env = build_playbook_envvars(&job, &settings, &pdd, None, None);
        for name in [
            "ANSIBLE_SSH_EXECUTABLE",
            "ANSIBLE_SCP_EXECUTABLE",
            "ANSIBLE_SFTP_EXECUTABLE",
            "ANSIBLE_SSH_EXTRA_ARGS",
        ] {
            assert!(
                !env.contains_key(name),
                "{name} must not reach the playbook environment: {env:?}"
            );
        }
        let pin = format!("UserKnownHostsFile={}", pdd.known_hosts.display());
        for name in ["ANSIBLE_SSH_ARGS", "ANSIBLE_SSH_COMMON_ARGS"] {
            let value = env
                .get(name)
                .unwrap_or_else(|| panic!("{name} is always pinned"))
                .to_string_lossy()
                .into_owned();
            assert!(value.contains(&pin), "missing {pin:?} in {name}: {value}");
            assert!(
                value.contains("StrictHostKeyChecking=accept-new"),
                "missing pins in {name}: {value}"
            );
        }
    }

    #[test]
    fn caller_host_key_checking_override_is_dropped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.extravars = json!({
            "keep": true,
            "ansible_host_key_checking": false,
            "ansible_ssh_host_key_checking": false,
        });
        job.playbook_env
            .insert("ANSIBLE_HOST_KEY_CHECKING".to_string(), "False".to_string());
        job.playbook_env.insert(
            "ANSIBLE_SSH_HOST_KEY_CHECKING".to_string(),
            "False".to_string(),
        );
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let rendered = fs::read_to_string(pdd.env.join("extravars")).expect("extravars file");
        let extravars: Value = serde_json::from_str(&rendered).expect("extravars JSON");
        assert_eq!(extravars.get("keep"), Some(&Value::Bool(true)));
        for key in ["ansible_host_key_checking", "ansible_ssh_host_key_checking"] {
            assert!(
                extravars.get(key).is_none(),
                "{key} must be dropped from extravars: {rendered}"
            );
        }
        assert!(
            !rendered.contains("host_key_checking"),
            "a fail-open host-key toggle must not survive in extravars: {rendered}"
        );

        let env = build_playbook_envvars(&job, &settings, &pdd, None, None);
        for name in ["ANSIBLE_HOST_KEY_CHECKING", "ANSIBLE_SSH_HOST_KEY_CHECKING"] {
            assert!(
                !env.contains_key(name),
                "{name} must not reach the playbook environment: {env:?}"
            );
        }
    }

    #[test]
    fn caller_scp_and_sftp_extra_args_are_neutralised() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        let fail_open = "-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null";
        job.extravars = json!({
            "keep": true,
            "ansible_scp_extra_args": fail_open,
            "ansible_sftp_extra_args": fail_open,
        });
        job.playbook_env
            .insert("ANSIBLE_SCP_EXTRA_ARGS".to_string(), fail_open.to_string());
        job.playbook_env
            .insert("ANSIBLE_SFTP_EXTRA_ARGS".to_string(), fail_open.to_string());
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let rendered = fs::read_to_string(pdd.env.join("extravars")).expect("extravars file");
        let extravars: Value = serde_json::from_str(&rendered).expect("extravars JSON");
        assert_eq!(extravars.get("keep"), Some(&Value::Bool(true)));
        for key in ["ansible_scp_extra_args", "ansible_sftp_extra_args"] {
            assert!(
                extravars.get(key).is_none(),
                "{key} must be dropped from extravars: {rendered}"
            );
        }
        assert!(
            !rendered.contains("StrictHostKeyChecking=no"),
            "fail-open checking must not survive in extravars: {rendered}"
        );

        let env = build_playbook_envvars(&job, &settings, &pdd, None, None);
        for name in ["ANSIBLE_SCP_EXTRA_ARGS", "ANSIBLE_SFTP_EXTRA_ARGS"] {
            assert!(
                !env.contains_key(name),
                "{name} must not reach the playbook environment: {env:?}"
            );
        }
    }

    #[test]
    fn sanitize_caller_ssh_args_strips_only_host_key_options() {
        assert_eq!(
            sanitize_caller_ssh_args(
                "-o BatchMode=yes -o StrictHostKeyChecking=no -o ConnectTimeout=10 UserKnownHostsFile=/dev/null"
            ),
            "-o BatchMode=yes -o ConnectTimeout=10"
        );
        // The attached `-oKey=value` spelling is stripped too.
        assert_eq!(
            sanitize_caller_ssh_args("-oGlobalKnownHostsFile=/dev/null -oHashKnownHosts=yes"),
            ""
        );
        // OpenSSH option names are case-insensitive.
        assert_eq!(
            sanitize_caller_ssh_args("-o stricthostkeychecking=no -o userknownhostsfile=/dev/null"),
            ""
        );
        // A fully quoted host-key assignment is stripped.
        assert_eq!(sanitize_caller_ssh_args("\"StrictHostKeyChecking=no\""), "");
        // A quoted non-host-key value survives verbatim, including inner spaces.
        assert_eq!(
            sanitize_caller_ssh_args("-o ProxyCommand=\"ssh -W %h:%p jump\""),
            "-o ProxyCommand=\"ssh -W %h:%p jump\""
        );
    }

    #[test]
    fn completion_batch_reports_new_host_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.targets[0].known_hosts_lines = vec!["host1 ssh-ed25519 AAAASEED".to_string()];
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        // A seeded line is not a newly learned key.
        assert!(new_host_keys_for_job(&config, &job).is_empty());

        // ssh TOFU-appends the observed key to the same job file.
        let mut observed = fs::read_to_string(&pdd.known_hosts).expect("job known_hosts");
        observed.push_str("host2 ssh-ed25519 AAAANEW\n");
        fs::write(&pdd.known_hosts, observed).expect("append observed host key");

        assert_eq!(
            new_host_keys_for_job(&config, &job),
            vec!["host2 ssh-ed25519 AAAANEW".to_string()]
        );
    }

    #[tokio::test]
    async fn runner_status_mappings_are_exhaustive_and_client_visible() {
        let succeeded = terminal_outcome(ProcessTerminalStatus::Exited(Some(0)));
        assert_eq!(succeeded.0, RunnerJobState::Succeeded);
        assert_eq!(succeeded.1, RunnerRunStatus::Succeeded);
        assert_eq!(succeeded.2, Some(0));
        assert_eq!(succeeded.3, None);

        let failed = terminal_outcome(ProcessTerminalStatus::Exited(Some(7)));
        assert_eq!(failed.0, RunnerJobState::Failed);
        assert_eq!(failed.1, RunnerRunStatus::Failed);
        assert_eq!(failed.2, Some(7));
        assert_eq!(
            failed.3.as_deref(),
            Some("ansible-playbook status=failed rc=7")
        );

        let missing_status = terminal_outcome(ProcessTerminalStatus::Exited(None));
        assert_eq!(missing_status.0, RunnerJobState::Failed);
        assert_eq!(missing_status.2, None);
        assert_eq!(
            missing_status.3.as_deref(),
            Some("ansible-playbook status=failed")
        );

        for (terminal, job_state, run_status, message) in [
            (
                ProcessTerminalStatus::Canceled,
                RunnerJobState::Canceled,
                RunnerRunStatus::Canceled,
                "Run was canceled by operator",
            ),
            (
                ProcessTerminalStatus::TimedOut,
                RunnerJobState::TimedOut,
                RunnerRunStatus::TimedOut,
                "Run exceeded job_timeout and was reaped by the executor",
            ),
            (
                ProcessTerminalStatus::LaunchFailed("spawn denied".to_string()),
                RunnerJobState::Failed,
                RunnerRunStatus::Failed,
                "spawn denied",
            ),
        ] {
            let outcome = terminal_outcome(terminal);
            assert_eq!(outcome.0, job_state);
            assert_eq!(outcome.1, run_status);
            assert_eq!(outcome.2, None);
            assert_eq!(outcome.3.as_deref(), Some(message));
        }

        let errors = [
            (
                RunnerError::BadRequest("bad".to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                RunnerError::Conflict("conflict".to_string()),
                StatusCode::CONFLICT,
            ),
            (
                RunnerError::Unauthorized("unauthorized".to_string()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                RunnerError::NotFound("missing".to_string()),
                StatusCode::NOT_FOUND,
            ),
            (
                RunnerError::Internal("broken".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (error, expected_status) in errors {
            let expected_detail = error.to_string();
            let response = error.into_response();
            assert_eq!(response.status(), expected_status);
            let body = axum::body::to_bytes(response.into_body(), 1_024)
                .await
                .expect("error response body");
            let json: Value = serde_json::from_slice(&body).expect("JSON error envelope");
            assert_eq!(json["detail"], expected_detail);
        }

        let failed_recap = RecapStats {
            failed: 1,
            ..RecapStats::default()
        };
        let failed_completion = completion_from_observation(
            None,
            Some(&failed_recap),
            RunnerRunStatus::Succeeded,
            None,
            Some("host failed"),
        );
        assert_eq!(failed_completion.0, RunnerRunStatus::Failed);
        assert_eq!(failed_completion.1, Some(2));
        assert_eq!(failed_completion.2.as_deref(), Some("host failed"));

        let observed = ObservedOutcome {
            status: RunnerRunStatus::TimedOut,
            exit_code: 124,
            error_message: Some("host timed out".to_string()),
        };
        let observed_completion = completion_from_observation(
            Some(&observed),
            None,
            RunnerRunStatus::Succeeded,
            Some(0),
            None,
        );
        assert_eq!(observed_completion.0, RunnerRunStatus::TimedOut);
        assert_eq!(observed_completion.1, Some(124));
        assert_eq!(observed_completion.2.as_deref(), Some("host timed out"));
    }

    #[test]
    fn settings_rejects_removed_python_runner_backend() {
        let error = RunnerSettings::parse(&json!({"execution_backend": "ansible_runner"}))
            .expect_err("ansible_runner backend must be rejected");

        assert!(error.to_string().contains("native ansible-playbook only"));
    }

    #[test]
    fn settings_enable_raw_stdout_passthrough_by_default() {
        let default_settings = RunnerSettings::parse(&json!({})).expect("settings");
        let compact_settings =
            RunnerSettings::parse(&json!({"raw_stdout_passthrough": false})).expect("settings");

        assert!(default_settings.raw_stdout_passthrough);
        assert!(!compact_settings.raw_stdout_passthrough);
    }

    #[test]
    fn validates_contract_version() {
        let mut job = sample_job(json!({}));
        job.protocol_version = RUNNER_CONTRACT_VERSION + 1;

        let error = validate_job(&job).expect_err("unknown contract must be rejected");

        assert!(error.to_string().contains("unsupported protocol_version"));
    }

    #[test]
    fn validation_rejects_job_bundle_and_path_boundaries() {
        let valid = sample_job(json!({}));
        validate_job(&valid).expect("baseline job is valid");

        let invalid_jobs = [
            {
                let mut job = valid.clone();
                job.dispatch_attempt = 0;
                (job, "dispatch_attempt")
            },
            {
                let mut job = valid.clone();
                job.playbook.clear();
                (job, "playbook must not be empty")
            },
            {
                let mut job = valid.clone();
                job.playbook.clear();
                job.entrypoint = Some("site.yml".to_string());
                job.project_files = vec![RunnerProjectFile {
                    path: "other.yml".to_string(),
                    content: "---\n- hosts: all\n".to_string(),
                    executable: false,
                }];
                (job, "is not present")
            },
            {
                let mut job = valid.clone();
                job.inventory = "  ".to_string();
                (job, "inventory")
            },
            {
                let mut job = valid.clone();
                job.targets.clear();
                (job, "targets")
            },
            {
                let mut job = valid.clone();
                job.callback_base_url = " ".to_string();
                (job, "callback_base_url must not be empty")
            },
            {
                let mut job = valid.clone();
                job.callback_base_url = "://not-a-url".to_string();
                (job, "not a valid URL")
            },
            {
                let mut job = valid.clone();
                job.group_callback_token = "\t".to_string();
                (job, "group_callback_token")
            },
        ];
        for (job, message) in invalid_jobs {
            let error = validate_job(&job).expect_err("invalid job rejected");
            assert!(
                error.to_string().contains(message),
                "expected {message:?} in {error}"
            );
        }

        assert_eq!(
            validated_project_path("./roles/./demo/tasks/main.yml", "project file")
                .expect("current-directory components normalize"),
            PathBuf::from("roles/demo/tasks/main.yml")
        );
        for invalid in [
            "",
            ".",
            "/absolute.yml",
            "../escape.yml",
            "a\\b.yml",
            "a\0b",
        ] {
            assert!(
                validated_project_path(invalid, "project file").is_err(),
                "unsafe path {invalid:?} must be rejected"
            );
        }
        assert!(validated_key_alias("nested/host").is_err());

        let duplicate = [
            RunnerProjectFile {
                path: "site.yml".to_string(),
                content: "a".to_string(),
                executable: false,
            },
            RunnerProjectFile {
                path: "./site.yml".to_string(),
                content: "b".to_string(),
                executable: false,
            },
        ];
        assert!(
            validate_project_bundle(&duplicate)
                .expect_err("normalized duplicate rejected")
                .to_string()
                .contains("duplicate")
        );
        let oversized_file = [RunnerProjectFile {
            path: "large.bin".to_string(),
            content: "x".repeat(MAX_PROJECT_FILE_BYTES + 1),
            executable: false,
        }];
        assert!(
            validate_project_bundle(&oversized_file)
                .expect_err("oversized file rejected")
                .to_string()
                .contains("too large")
        );
        let too_many_files = (0..=MAX_PROJECT_FILES)
            .map(|index| RunnerProjectFile {
                path: format!("files/{index}"),
                content: String::new(),
                executable: false,
            })
            .collect::<Vec<_>>();
        assert!(
            validate_project_bundle(&too_many_files)
                .expect_err("file-count limit enforced")
                .to_string()
                .contains("too many files")
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let settings = RunnerSettings::parse(&valid.settings).expect("settings");
        let (pdd, ssh_agent) =
            prepare_job_execution(&config, &valid, &settings).expect("prepare without SSH key");
        assert!(ssh_agent.is_none());
        assert!(pdd.project.join(&pdd.entrypoint).is_file());
    }

    #[test]
    fn materializes_native_ansible_private_data_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let job = sample_job(json!({"forks": 32, "strategy": "free"}));
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");

        let playbook = fs::read_to_string(pdd.project.join("play.yml")).expect("playbook");
        let inventory = fs::read_to_string(pdd.inventory.join("hosts")).expect("inventory");
        let ansible_cfg = fs::read_to_string(pdd.project.join("ansible.cfg")).expect("ansible cfg");
        assert!(playbook.contains("hosts: all"));
        assert!(inventory.contains("host1"));
        assert!(ansible_cfg.contains("strategy = free"));
    }

    #[tokio::test]
    async fn persisted_dispatches_recover_missing_processes_without_replaying_terminal_jobs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let accepted_id = Uuid::from_u128(0x101);
        let succeeded_id = Uuid::from_u128(0x202);
        let accepted = PersistedDispatch {
            receipt: RunnerDispatchReceipt {
                dispatch_id: accepted_id,
                run_group_id: Uuid::from_u128(0x111),
                dispatch_attempt: 1,
            },
            status: RunnerJobState::Accepted,
            rc: None,
            process_id: Some(u32::MAX),
            terminal_transition_at_unix_s: None,
        };
        let succeeded = PersistedDispatch {
            receipt: RunnerDispatchReceipt {
                dispatch_id: succeeded_id,
                run_group_id: Uuid::from_u128(0x222),
                dispatch_attempt: 2,
            },
            status: RunnerJobState::Succeeded,
            rc: Some(0),
            process_id: None,
            terminal_transition_at_unix_s: Some(current_unix_time_seconds().expect("current time")),
        };

        persist_dispatch(&config, &accepted).expect("persist accepted receipt");
        persist_dispatch(&config, &succeeded).expect("persist terminal receipt");
        fs::write(dispatch_receipt_dir(&config).join("orphan.tmp"), b"partial")
            .expect("seed non-JSON crash residue");

        let jobs = load_persisted_dispatches(&config).expect("load durable receipts");
        assert_eq!(jobs.len(), 2, "non-JSON residue must be ignored");
        let recovered = jobs.get(&accepted_id).expect("accepted dispatch loaded");
        assert_eq!(*recovered.status.read().await, RunnerJobState::Failed);
        assert_eq!(*recovered.process_id.read().await, None);
        let terminal = jobs.get(&succeeded_id).expect("terminal dispatch loaded");
        assert_eq!(*terminal.status.read().await, RunnerJobState::Succeeded);
        assert_eq!(*terminal.rc.read().await, Some(0));

        let on_disk: PersistedDispatch = serde_json::from_slice(
            &fs::read(dispatch_receipt_path(&config, accepted_id)).expect("read recovered receipt"),
        )
        .expect("recovered receipt remains valid JSON");
        assert_eq!(on_disk.status, RunnerJobState::Failed);
        assert_eq!(on_disk.process_id, None);
        assert!(on_disk.terminal_transition_at_unix_s.is_some());
        assert!(!process_is_alive(u32::MAX));

        let receipt_mode = fs::metadata(dispatch_receipt_path(&config, accepted_id))
            .expect("receipt metadata")
            .permissions()
            .mode()
            & 0o777;
        let directory_mode = fs::metadata(dispatch_receipt_dir(&config))
            .expect("receipt directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(receipt_mode, 0o600);
        assert_eq!(directory_mode, 0o700);

        let mut cleanup_config = config.clone();
        cleanup_config.keep_data_dir = false;
        let run_group_id = Uuid::from_u128(0x333);
        let job_dir = cleanup_config.data_dir.join(run_group_id.to_string());
        fs::create_dir_all(&job_dir).expect("seed early-failure directory");
        remove_job_data_dir(&cleanup_config, run_group_id);
        assert!(!job_dir.exists());
        remove_job_data_dir(&cleanup_config, Uuid::from_u128(0x334));
    }

    #[tokio::test]
    async fn startup_prunes_only_terminal_receipts_older_than_retention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().to_path_buf());
        config.dispatch_receipt_retention = Duration::from_secs(60);
        let old_time = SystemTime::now()
            .checked_sub(Duration::from_secs(120))
            .expect("old receipt time");
        let old_unix_time = old_time
            .duration_since(UNIX_EPOCH)
            .expect("old receipt time is after the Unix epoch")
            .as_secs();

        let terminal_id = Uuid::from_u128(0x701);
        let legacy_terminal_id = Uuid::from_u128(0x702);
        let active_id = Uuid::from_u128(0x703);
        for (dispatch_id, status, process_id, terminal_transition_at_unix_s) in [
            (
                terminal_id,
                RunnerJobState::Succeeded,
                None,
                Some(old_unix_time),
            ),
            (
                legacy_terminal_id,
                RunnerJobState::Failed,
                None,
                Some(old_unix_time),
            ),
            (
                active_id,
                RunnerJobState::Running,
                Some(std::process::id()),
                None,
            ),
        ] {
            persist_dispatch(
                &config,
                &PersistedDispatch {
                    receipt: RunnerDispatchReceipt {
                        dispatch_id,
                        run_group_id: Uuid::from_u128(dispatch_id.as_u128() + 0x100),
                        dispatch_attempt: 1,
                    },
                    status,
                    rc: None,
                    process_id,
                    terminal_transition_at_unix_s,
                },
            )
            .expect("persist retention fixture");
        }

        let legacy_terminal_path = dispatch_receipt_path(&config, legacy_terminal_id);
        let mut legacy_json: Value = serde_json::from_slice(
            &fs::read(&legacy_terminal_path).expect("read legacy receipt fixture"),
        )
        .expect("parse legacy receipt fixture");
        assert!(
            legacy_json
                .as_object_mut()
                .expect("persisted receipt is an object")
                .remove("terminal_transition_at_unix_s")
                .is_some(),
            "legacy fixture must omit the new field"
        );
        fs::write(
            &legacy_terminal_path,
            serde_json::to_vec(&legacy_json).expect("serialize legacy receipt fixture"),
        )
        .expect("write legacy receipt fixture");

        for path in [
            legacy_terminal_path.clone(),
            dispatch_receipt_path(&config, active_id),
        ] {
            fs::File::open(path)
                .expect("open old receipt fixture")
                .set_times(
                    fs::FileTimes::new()
                        .set_accessed(old_time)
                        .set_modified(old_time),
                )
                .expect("age receipt fixture");
        }

        let jobs = load_persisted_dispatches(&config).expect("load retained receipts");

        assert!(
            !dispatch_receipt_path(&config, terminal_id).exists(),
            "new-format expired terminal receipt must be pruned"
        );
        assert!(
            !legacy_terminal_path.exists(),
            "legacy expired terminal receipt must use its mtime and be pruned"
        );
        assert!(
            dispatch_receipt_path(&config, active_id).exists(),
            "equally old nonterminal receipt must never be age-pruned"
        );
        let active = jobs
            .get(&active_id)
            .expect("active dispatch remains loaded");
        assert_eq!(*active.status.read().await, RunnerJobState::Running);
    }

    #[test]
    fn materializes_project_bundle_and_uses_entrypoint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.playbook.clear();
        job.entrypoint = Some("site.yml".to_string());
        job.project_files = vec![
            RunnerProjectFile {
                path: "site.yml".to_string(),
                content: "---\n- hosts: all\n  roles:\n    - demo\n".to_string(),
                executable: false,
            },
            RunnerProjectFile {
                path: "roles/demo/tasks/main.yml".to_string(),
                content: "---\n- debug:\n    msg: bundled role\n".to_string(),
                executable: false,
            },
            RunnerProjectFile {
                path: "scripts/check.sh".to_string(),
                content: "#!/bin/sh\nexit 0\n".to_string(),
                executable: true,
            },
        ];
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let command = build_ansible_command(&job, &settings, &pdd);

        assert!(pdd.project.join("site.yml").is_file());
        assert!(pdd.project.join("roles/demo/tasks/main.yml").is_file());
        assert!(!pdd.project.join("play.yml").exists());
        let expected_entrypoint = pdd.project.join("site.yml");
        let args = command.as_std().get_args().collect::<Vec<_>>();
        assert!(
            args.iter()
                .any(|arg| *arg == expected_entrypoint.as_os_str()),
            "command args must include project entrypoint; got {args:?}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(pdd.project.join("scripts/check.sh"))
                .expect("script metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn rejects_project_bundle_path_traversal() {
        let mut job = sample_job(json!({}));
        job.playbook.clear();
        job.entrypoint = Some("../site.yml".to_string());
        job.project_files = vec![RunnerProjectFile {
            path: "../site.yml".to_string(),
            content: "---\n- hosts: all\n".to_string(),
            executable: false,
        }];

        let error = validate_job(&job).expect_err("traversal must be rejected");

        assert!(error.to_string().contains("traversal"));
    }

    #[test]
    fn materializes_per_host_key_files_with_private_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = test_config(tmp.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.targets[0].ssh_private_key = Some("-----BEGIN KEY web-01-----\n".to_string());
        job.targets.push(RunnerJobTarget {
            alias: "db-02".to_string(),
            run_id: Uuid::from_u128(9),
            job_token: "t2".to_string(),
            target_kind: "connection_target".to_string(),
            target_id: Uuid::from_u128(10),
            source_kind: "device".to_string(),
            source_id: None,
            ssh_private_key: Some("-----BEGIN KEY db-02-----\n".to_string()),
            known_hosts_lines: Vec::new(),
        });
        let settings = RunnerSettings::parse(&job.settings).expect("settings");

        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");

        let key_a = pdd.project.join(RUNNER_PER_HOST_KEY_DIR).join("host1");
        let key_b = pdd.project.join(RUNNER_PER_HOST_KEY_DIR).join("db-02");
        assert_eq!(
            fs::read_to_string(&key_a).expect("host1 key"),
            "-----BEGIN KEY web-01-----\n"
        );
        assert_eq!(
            fs::read_to_string(&key_b).expect("db-02 key"),
            "-----BEGIN KEY db-02-----\n"
        );
        for key in [&key_a, &key_b] {
            let mode = fs::metadata(key).expect("key meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "per-host key must be 0600");
        }
    }

    #[test]
    fn rejects_project_bundle_reserved_key_dir() {
        let mut job = sample_job(json!({}));
        job.entrypoint = Some("site.yml".to_string());
        job.project_files = vec![
            RunnerProjectFile {
                path: "site.yml".to_string(),
                content: "---\n- hosts: all\n".to_string(),
                executable: false,
            },
            RunnerProjectFile {
                path: format!("{RUNNER_PER_HOST_KEY_DIR}/host1"),
                content: "malicious".to_string(),
                executable: false,
            },
        ];

        let error = validate_job(&job).expect_err("reserved key dir must be rejected");
        assert!(error.to_string().contains(RUNNER_PER_HOST_KEY_DIR));
    }

    #[test]
    fn rejects_project_bundle_ansible_cfg_override() {
        let mut job = sample_job(json!({}));
        job.entrypoint = Some("site.yml".to_string());
        job.project_files = vec![
            RunnerProjectFile {
                path: "site.yml".to_string(),
                content: "---\n- hosts: all\n".to_string(),
                executable: false,
            },
            RunnerProjectFile {
                path: "ansible.cfg".to_string(),
                content: "[defaults]\nstdout_callback = debug\n".to_string(),
                executable: false,
            },
        ];

        let error = validate_job(&job).expect_err("ansible.cfg override must be rejected");

        assert!(error.to_string().contains("ansible.cfg"));
    }

    #[test]
    fn rejects_project_bundle_total_size_over_budget() {
        let mut files = Vec::new();
        for index in 0..=32 {
            files.push(RunnerProjectFile {
                path: format!("files/{index}.txt"),
                content: "x".repeat(MAX_PROJECT_FILE_BYTES),
                executable: false,
            });
        }

        let error = validate_project_bundle(&files).expect_err("oversized bundle rejected");

        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn result_parser_attributes_warnings_only_when_host_is_explicit() {
        let settings = RunnerSettings::parse(&json!({"compact_events": true})).expect("settings");
        let config = test_config(PathBuf::from("/tmp"));
        let job = sample_job(json!({}));
        let mut state = ParseState::default();

        let warning = map_output_line(
            &job,
            &settings,
            &config,
            &mut state,
            OutputStream::Stdout,
            "[WARNING]: Host 'host1' is using the discovered Python interpreter",
        );
        assert!(
            warning.is_none(),
            "interpreter warnings are suppressed in compact mode"
        );

        let event = map_output_line(
            &job,
            &settings,
            &config,
            &mut state,
            OutputStream::Stdout,
            "ok: [host1 -> localhost] => {\"changed\": false}",
        )
        .expect("event");
        assert_eq!(event.host.as_deref(), Some("host1"));
    }

    #[test]
    fn map_output_line_strips_ansi_before_parsing_so_colour_never_breaks_it() {
        let settings = RunnerSettings::parse(&json!({})).expect("settings");
        let config = test_config(PathBuf::from("/tmp"));
        let job = sample_job(json!({}));
        let mut state = ParseState::default();

        // A green-coloured `ok:` line still resolves the host: the parser sees the
        // ANSI-stripped copy while the raw passthrough keeps the colour for the UI.
        let ok = map_output_line(
            &job,
            &settings,
            &config,
            &mut state,
            OutputStream::Stdout,
            "\u{1b}[0;32mok: [host1 -> localhost] => {\"changed\": false}\u{1b}[0m",
        )
        .expect("coloured ok line parses");
        assert_eq!(ok.host.as_deref(), Some("host1"));

        // A coloured PLAY RECAP line still populates recap stats (protects the
        // per-target rail projection).
        let recap = map_output_line(
            &job,
            &settings,
            &config,
            &mut state,
            OutputStream::Stdout,
            "\u{1b}[0;32mhost1\u{1b}[0m                    : ok=3    changed=1    unreachable=0    failed=0    skipped=0    rescued=0    ignored=0",
        );
        assert!(recap.is_some(), "coloured recap line is mapped to an event");
        let recap_stats = state
            .recap
            .get("host1")
            .expect("recap stats recorded for host1");
        assert_eq!(recap_stats.ok, 3);
        assert_eq!(recap_stats.changed, 1);
    }

    #[test]
    fn completion_prefers_recap_per_target_over_global_failure() {
        let job = sample_job(json!({}));
        let mut parse_state = ParseState::default();
        parse_state.recap.insert(
            "host1".to_string(),
            RecapStats {
                ok: 3,
                ..RecapStats::default()
            },
        );

        let items = completion_items(
            &job,
            &parse_state,
            RunnerRunStatus::Failed,
            Some(2),
            Some("global failed"),
        );

        assert_eq!(items[0].status, RunnerRunStatus::Succeeded);
        assert_eq!(items[0].exit_code, Some(0));
        assert_eq!(items[0].error_message, None);
    }

    #[test]
    fn auth_accepts_bearer_only() {
        let mut bearer = HeaderMap::new();
        bearer.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer secret".parse().expect("header"),
        );
        assert!(require_auth(&bearer, "secret").is_ok());

        // The legacy plaintext `x-runner-secret` header is no longer an accepted
        // auth path — the backend only ever sends a Bearer token.
        let mut legacy = HeaderMap::new();
        legacy.insert("x-runner-secret", "secret".parse().expect("header"));
        assert!(matches!(
            require_auth(&legacy, "secret"),
            Err(RunnerError::Unauthorized(_))
        ));

        let mut bad = HeaderMap::new();
        bad.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer wrong".parse().expect("header"),
        );
        assert!(matches!(
            require_auth(&bad, "secret"),
            Err(RunnerError::Unauthorized(_))
        ));
    }

    #[tokio::test]
    async fn dispatch_handlers_are_idempotent_concurrent_and_durable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let job = sample_job(json!({}));
        let receipt = RunnerDispatchReceipt {
            dispatch_id: job.dispatch_id,
            run_group_id: job.run_group_id,
            dispatch_attempt: job.dispatch_attempt,
        };
        let active = job_control(receipt.clone(), RunnerJobState::Accepted, None, None);

        let stale_id = Uuid::from_u128(0x303);
        let stale_receipt = RunnerDispatchReceipt {
            dispatch_id: stale_id,
            run_group_id: Uuid::from_u128(0x313),
            dispatch_attempt: 1,
        };
        let stale = job_control(stale_receipt, RunnerJobState::Running, None, Some(u32::MAX));

        let terminal_id = Uuid::from_u128(0x404);
        let terminal_receipt = RunnerDispatchReceipt {
            dispatch_id: terminal_id,
            run_group_id: Uuid::from_u128(0x414),
            dispatch_attempt: 1,
        };
        let terminal = job_control(terminal_receipt, RunnerJobState::Succeeded, Some(0), None);
        let jobs = HashMap::from([
            (job.dispatch_id, active.clone()),
            (stale_id, stale.clone()),
            (terminal_id, terminal.clone()),
        ]);
        let state = Arc::new(AppState {
            config,
            jobs: Mutex::new(jobs),
        });

        let (status, accepted) = create_job(
            State(Arc::clone(&state)),
            authorized_headers(),
            Json(job.clone()),
        )
        .await
        .expect("same dispatch identity is an idempotent retry");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(accepted.0, receipt);

        let mut rebound = job.clone();
        rebound.run_group_id = Uuid::from_u128(0x505);
        assert!(matches!(
            create_job(
                State(Arc::clone(&state)),
                authorized_headers(),
                Json(rebound),
            )
            .await,
            Err(RunnerError::Conflict(_))
        ));

        let mut competing = job.clone();
        competing.dispatch_id = Uuid::from_u128(0x606);
        assert!(matches!(
            create_job(
                State(Arc::clone(&state)),
                authorized_headers(),
                Json(competing),
            )
            .await,
            Err(RunnerError::Conflict(_))
        ));

        let first_cancel = cancel_dispatch(
            State(Arc::clone(&state)),
            authorized_headers(),
            AxumPath(job.dispatch_id),
        );
        let second_cancel = cancel_dispatch(
            State(Arc::clone(&state)),
            authorized_headers(),
            AxumPath(job.dispatch_id),
        );
        let (first, second) = tokio::join!(first_cancel, second_cancel);
        assert_eq!(
            first.expect("first cancel").0.status,
            RunnerJobState::Canceling
        );
        assert_eq!(
            second.expect("concurrent cancel").0.status,
            RunnerJobState::Canceling
        );
        assert!(active.cancel_requested.load(Ordering::SeqCst));
        let persisted: PersistedDispatch = serde_json::from_slice(
            &fs::read(dispatch_receipt_path(&state.config, job.dispatch_id))
                .expect("read canceled receipt"),
        )
        .expect("concurrent persistence leaves valid JSON");
        assert_eq!(persisted.status, RunnerJobState::Canceling);

        let reconciled = get_dispatch(
            State(Arc::clone(&state)),
            authorized_headers(),
            AxumPath(stale_id),
        )
        .await
        .expect("stale running process reconciles");
        assert_eq!(reconciled.0.status, RunnerJobState::Failed);
        assert_eq!(*stale.process_id.read().await, None);

        let terminal_response = cancel_dispatch(
            State(Arc::clone(&state)),
            authorized_headers(),
            AxumPath(terminal_id),
        )
        .await
        .expect("canceling a terminal dispatch is idempotent");
        assert_eq!(terminal_response.0.status, RunnerJobState::Succeeded);
        assert!(!terminal.cancel_requested.load(Ordering::SeqCst));

        assert!(matches!(
            get_dispatch(
                State(state),
                authorized_headers(),
                AxumPath(Uuid::from_u128(0x999)),
            )
            .await,
            Err(RunnerError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn canceled_jobs_win_over_invalid_input_and_callback_refusal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().to_path_buf());
        config.callback_timeout = Duration::from_millis(50);
        let mut job = sample_job(json!({}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        job.playbook.clear();
        job.entrypoint = Some("site.yml".to_string());
        job.project_files = vec![RunnerProjectFile {
            path: "other.yml".to_string(),
            content: "---\n- hosts: all\n".to_string(),
            executable: false,
        }];
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let state = Arc::new(AppState {
            config,
            jobs: Mutex::new(HashMap::new()),
        });
        let status = Arc::new(RwLock::new(RunnerJobState::Accepted));
        let control = JobControl {
            receipt: RunnerDispatchReceipt {
                dispatch_id: job.dispatch_id,
                run_group_id: job.run_group_id,
                dispatch_attempt: job.dispatch_attempt,
            },
            status: Arc::clone(&status),
            rc: Arc::new(RwLock::new(None)),
            cancel_requested: Arc::new(AtomicBool::new(true)),
            process_id: Arc::new(RwLock::new(None)),
            terminal_transition_at_unix_s: Arc::new(RwLock::new(None)),
        };

        execute_job(Arc::clone(&state), job.clone(), settings, control)
            .await
            .expect("pre-canceled job must not enter preparation");

        assert_eq!(*status.read().await, RunnerJobState::Canceled);
        assert!(
            !state
                .config
                .data_dir
                .join(job.run_group_id.to_string())
                .exists(),
            "invalid project input must remain untouched after pre-cancel"
        );
        let persisted: PersistedDispatch = serde_json::from_slice(
            &fs::read(dispatch_receipt_path(&state.config, job.dispatch_id))
                .expect("read canceled receipt"),
        )
        .expect("canceled receipt JSON");
        assert_eq!(persisted.status, RunnerJobState::Canceled);
        assert_eq!(persisted.process_id, None);
        assert!(persisted.terminal_transition_at_unix_s.is_some());
    }

    #[tokio::test]
    async fn execute_job_persists_success_and_cleans_up_when_callback_is_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().join("jobs"));
        config.callback_timeout = Duration::from_millis(50);
        config.event_drain_timeout = Duration::from_secs(2);
        config.keep_data_dir = false;
        let bin = write_fake_ansible_playbook(dir.path());
        let mut job = sample_job(json!({"compact_events": true}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        job.playbook_env
            .insert("PATH".to_string(), bin.display().to_string());
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let state = Arc::new(AppState {
            config,
            jobs: Mutex::new(HashMap::new()),
        });
        let status = Arc::new(RwLock::new(RunnerJobState::Accepted));
        let rc = Arc::new(RwLock::new(None));
        let process_id = Arc::new(RwLock::new(None));
        let control = JobControl {
            receipt: RunnerDispatchReceipt {
                dispatch_id: job.dispatch_id,
                run_group_id: job.run_group_id,
                dispatch_attempt: job.dispatch_attempt,
            },
            status: Arc::clone(&status),
            rc: Arc::clone(&rc),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            process_id: Arc::clone(&process_id),
            terminal_transition_at_unix_s: Arc::new(RwLock::new(None)),
        };

        timeout(
            Duration::from_secs(10),
            execute_job(Arc::clone(&state), job.clone(), settings, control),
        )
        .await
        .expect("callback refusal must not hang job completion")
        .expect("successful child process completes the job");

        assert_eq!(*status.read().await, RunnerJobState::Succeeded);
        assert_eq!(*rc.read().await, Some(0));
        assert!(process_id.read().await.is_none());
        assert!(
            !state
                .config
                .data_dir
                .join(job.run_group_id.to_string())
                .exists(),
            "successful job cleanup must remove private data"
        );
        let persisted: PersistedDispatch = serde_json::from_slice(
            &fs::read(dispatch_receipt_path(&state.config, job.dispatch_id))
                .expect("read successful receipt"),
        )
        .expect("successful receipt JSON");
        assert_eq!(persisted.status, RunnerJobState::Succeeded);
        assert_eq!(persisted.rc, Some(0));
        assert_eq!(persisted.process_id, None);
        assert!(persisted.terminal_transition_at_unix_s.is_some());
    }

    #[tokio::test]
    async fn ansible_process_honors_pre_cancel_and_preserves_real_child_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().join("jobs"));
        config.callback_timeout = Duration::from_millis(50);
        config.event_drain_timeout = Duration::from_secs(2);
        let bin = write_fake_ansible_playbook(dir.path());

        let mut job = sample_job(json!({"compact_events": true}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        job.playbook_env
            .insert("PATH".to_string(), bin.display().to_string());
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");

        let mut canceled_state = ParseState::default();
        let canceled_process_id = Arc::new(RwLock::new(None));
        let canceled = run_ansible_process(ProcessRunContext {
            job: &job,
            settings: &settings,
            config: &config,
            pdd: &pdd,
            ssh_agent: None,
            galaxy: None,
            dispatcher: &dispatcher,
            parse_state: &mut canceled_state,
            cancel_requested: Arc::new(AtomicBool::new(true)),
            process_id: Arc::clone(&canceled_process_id),
        })
        .await;
        assert!(matches!(canceled, ProcessTerminalStatus::Canceled));
        assert!(canceled_process_id.read().await.is_none());
        assert!(canceled_state.observed.is_empty());

        let mut parse_state = ParseState::default();
        let process_id = Arc::new(RwLock::new(None));
        let terminal = timeout(
            Duration::from_secs(5),
            run_ansible_process(ProcessRunContext {
                job: &job,
                settings: &settings,
                config: &config,
                pdd: &pdd,
                ssh_agent: None,
                galaxy: None,
                dispatcher: &dispatcher,
                parse_state: &mut parse_state,
                cancel_requested: Arc::new(AtomicBool::new(false)),
                process_id: Arc::clone(&process_id),
            }),
        )
        .await
        .expect("fake ansible process must terminate");
        assert!(matches!(terminal, ProcessTerminalStatus::Exited(Some(0))));
        assert!(process_id.read().await.is_some());
        assert_eq!(parse_state.current_task.as_deref(), Some("Probe"));
        let observed = parse_state.observed.get("host1").expect("host result");
        assert_eq!(observed.status, RunnerRunStatus::Succeeded);
        assert_eq!(observed.exit_code, 0);
        let recap = parse_state.recap.get("host1").expect("host recap");
        assert_eq!(recap.ok, 1);
        assert_eq!(recap.changed, 1);

        let mut missing_binary_job = job.clone();
        missing_binary_job.playbook_env.insert(
            "PATH".to_string(),
            dir.path().join("missing").display().to_string(),
        );
        let mut launch_state = ParseState::default();
        let launch_failure = run_ansible_process(ProcessRunContext {
            job: &missing_binary_job,
            settings: &settings,
            config: &config,
            pdd: &pdd,
            ssh_agent: None,
            galaxy: None,
            dispatcher: &dispatcher,
            parse_state: &mut launch_state,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            process_id: Arc::new(RwLock::new(None)),
        })
        .await;
        assert!(
            matches!(launch_failure, ProcessTerminalStatus::LaunchFailed(message) if message.contains("Failed to launch ansible-playbook"))
        );
        dispatcher.finish().await;
    }

    #[test]
    fn event_batch_callback_url_includes_run_group_id() {
        let base = Url::parse("http://backend:7000/callback-root?stale=true").expect("base url");
        let run_group_id = Uuid::from_u128(0x019f_432d_d772_7770_9bdf_83a2_d2c5_e244);

        let endpoint = group_event_batch_endpoint(&base, run_group_id);

        assert_eq!(
            endpoint.as_str(),
            format!("http://backend:7000/api/v1/playbooks/run-groups/{run_group_id}/events/batch")
        );
    }

    #[test]
    fn ssh_connect_timeout_label_reads_ssh_common_args() {
        let settings = RunnerSettings::parse(&json!({
            "execution_backend": "native",
            "ssh_common_args": "-o ConnectTimeout=8 -o ServerAliveInterval=8",
        }))
        .expect("settings");

        assert_eq!(ssh_connect_timeout_label(&settings), "8s");
    }

    #[test]
    fn host_events_are_enriched_with_target_run_id() {
        let job = sample_job(json!({}));
        let mut event = RunnerGroupEvent {
            run_id: None,
            host: Some("host1".to_string()),
            event_kind: RunnerEventKind::Stdout,
            event_type: "runner_on_ok".to_string(),
            message: Some("ok: [host1]".to_string()),
            payload: None,
        };

        enrich_event_run_id(&job, &mut event);

        assert_eq!(event.run_id, Some(Uuid::from_u128(2)));
    }

    #[test]
    fn playbook_process_env_is_allowlisted_and_never_leaks_runner_secrets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.playbook_env
            .insert("PIPING_API_TOKEN".to_string(), "pjt_example".to_string());
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let parent_env: Vec<(OsString, OsString)> = [
            ("PATH", "/usr/local/bin:/usr/bin"),
            ("HOME", "/home/runner"),
            ("LANG", "C.UTF-8"),
            ("LC_ALL", "C.UTF-8"),
            ("TMPDIR", "/home/runner/tmp"),
            ("ANSIBLE_HOME", "/home/runner/.ansible"),
            ("PYTHONDONTWRITEBYTECODE", "1"),
            (
                "PIPING_ANSIBLE_RUNNER__SHARED_SECRET",
                "control-plane-secret",
            ),
            ("PIPING_ANSIBLE_RUNNER__DATA_DIR", "/var/lib/piping-runner"),
            ("AWS_SECRET_ACCESS_KEY", "not-for-playbooks"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();

        let envs = build_process_env(parent_env, &job, &settings, &pdd, None, None);

        assert!(
            envs.keys()
                .all(|name| !name.starts_with("PIPING_ANSIBLE_RUNNER__")),
            "runner control-plane env must never reach playbooks: {:?}",
            envs.keys().collect::<Vec<_>>()
        );
        assert!(!envs.contains_key("AWS_SECRET_ACCESS_KEY"));
        for inherited in [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "TMPDIR",
            "ANSIBLE_HOME",
            "PYTHONDONTWRITEBYTECODE",
        ] {
            assert!(
                envs.contains_key(inherited),
                "expected {inherited} to be inherited by ansible-playbook"
            );
        }
        assert!(envs.contains_key("ANSIBLE_CONFIG"));
        assert_eq!(
            envs.get("PIPING_API_TOKEN"),
            Some(&OsString::from("pjt_example")),
            "explicit request playbook_env must pass through"
        );
    }

    #[tokio::test]
    async fn spawned_playbook_process_does_not_inherit_parent_environment() {
        assert!(
            env::var("CARGO_MANIFEST_DIR").is_ok(),
            "test requires cargo to set CARGO_MANIFEST_DIR in the parent env"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let job = sample_job(json!({}));
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printenv CARGO_MANIFEST_DIR || echo __ABSENT__");
        command.env_clear();
        command.envs(build_process_env(
            env::vars_os(),
            &job,
            &settings,
            &pdd,
            None,
            None,
        ));

        let output = command.output().await.expect("run probe shell");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("__ABSENT__"),
            "non-allowlisted parent env must not leak into the child: {stdout}"
        );
    }

    async fn capture_completion_batch(
        State(sender): State<mpsc::Sender<Value>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let _ = sender.send(body).await;
        Json(json!({"processed": true}))
    }

    async fn acknowledge_event_batch(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({"processed": true}))
    }

    async fn spawn_callback_capture_server() -> (String, mpsc::Receiver<Value>) {
        let (sender, receiver) = mpsc::channel(16);
        let router = Router::new()
            .route(
                "/api/v1/playbooks/run-groups/{run_group_id}/completion-batch",
                post(capture_completion_batch),
            )
            .route(
                "/api/v1/playbooks/run-groups/{run_group_id}/events/batch",
                post(acknowledge_event_batch),
            )
            .with_state(sender);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capture server");
        let address = listener.local_addr().expect("capture server address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (format!("http://{address}"), receiver)
    }

    #[tokio::test]
    async fn execute_job_reports_failed_completion_when_preparation_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base_url, mut completions) = spawn_callback_capture_server().await;
        let mut config = test_config(dir.path().to_path_buf());
        config.keep_data_dir = false;
        let mut job = sample_job(json!({}));
        job.run_group_id = Uuid::from_u128(77);
        job.callback_base_url = base_url;
        // Deterministic preparation failure: bundle without the entrypoint and
        // no inline playbook to fall back to.
        job.playbook.clear();
        job.entrypoint = Some("site.yml".to_string());
        job.project_files = vec![RunnerProjectFile {
            path: "other.yml".to_string(),
            content: "---\n- hosts: all\n".to_string(),
            executable: false,
        }];
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let state = Arc::new(AppState {
            config,
            jobs: Mutex::new(HashMap::new()),
        });
        let status = Arc::new(RwLock::new(RunnerJobState::Accepted));
        let control = JobControl {
            receipt: RunnerDispatchReceipt {
                dispatch_id: job.dispatch_id,
                run_group_id: job.run_group_id,
                dispatch_attempt: job.dispatch_attempt,
            },
            status: Arc::clone(&status),
            rc: Arc::new(RwLock::new(None)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            process_id: Arc::new(RwLock::new(None)),
            terminal_transition_at_unix_s: Arc::new(RwLock::new(None)),
        };

        let result = execute_job(Arc::clone(&state), job.clone(), settings, control).await;

        assert!(
            result.is_err(),
            "early preparation failure must still surface as an error"
        );
        let batch = timeout(Duration::from_secs(5), completions.recv())
            .await
            .expect("completion batch must arrive without waiting for a backend timeout")
            .expect("capture channel must stay open");
        let runs = batch["runs"].as_array().expect("runs array");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["run_id"], json!(job.targets[0].run_id));
        assert_eq!(runs[0]["status"], "failed");
        let message = runs[0]["error_message"].as_str().expect("error message");
        assert!(
            message.contains("Failed to prepare ansible job"),
            "completion must carry an operator-readable reason: {message}"
        );
        assert_eq!(*status.read().await, RunnerJobState::Failed);
        assert!(
            !state
                .config
                .data_dir
                .join(job.run_group_id.to_string())
                .exists(),
            "early failure must clean up the private data dir"
        );
    }

    #[tokio::test]
    async fn supervise_child_reaps_job_when_grandchild_holds_output_pipe() {
        let mut config = test_config(PathBuf::from("/unused"));
        config.output_drain_grace = Duration::from_millis(200);
        let mut job = sample_job(json!({}));
        // Unroutable callback target: dispatched events fail fast and are
        // irrelevant to the supervision behavior under test.
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");
        let mut parse_state = ParseState::default();
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("echo started; sleep 10 & exit 0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command.spawn().expect("spawn probe shell");

        let status = timeout(
            Duration::from_secs(5),
            supervise_child(
                child,
                ChildSupervisionContext {
                    job: &job,
                    settings: &settings,
                    config: &config,
                    dispatcher: &dispatcher,
                    parse_state: &mut parse_state,
                    cancel_requested: Arc::new(AtomicBool::new(false)),
                    process_id: Arc::new(RwLock::new(None)),
                },
            ),
        )
        .await
        .expect("supervision must not hang while a grandchild holds the output pipe");

        assert!(
            matches!(status, ProcessTerminalStatus::Exited(Some(0))),
            "exit status of the direct child must be preserved: {status:?}"
        );
        dispatcher.finish().await;
    }

    #[test]
    fn job_timeout_falls_back_to_runner_default_when_settings_omit_it() {
        let config = test_config(PathBuf::from("/unused"));
        let explicit = RunnerSettings::parse(&json!({"job_timeout": 120})).expect("settings");
        let omitted = RunnerSettings::parse(&json!({})).expect("settings");

        assert_eq!(
            effective_job_timeout(&explicit, &config),
            Duration::from_mins(2)
        );
        assert_eq!(
            effective_job_timeout(&omitted, &config),
            Duration::from_secs(DEFAULT_JOB_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn completion_marks_missing_target_failed_even_when_group_succeeded() {
        let job = sample_job(json!({}));
        let parse_state = ParseState::default();

        let items = completion_items(
            &job,
            &parse_state,
            RunnerRunStatus::Succeeded,
            Some(0),
            None,
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, RunnerRunStatus::Failed);
        assert_eq!(items[0].exit_code, None);
        assert_eq!(
            items[0].error_message.as_deref(),
            Some(MISSING_TARGET_OUTPUT_MESSAGE)
        );
    }

    #[test]
    fn completion_keeps_group_fallback_for_missing_target_when_group_did_not_succeed() {
        let job = sample_job(json!({}));
        let parse_state = ParseState::default();

        let items = completion_items(
            &job,
            &parse_state,
            RunnerRunStatus::Canceled,
            None,
            Some("Run was canceled by operator"),
        );

        assert_eq!(items[0].status, RunnerRunStatus::Canceled);
        assert_eq!(
            items[0].error_message.as_deref(),
            Some("Run was canceled by operator")
        );
    }

    #[test]
    fn inherited_env_allowlist_filters_by_exact_name_and_prefix() {
        let parent_env = [
            ("PATH", "/usr/bin"),
            ("ANSIBLE_SSH_CONTROL_PATH_DIR", "/home/runner/.ansible/cp"),
            ("LC_CTYPE", "C.UTF-8"),
            ("PYTHONPATH", "/opt/lib"),
            ("PIPING_ANSIBLE_RUNNER__SHARED_SECRET", "secret"),
            ("SSH_AUTH_SOCK", "/run/agent.sock"),
            ("RANDOM_VAR", "1"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));

        let envs = inherited_env_allowlist(parent_env);

        for kept in [
            "PATH",
            "ANSIBLE_SSH_CONTROL_PATH_DIR",
            "LC_CTYPE",
            "PYTHONPATH",
        ] {
            assert!(envs.contains_key(kept), "{kept} must be inherited");
        }
        // The runner's own SSH_AUTH_SOCK (if any) must not leak; the per-job
        // agent socket is injected explicitly via build_playbook_envvars.
        for dropped in [
            "PIPING_ANSIBLE_RUNNER__SHARED_SECRET",
            "SSH_AUTH_SOCK",
            "RANDOM_VAR",
        ] {
            assert!(!envs.contains_key(dropped), "{dropped} must be dropped");
        }
    }

    fn write_requirements(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir requirements dir");
        fs::write(path, body).expect("write requirements");
    }

    #[test]
    fn galaxy_plan_is_empty_when_no_requirements_declared() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A bundle with unrelated files must not trigger any galaxy install:
        // the common case stays network-free and adds no latency.
        fs::write(dir.path().join("play.yml"), "---\n- hosts: all\n").expect("write play");
        assert!(plan_galaxy_installs(dir.path()).is_empty());
    }

    #[test]
    fn galaxy_plan_maps_each_requirements_file_to_its_subcommand() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_requirements(
            dir.path(),
            "requirements.yml",
            "collections: []\nroles: []\n",
        );
        write_requirements(
            dir.path(),
            "collections/requirements.yml",
            "collections: []\n",
        );
        write_requirements(dir.path(), "roles/requirements.yml", "roles: []\n");

        let plan = plan_galaxy_installs(dir.path());

        // Root file feeds both sub-commands; each nested file feeds only its own.
        assert_eq!(
            plan,
            vec![
                GalaxyInstall {
                    kind: GalaxyReqKind::Collection,
                    requirements: PathBuf::from("requirements.yml"),
                },
                GalaxyInstall {
                    kind: GalaxyReqKind::Role,
                    requirements: PathBuf::from("requirements.yml"),
                },
                GalaxyInstall {
                    kind: GalaxyReqKind::Collection,
                    requirements: PathBuf::from("collections/requirements.yml"),
                },
                GalaxyInstall {
                    kind: GalaxyReqKind::Role,
                    requirements: PathBuf::from("roles/requirements.yml"),
                },
            ]
        );
    }

    #[test]
    fn galaxy_root_bare_list_is_roles_only() {
        // Classic pre-2.10 role file: a bare top-level sequence. Running
        // `collection install` on it errors, so it must map to roles only.
        let kinds = detect_root_galaxy_kinds("---\n- src: geerlingguy.docker\n- src: foo\n");
        assert_eq!(kinds, vec![GalaxyReqKind::Role]);
    }

    #[test]
    fn galaxy_root_mapping_classifies_only_present_sections() {
        assert_eq!(
            detect_root_galaxy_kinds("collections:\n  - community.general\n"),
            vec![GalaxyReqKind::Collection]
        );
        assert_eq!(
            detect_root_galaxy_kinds("roles:\n  - name: foo\n"),
            vec![GalaxyReqKind::Role]
        );
        assert_eq!(
            detect_root_galaxy_kinds("collections:\n  - a\nroles:\n  - b\n"),
            vec![GalaxyReqKind::Collection, GalaxyReqKind::Role]
        );
        // A nested `roles:` key under `collections:` must not be misread as a
        // top-level roles section.
        assert_eq!(
            detect_root_galaxy_kinds("collections:\n  - name: a\n    roles: nope\n"),
            vec![GalaxyReqKind::Collection]
        );
        // Malformed/empty mapping declares nothing installable.
        assert!(detect_root_galaxy_kinds("# just a comment\n").is_empty());
    }

    #[test]
    fn galaxy_plan_bare_list_root_skips_collection_install() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_requirements(
            dir.path(),
            "requirements.yml",
            "- src: geerlingguy.docker\n",
        );

        let plan = plan_galaxy_installs(dir.path());

        assert_eq!(
            plan,
            vec![GalaxyInstall {
                kind: GalaxyReqKind::Role,
                requirements: PathBuf::from("requirements.yml"),
            }],
            "a classic bare-list root must not trigger a failing collection install: {plan:?}"
        );
    }

    #[test]
    fn galaxy_plan_prefers_yml_over_yaml_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_requirements(dir.path(), "requirements.yml", "collections: []\n");
        write_requirements(dir.path(), "requirements.yaml", "collections: []\n");

        let plan = plan_galaxy_installs(dir.path());

        assert!(
            plan.iter()
                .all(|install| install.requirements.as_path() == Path::new("requirements.yml")),
            "requirements.yml must win over requirements.yaml: {plan:?}"
        );
    }

    #[test]
    fn galaxy_install_argv_uses_explicit_paths_per_kind() {
        let collections = PathBuf::from("/cache/abc/collections");
        let roles = PathBuf::from("/cache/abc/roles");
        let collection_install = GalaxyInstall {
            kind: GalaxyReqKind::Collection,
            requirements: PathBuf::from("collections/requirements.yml"),
        };
        let role_install = GalaxyInstall {
            kind: GalaxyReqKind::Role,
            requirements: PathBuf::from("roles/requirements.yml"),
        };

        let (program, args) = galaxy_install_argv(&collection_install, &collections, &roles);
        assert_eq!(program, "ansible-galaxy");
        assert_eq!(
            args,
            vec![
                OsString::from("collection"),
                OsString::from("install"),
                OsString::from("-r"),
                OsString::from("collections/requirements.yml"),
                OsString::from("-p"),
                OsString::from("/cache/abc/collections"),
            ]
        );

        let (_, role_args) = galaxy_install_argv(&role_install, &collections, &roles);
        assert_eq!(role_args[0], OsString::from("role"));
        assert_eq!(role_args[5], OsString::from("/cache/abc/roles"));
    }

    #[test]
    fn galaxy_requirements_hash_is_deterministic_and_content_sensitive() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_requirements(dir.path(), "requirements.yml", "collections:\n  - a\n");
        let plan = plan_galaxy_installs(dir.path());
        let first = galaxy_requirements_hash(dir.path(), &plan).expect("hash");
        let again = galaxy_requirements_hash(dir.path(), &plan).expect("hash");
        assert_eq!(first, again, "same content must yield the same cache key");

        fs::write(dir.path().join("requirements.yml"), "collections:\n  - b\n").expect("rewrite");
        let changed = galaxy_requirements_hash(dir.path(), &plan).expect("hash");
        assert_ne!(
            first, changed,
            "changed requirements must map to a fresh cache key"
        );
    }

    #[test]
    fn galaxy_consume_paths_prepend_cache_to_system_path() {
        let paths = galaxy_consume_paths(
            Path::new("/cache/abc/collections"),
            Path::new("/cache/abc/roles"),
        );

        assert_eq!(
            paths.collections_path,
            OsString::from(format!(
                "/cache/abc/collections:{GALAXY_SYSTEM_COLLECTIONS_PATH}"
            )),
        );
        assert_eq!(
            paths.roles_path,
            OsString::from(format!("/cache/abc/roles:{GALAXY_SYSTEM_ROLES_PATH}")),
        );
    }

    #[test]
    fn playbook_env_advertises_galaxy_cache_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let job = sample_job(json!({}));
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let galaxy = galaxy_consume_paths(
            Path::new("/cache/x/collections"),
            Path::new("/cache/x/roles"),
        );

        let with_galaxy = build_playbook_envvars(&job, &settings, &pdd, None, Some(&galaxy));
        assert_eq!(
            with_galaxy.get("ANSIBLE_COLLECTIONS_PATH"),
            Some(&galaxy.collections_path)
        );
        assert_eq!(
            with_galaxy.get("ANSIBLE_ROLES_PATH"),
            Some(&galaxy.roles_path)
        );

        // Absent requirements must leave the play env untouched.
        let without_galaxy = build_playbook_envvars(&job, &settings, &pdd, None, None);
        assert!(!without_galaxy.contains_key("ANSIBLE_COLLECTIONS_PATH"));
        assert!(!without_galaxy.contains_key("ANSIBLE_ROLES_PATH"));
    }

    #[test]
    fn galaxy_env_is_secret_free_and_config_pinned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let job = sample_job(json!({}));
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let parent_env = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/runner"),
            (
                "PIPING_ANSIBLE_RUNNER__SHARED_SECRET",
                "control-plane-secret",
            ),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));

        let envs = build_galaxy_env(parent_env, &pdd);

        assert!(
            !envs.contains_key("PIPING_ANSIBLE_RUNNER__SHARED_SECRET"),
            "galaxy must not inherit the control-plane secret: {:?}",
            envs.keys().collect::<Vec<_>>()
        );
        assert!(envs.contains_key("PATH"));
        assert_eq!(
            envs.get("ANSIBLE_CONFIG"),
            Some(&pdd.project.join("ansible.cfg").into_os_string()),
            "galaxy must read the generated ansible.cfg, not a bundle-supplied one"
        );
    }

    #[test]
    fn mask_url_userinfo_redacts_credentials_in_output() {
        assert_eq!(
            mask_url_userinfo("Cloning git+https://oauth2:ghp_secret@example.com/repo.git"),
            "Cloning git+https://***@example.com/repo.git"
        );
        // Ordinary lines are borrowed unchanged.
        assert_eq!(
            mask_url_userinfo("Installing collection community.general"),
            "Installing collection community.general"
        );
    }

    #[test]
    fn publish_galaxy_cache_moves_staging_into_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let staging = dir.path().join(".staging-1");
        let cache_dir = dir.path().join("deadbeef");
        fs::create_dir_all(staging.join("collections")).expect("seed staging");
        fs::write(staging.join("collections/marker"), b"x").expect("marker");

        publish_galaxy_cache(&staging, &cache_dir).expect("publish");

        assert!(
            cache_dir.join("collections/marker").exists(),
            "cache populated"
        );
        assert!(!staging.exists(), "staging consumed by the rename");
    }

    #[test]
    fn publish_galaxy_cache_discards_staging_when_peer_won() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path().join("deadbeef");
        // A concurrent job already published this content-addressed dir.
        fs::create_dir_all(cache_dir.join("collections")).expect("peer cache");
        fs::write(cache_dir.join("collections/peer"), b"peer").expect("peer marker");
        let staging = dir.path().join(".staging-2");
        fs::create_dir_all(staging.join("collections")).expect("seed staging");
        fs::write(staging.join("collections/mine"), b"mine").expect("my marker");

        publish_galaxy_cache(&staging, &cache_dir).expect("peer-won is not an error");

        assert!(!staging.exists(), "losing staging must be discarded");
        assert!(
            cache_dir.join("collections/peer").exists(),
            "peer cache untouched"
        );
        assert!(
            !cache_dir.join("collections/mine").exists(),
            "peer cache must not be clobbered by the loser"
        );
    }

    #[tokio::test]
    async fn install_galaxy_requirements_reuses_populated_cache_without_network() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().to_path_buf());
        config.galaxy_cache_dir = dir.path().join("galaxy_cache");
        let mut job = sample_job(json!({}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        write_requirements(
            &pdd.project,
            "requirements.yml",
            "collections:\n  - community.general\n",
        );

        // Pre-seed the content-addressed cache dir so the step is a hit: it must
        // resolve without spawning ansible-galaxy (no network, no binary needed).
        let plan = plan_galaxy_installs(&pdd.project);
        let hash = galaxy_requirements_hash(&pdd.project, &plan).expect("hash");
        let cache_dir = config.galaxy_cache_dir.join(&hash);
        fs::create_dir_all(cache_dir.join("collections")).expect("seed cache");

        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");
        let result = install_galaxy_requirements(&config, &job, &pdd, &dispatcher)
            .await
            .expect("cache hit must succeed offline");
        dispatcher.finish().await;

        let paths = result.expect("galaxy paths present when requirements exist");
        assert_eq!(
            paths.collections_path,
            galaxy_consume_paths(&cache_dir.join("collections"), &cache_dir.join("roles"))
                .collections_path
        );
    }

    #[tokio::test]
    async fn run_galaxy_command_reaps_when_grandchild_holds_pipe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().to_path_buf());
        config.output_drain_grace = Duration::from_millis(200);
        let mut job = sample_job(json!({}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");

        // A backgrounded grandchild inherits the pipe; the drain grace must
        // force the readers closed so the step still returns on the real exit.
        let args = [
            OsString::from("-c"),
            OsString::from("echo installing; sleep 10 & exit 0"),
        ];
        let result = timeout(
            Duration::from_secs(5),
            run_galaxy_command(&config, &job, &pdd, &dispatcher, "sh", &args),
        )
        .await
        .expect("galaxy step must not hang on a leftover grandchild");
        dispatcher.finish().await;

        assert!(
            result.is_ok(),
            "a zero-exit install must succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_galaxy_command_times_out_and_reports_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(dir.path().to_path_buf());
        config.galaxy_install_timeout = Duration::from_millis(300);
        let mut job = sample_job(json!({}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");

        let args = [OsString::from("-c"), OsString::from("sleep 30")];
        let result = timeout(
            Duration::from_secs(5),
            run_galaxy_command(&config, &job, &pdd, &dispatcher, "sh", &args),
        )
        .await
        .expect("timed-out galaxy step must return, not hang");
        dispatcher.finish().await;

        let error = result.expect_err("a hung install must surface as an error");
        assert!(
            error.to_string().contains("timed out"),
            "operator must see why the step failed: {error}"
        );
    }

    #[tokio::test]
    async fn run_galaxy_command_surfaces_nonzero_exit_with_masked_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");
        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");

        // Emit a tokened URL then fail: the failure tail must carry the reason
        // but never the credential.
        let args = [
            OsString::from("-c"),
            OsString::from("echo 'cloning https://user:tok3n@example.com/repo'; exit 7"),
        ];
        let result = run_galaxy_command(&config, &job, &pdd, &dispatcher, "sh", &args).await;
        dispatcher.finish().await;

        let error = result
            .expect_err("nonzero exit must fail the step")
            .to_string();
        assert!(
            error.contains("status 7"),
            "exit code must be reported: {error}"
        );
        assert!(
            error.contains("//***@") && !error.contains("tok3n"),
            "credential in the failure tail must be masked: {error}"
        );
    }

    #[tokio::test]
    async fn install_galaxy_requirements_is_noop_without_requirements() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path().to_path_buf());
        let mut job = sample_job(json!({}));
        job.callback_base_url = "http://127.0.0.1:9".to_string();
        let settings = RunnerSettings::parse(&job.settings).expect("settings");
        let pdd = materialize_private_data_dir(&config, &job, &settings).expect("materialize");

        let dispatcher = EventDispatcher::new(&job, &config).expect("dispatcher");
        let result = install_galaxy_requirements(&config, &job, &pdd, &dispatcher)
            .await
            .expect("no requirements must succeed");
        dispatcher.finish().await;

        assert!(
            result.is_none(),
            "a project without requirements must not produce galaxy paths"
        );
    }
}
