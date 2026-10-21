//! SSH proxy sessions: attachable shell relays, one-shot exec, SCP and remote file transfer, host-key verification and agent-facing session helpers.
use std::collections::VecDeque;
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};

use chrono::{TimeZone, Utc};
use futures_util::{Sink, SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
#[cfg(test)]
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::time::{Duration, Instant, timeout};
use uuid::Uuid;

use crate::contexts::shared::host_key_pins::{
    delete_pin_statement, known_hosts_lines_from_payload, mismatch_diagnosis,
    parse_known_hosts_line, write_seed_file,
};
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_error_event, log_event};
use piping_contracts::playbook_runner::{JOB_KNOWN_HOSTS_FILE, pinned_host_key_options};

use super::metrics::{
    ConnectionMetricsState, LinkType, TrafficDirection, mark_connection_metrics_connected,
    mark_connection_metrics_disconnected, mark_connection_metrics_error, record_connection_traffic,
};
use super::pty_unix::{apply_pty_resize, create_pty_pair, send_sigwinch};
use super::{
    AgentSshExecRequest, AgentSshExecResult, AgentSshExecStream, AgentSshFileDeleteResult,
    AgentSshFileDownloadPreflight, AgentSshFileEntry, AgentSshFileList, AgentSshFileMoveResult,
    AgentSshFilePreview, AgentSshFileTransferResult, AgentSshReadChunk, AgentSshReadSnapshot,
    ProxyConnectionType, ProxyState,
};

mod agent;
mod attach;
mod command_options;
mod config;
mod download;
mod exec;
mod host_keys;
mod material;
mod readiness;
mod relay;
mod remote_fs;
mod scp;
mod session_start;

use agent::*;
use attach::*;
use command_options::*;
use config::*;
use download::*;
use exec::*;
use host_keys::*;
use material::*;
use readiness::*;
use relay::*;
use remote_fs::*;
use scp::*;
use session_start::*;

pub(super) use agent::{
    agent_ssh_delete_files, agent_ssh_download_file, agent_ssh_ensure_dir, agent_ssh_exec,
    agent_ssh_list_dir, agent_ssh_move_files, agent_ssh_preflight_download_file,
    agent_ssh_preview_file, agent_ssh_session, agent_ssh_upload_file,
};
pub(super) use attach::{proxy_ws_ssh_attachable_loop, reap_detached_ssh_sessions};
pub(super) use remote_fs::remote_file_name;
pub(super) use session_start::ssh_session_init_lock;

/// Runs a one-shot SSH exec against a proxy payload that has no registered
/// connection.
///
/// The connection id and metrics state are throwaway: lifecycle recording is
/// confined to that per-call state and never reaches a registered connection,
/// and no ControlMaster session is required.
pub(super) async fn ssh_exec_payload(
    payload: Value,
    request: AgentSshExecRequest,
) -> Result<AgentSshExecResult, AppError> {
    run_ssh_exec_runtime(
        Uuid::now_v7(),
        &payload,
        Arc::new(RwLock::new(ConnectionMetricsState::default())),
        request,
        SshProcessMetricsMode::Lifecycle,
        None,
    )
    .await
}

const WS_CONTROL_PREFIX: &str = "__piping_ws__:";
const WS_CLOSE_NORMAL: u16 = 1000;
/// Close reason used when the *client* leg dropped but the SSH session is still
/// alive and re-attachable. The upstream API bridge keys on this reason to keep
/// the DB session non-terminal (re-attachable) instead of marking it
/// `Disconnected`. Sent with [`WS_CLOSE_NORMAL`].
const WS_REASON_DETACHED_ALIVE: &str = "detached_alive";
/// Close reason used when the SSH session has actually ended (PTY EOF, target
/// died, or explicit shutdown). The API bridge treats this as terminal.
const WS_REASON_SESSION_ENDED: &str = "SSH session ended";
/// We poll the per-session ControlMaster socket rather than treating process
/// spawn as an upstream connection. OpenSSH creates this socket only once its
/// authenticated master connection is usable for multiplexed sessions. There
/// is deliberately no readiness deadline here: terminal state, not elapsed
/// time, decides an unsuccessful startup.
const SSH_CONTROL_MASTER_READY_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Sidecars must never establish their own network connection. OpenSSH falls
/// back to a normal connection when `ControlMaster=no` cannot reach the
/// requested control socket, so mux-only commands use this deliberately
/// failing transport for that fallback path. A live ControlMaster is consulted
/// first and therefore bypasses it.
/// The shipped Debian runtime keeps this Essential `coreutils` binary.
const SSH_MUX_ONLY_FALLBACK_BLOCKER: &str = "/usr/bin/false";
/// The interactive master writes this marker using OpenSSH's `LocalCommand`
/// only after it has successfully connected to the upstream server.  It lives
/// in the generation-scoped, owner-only session material directory and is
/// retained until terminal classification finishes, so a short-lived master
/// cannot lose its authenticated-history fact between readiness polls.
const SSH_AUTHENTICATION_MARKER_FILE: &str = "authenticated";
/// The shipped Debian runtime provides this Essential `coreutils` binary.  An
/// absolute path avoids PATH-dependent LocalCommand behavior.
const SSH_AUTHENTICATION_MARKER_TOUCH: &str = "/usr/bin/touch";
/// OpenSSH stderr markers that mean the presented host key is not the pinned
/// one. Either marker must fail the operation: retrying or continuing would be
/// the silent re-trust that host key pinning exists to prevent.
const SSH_HOST_KEY_CHANGED_MARKER: &str = "REMOTE HOST IDENTIFICATION HAS CHANGED";
const SSH_HOST_KEY_VERIFICATION_FAILED_MARKER: &str = "Host key verification failed";
/// Server-side keepalive ping interval on the proxy attach loop. A silently
/// dead client TCP is surfaced as a send error within roughly this window, so
/// the detach-grace timer starts promptly rather than waiting on the next
/// inbound frame.
const SSH_ATTACH_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const WS_CLOSE_INVALID_PAYLOAD: u16 = 1008;
const WS_CLOSE_INTERNAL_ERROR: u16 = 1011;
const WS_CLOSE_TARGET_UNAVAILABLE: u16 = 1013;
const SSH_PTY_BUFFER_SIZE: usize = 8_192;
const SSH_EXEC_BUFFER_SIZE: usize = 8_192;
const SSH_FILE_LIST_MAX_BYTES: usize = 4 * 1024 * 1024;
const SSH_FILE_LIST_TIMEOUT: Duration = Duration::from_secs(20);
const SSH_FILE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const SSH_FILE_REMOTE_PATH_MAX_CHARS: usize = 4096;
const SSH_FILE_ARCHIVE_MAX_ENTRIES: u64 = 100_000;
const SSH_FILE_PREVIEW_MAX_BYTES: usize = 64 * 1024;
const SSH_FILE_PREVIEW_OUTPUT_MAX_BYTES: usize = SSH_FILE_PREVIEW_MAX_BYTES + 1024;
const SSH_FILE_DELETE_MAX_PATHS: usize = 128;
const RESIZE_LOG_INITIAL_LIMIT: u64 = 5;
const RESIZE_LOG_EVERY_NTH_EVENT: u64 = 20;
const INBOUND_FRAME_LOG_INITIAL_LIMIT: u64 = 5;
const INBOUND_FRAME_LOG_EVERY_NTH_EVENT: u64 = 50;
const STDERR_EXCERPT_CHAR_LIMIT: usize = 240;
const SSH_OUTPUT_BROADCAST_CAPACITY: usize = 1024;
const SSH_HISTORY_LIMIT_BYTES: usize = 1024 * 1024;
/// While the SSH handshake is pending, retain client input until the session
/// is ready instead of writing it into a potential authentication prompt. The
/// cap keeps a stalled upstream from turning client buffering into an
/// unbounded-memory path.
const SSH_PENDING_INPUT_MAX_BYTES: usize = SSH_HISTORY_LIMIT_BYTES;
const BROWSER_READY_ENQUEUE_OPEN: u8 = 0;
const BROWSER_READY_ENQUEUE_CLAIMED: u8 = 1;
const BROWSER_READY_ENQUEUE_SENDING: u8 = 2;
const BROWSER_READY_ENQUEUE_EXITED: u8 = 3;
/// Hard cap on the number of concurrently live SSH sessions in the registry.
///
/// Re-attaches (registry hits) are unaffected; only new session spawns are
/// rejected once this limit is reached. Prevents abandon-and-pin DoS scenarios
/// where many abandoned-but-alive sessions exhaust file-descriptors / PTYs.
/// A per-user cap is a follow-up; this global backstop is sufficient for now.
const MAX_CONCURRENT_SSH_SESSIONS: usize = 256;

#[derive(Debug, Deserialize)]
struct WsControlMessage {
    #[serde(rename = "type")]
    control_type: String,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
}

impl WsControlMessage {
    fn resize_dimensions(&self) -> Option<(u16, u16)> {
        if self.control_type != "resize" {
            return None;
        }
        let cols = self.cols?;
        let rows = self.rows?;
        if cols == 0 || rows == 0 {
            return None;
        }
        Some((cols, rows))
    }
}

#[derive(Debug, Clone)]
struct SshLaunchConfig {
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
    private_key: Option<String>,
    /// Ordered bastion/jump-host chain (least → most distant from the target).
    /// Empty for direct connections, which leaves the launch unchanged.
    jump: Vec<SshJumpHop>,
}

/// A single resolved hop in an SSH `ProxyJump` chain. Built from the effective
/// connection-config `jump[]` field, where each hop record was resolved to a
/// concrete host/port/credentials by the connection-config resolver.
#[derive(Debug, Clone)]
struct SshJumpHop {
    host: String,
    port: u16,
    username: String,
    private_key: Option<String>,
}

impl SshJumpHop {
    /// Render the hop as an OpenSSH `ProxyJump` destination (`[user@]host[:port]`).
    fn proxy_jump_spec(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.username, self.host)
        } else {
            format!("{}@{}:{}", self.username, self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SshProcessMetricsMode {
    Lifecycle,
    Sidecar,
}

impl SshProcessMetricsMode {
    fn records_lifecycle(self) -> bool {
        matches!(self, Self::Lifecycle)
    }
}

enum SshRuntimeInput {
    Bytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Shutdown,
}

#[derive(Clone)]
struct SshHistoryChunk {
    seq: u64,
    bytes: Vec<u8>,
}

struct SshHistory {
    chunks: VecDeque<SshHistoryChunk>,
    bytes: usize,
    next_seq: u64,
}

struct SshOutputState {
    close_code: u16,
    close_reason: &'static str,
    close_reason_source: &'static str,
    stderr_excerpt: Option<String>,
    relay_failed: bool,
}

/// Mutable lifecycle facts gathered by the SSH relay before it derives the
/// terminal frame. Keeping these facts separate from [`SshOutputState`] makes
/// the event-loop, child wait and reader drain independently auditable.
#[derive(Default)]
struct SshRelayState {
    child_status: Option<std::process::ExitStatus>,
    child_exit_observed: bool,
    reader_done: bool,
    input_relay_failed: bool,
}

enum SshRuntimeInputOutcome {
    Continue,
    Stop { input_relay_failed: bool },
}

/// Immutable terminal facts used for metrics, publication and the final event
/// after the relay has fully stopped touching the PTY and SSH child.
struct SshTerminalFinalization {
    child_status: Option<std::process::ExitStatus>,
    close_code: u16,
    close_reason: String,
    close_reason_source: &'static str,
    relay_failed: bool,
    stderr_excerpt: Option<String>,
}

/// Resources that remain owned by the interactive relay after startup has
/// transferred the child to its exit watcher. Its fields are intentionally
/// kept together so lifecycle helpers cannot accidentally mix generations.
struct SshRelayRuntime {
    connection_id: Uuid,
    material: SshSessionMaterial,
    terminal_session: Arc<SshSession>,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
    child_pid: Option<i32>,
    slave_pty_resize: std::fs::File,
    master_pty_resize: std::fs::File,
    pty_writer: tokio::fs::File,
    output_state: Arc<Mutex<SshOutputState>>,
    resize_applied_count: u64,
    /// Target endpoint and its seed lines, kept so a host key mismatch in the
    /// PTY output can be refused by name with the pinned fingerprint.
    host: String,
    port: u16,
    seed_lines: Vec<String>,
}

/// One-shot and stream handles that advance the relay. They stay separate from
/// [`SshRelayRuntime`] so the select loop can borrow them independently from
/// the mutable PTY writer and resize state.
struct SshRelayChannels {
    input_rx: mpsc::Receiver<SshRuntimeInput>,
    child_exit_rx: tokio::sync::oneshot::Receiver<std::io::Result<std::process::ExitStatus>>,
    child_shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    reader_task: tokio::task::JoinHandle<std::io::Result<()>>,
}

/// Private material owned by one interactive SSH process generation.  The
/// directory is created atomically with mode `0700`; its exact ControlMaster
/// path is therefore a capability for that generation only.  A successor
/// always receives a different directory, so delayed cleanup or an already
/// authorized sidecar cannot address the successor's master.
#[derive(Debug, Clone)]
struct SshSessionMaterial {
    dir: PathBuf,
    control_path: String,
    authentication_marker: PathBuf,
}

/// Private-key files materialized for one interactive SSH runtime. The files
/// live only in that runtime's generation directory and are removed together
/// with the directory at startup failure or terminal cleanup.
struct SshSessionKeyPaths {
    target_private_key: Option<String>,
    jump_private_keys: Vec<String>,
}

/// SSH command and credential facts prepared before spawning the interactive
/// child. Keeping preparation separate makes its cleanup boundary explicit:
/// any failure before `ssh` exists removes the generation material.
struct PreparedInteractiveSshCommand {
    command: Command,
    password: Option<String>,
}

/// PTY endpoints retained by the runtime after the slave endpoints have been
/// transferred to the SSH child command.
struct InteractiveSshPty {
    master_pty: std::fs::File,
    slave_pty_resize: std::fs::File,
}

impl Default for SshOutputState {
    fn default() -> Self {
        Self {
            close_code: WS_CLOSE_NORMAL,
            close_reason: "SSH session ended",
            close_reason_source: "default",
            stderr_excerpt: None,
            relay_failed: false,
        }
    }
}

#[derive(Debug, Clone)]
struct SshTerminalState {
    close_code: u16,
    close_reason: String,
}

impl SshTerminalState {
    fn session_ended() -> Self {
        Self {
            close_code: WS_CLOSE_NORMAL,
            close_reason: WS_REASON_SESSION_ENDED.to_string(),
        }
    }
}

/// A child exit or terminal transition seals ready enqueue before the relay
/// publishes its final close code. Attach loops wait for that publication
/// instead of returning a synthetic-ready or close-less result in the narrow
/// seal-to-terminal window.
async fn await_terminal_after_lifecycle_seal(
    terminal_rx: &mut watch::Receiver<Option<SshTerminalState>>,
) -> SshTerminalState {
    if let Some(terminal) = terminal_rx.borrow().clone() {
        return terminal;
    }
    match terminal_rx.changed().await {
        Ok(()) => terminal_rx
            .borrow()
            .clone()
            .unwrap_or_else(SshTerminalState::session_ended),
        Err(_) => SshTerminalState::session_ended(),
    }
}

enum SshAttachOutcome {
    ClientLegDropped,
    SessionEnded(SshTerminalState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SshPendingInputResult {
    Buffered,
    ClientLegDropped,
    LimitExceeded,
}

impl SshHistory {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            next_seq: 0,
        }
    }

    fn push(&mut self, bytes: Vec<u8>) -> SshHistoryChunk {
        self.bytes = self.bytes.saturating_add(bytes.len());
        let chunk = SshHistoryChunk {
            seq: self.next_seq,
            bytes,
        };
        self.chunks.push_back(chunk.clone());
        self.next_seq = self.next_seq.saturating_add(1);
        while self.bytes > SSH_HISTORY_LIMIT_BYTES {
            let Some(removed) = self.chunks.pop_front() else {
                self.bytes = 0;
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.bytes.len());
        }
        chunk
    }

    fn snapshot(&self) -> Vec<SshHistoryChunk> {
        self.chunks.iter().cloned().collect()
    }

    fn snapshot_since(&self, after_seq: Option<u64>, max_bytes: usize) -> AgentSshReadSnapshot {
        let max_bytes = max_bytes.max(1);
        let selected = self
            .chunks
            .iter()
            .filter(|chunk| after_seq.is_none_or(|seq| chunk.seq > seq))
            .cloned()
            .collect::<Vec<_>>();

        let mut bytes = 0usize;
        let mut start = selected.len();
        let mut truncated = false;
        for (index, chunk) in selected.iter().enumerate().rev() {
            let next_bytes = bytes.saturating_add(chunk.bytes.len());
            if next_bytes > max_bytes && start < selected.len() {
                truncated = true;
                break;
            }
            if next_bytes > max_bytes {
                start = index;
                truncated = true;
                break;
            }
            bytes = next_bytes;
            start = index;
        }

        AgentSshReadSnapshot {
            chunks: selected
                .into_iter()
                .skip(start)
                .map(|chunk| AgentSshReadChunk {
                    seq: chunk.seq,
                    bytes: chunk.bytes,
                })
                .collect(),
            next_seq: self.next_seq,
            history_start_seq: self.chunks.front().map(|chunk| chunk.seq),
            truncated,
        }
    }
}

pub(super) struct SshSession {
    input_tx: mpsc::Sender<SshRuntimeInput>,
    output_tx: broadcast::Sender<SshHistoryChunk>,
    history: RwLock<SshHistory>,
    attach_lock: Mutex<()>,
    /// OpenSSH ControlMaster socket for file-browser sidecar operations.
    /// The socket is created under the per-session 0700 temp directory and is
    /// removed with the rest of the session material on teardown.
    control_path: Option<String>,
    /// Instant at which the most recent client leg detached while the SSH
    /// session was still alive. `None` means a client is currently attached
    /// (or the session has never been attached yet). Drives the detach-grace
    /// reaper: a session whose detach is older than the grace window is GC'd.
    detached_at: Mutex<Option<Instant>>,
    /// Set exactly once when the backend SSH runtime has ended. This is
    /// independent of the client WebSocket leg: a browser may disappear before
    /// startup failure is known, but later re-attaches must still get a
    /// terminal close instead of a fake-ready black terminal.
    terminal_tx: watch::Sender<Option<SshTerminalState>>,
    /// Flips only after the OpenSSH ControlMaster socket proves that the
    /// upstream SSH connection completed its authenticated handshake. The
    /// browser-facing `ready` control frame must never be emitted at spawn.
    ready_tx: watch::Sender<bool>,
    /// Published synchronously by the child waiter after it acquires the same
    /// short browser-ready boundary as `start_send`. It deliberately precedes
    /// the relay's slower PTY drain, diagnostic classification, and filesystem
    /// cleanup, so no later readiness probe can revive a dead process.
    process_exit_tx: watch::Sender<bool>,
    /// The child waiter sets this under the browser-ready gate, after a
    /// currently synchronous `start_send` has either completed or yielded the
    /// boundary. It is the durable source of truth for an observed process
    /// exit; the watch channel below wakes tasks that are already blocked in a
    /// select loop.
    process_exit_seal: AtomicBool,
    /// Linearizes the actual browser-ready frame enqueue against terminal
    /// publication. It is deliberately a synchronous mutex because the
    /// protected operation is only a watch update or `start_send`, never an
    /// awaited network flush. Child exit takes this same gate before publishing
    /// its atomic seal.
    browser_ready_gate: StdMutex<()>,
    /// A one-shot notification used only by deterministic gate-contention
    /// regressions. It fires immediately before a real caller blocks on
    /// `browser_ready_gate`.
    #[cfg(test)]
    browser_ready_gate_waiter: StdMutex<Option<std::sync::mpsc::Sender<()>>>,
    /// Atomically records whether a ready-frame enqueue is claimed, committed
    /// to `start_send`, or whether terminal/child-exit has won. This makes the
    /// exact enqueue point linearizable against the exit seal without holding
    /// a mutex over any await.
    browser_ready_enqueue_state: AtomicU8,
    /// Serializes the one-way connecting → ready / terminal transitions so
    /// neither may overwrite the other after an await in metric recording.
    lifecycle_transition: Mutex<()>,
}

impl SshSession {
    fn control_path(&self) -> Option<String> {
        self.control_path.clone()
    }

    async fn history_snapshot(&self) -> Vec<SshHistoryChunk> {
        self.history.read().await.snapshot()
    }

    /// Records that the client leg has detached while the SSH session remains
    /// alive, starting the grace timer for the reaper.
    pub(super) async fn mark_detached(&self) {
        *self.detached_at.lock().await = Some(Instant::now());
    }

    /// Clears any pending detach timestamp because a client (re)attached.
    pub(super) async fn mark_attached(&self) {
        *self.detached_at.lock().await = None;
    }

    /// Returns how long the session has been detached, or `None` if a client
    /// is currently attached.
    async fn detached_for(&self, now: Instant) -> Option<Duration> {
        self.detached_at
            .lock()
            .await
            .map(|detached_at| now.saturating_duration_since(detached_at))
    }

    pub(super) async fn shutdown(&self) {
        let _ = self.input_tx.send(SshRuntimeInput::Shutdown).await;
    }

    fn terminal_state(&self) -> Option<SshTerminalState> {
        self.terminal_tx.borrow().clone()
    }

    fn terminal_receiver(&self) -> watch::Receiver<Option<SshTerminalState>> {
        self.terminal_tx.subscribe()
    }

    fn is_ready(&self) -> bool {
        *self.ready_tx.borrow()
    }

    fn ready_receiver(&self) -> watch::Receiver<bool> {
        self.ready_tx.subscribe()
    }

    fn process_exit_sealed(&self) -> bool {
        self.process_exit_seal.load(AtomicOrdering::Acquire)
    }

    fn process_exit_receiver(&self) -> watch::Receiver<bool> {
        self.process_exit_tx.subscribe()
    }

    fn lock_browser_ready_gate(&self) -> StdMutexGuard<'_, ()> {
        #[cfg(test)]
        self.notify_browser_ready_gate_waiter();
        self.browser_ready_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Arms a one-shot notification just before a real caller attempts the
    /// shared browser-ready gate. The test uses this to distinguish an actual
    /// blocked production seal from a thread that merely has not been
    /// scheduled yet.
    #[cfg(test)]
    fn arm_browser_ready_gate_waiter(&self, waiter: std::sync::mpsc::Sender<()>) {
        let mut armed_waiter = self
            .browser_ready_gate_waiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            armed_waiter.replace(waiter).is_none(),
            "only one browser-ready gate waiter may be armed per test session"
        );
    }

    #[cfg(test)]
    fn notify_browser_ready_gate_waiter(&self) {
        let waiter = self
            .browser_ready_gate_waiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(waiter) = waiter {
            let _ = waiter.send(());
        }
    }

    /// Called by the sole child waiter immediately after `wait()` completes.
    /// This is intentionally synchronous. Its browser-ready gate is the
    /// linearization boundary shared with the non-awaiting `start_send`: if a
    /// ready sender acquired the gate first, it completes that synchronous
    /// enqueue attempt before the exit becomes observable; otherwise this seal
    /// prevents the enqueue.
    fn seal_process_exit(&self) {
        let _browser_ready_gate = self.lock_browser_ready_gate();
        self.publish_process_exit_after_ready_gate();
    }

    /// Publishes process exit only after the caller acquired the shared
    /// browser-ready boundary. Keeping this side effect in one helper makes
    /// the ordered publication path explicit.
    fn publish_process_exit_after_ready_gate(&self) {
        // This is the first publication of process exit. Holding the same
        // gate through the `EXITED` store means it cannot land between the
        // ready sender's `SENDING` CAS and its synchronous `start_send`.
        self.browser_ready_enqueue_state
            .store(BROWSER_READY_ENQUEUE_EXITED, AtomicOrdering::Release);
        self.process_exit_seal.store(true, AtomicOrdering::Release);
        self.process_exit_tx.send_replace(true);
    }

    /// Whether it is still safe for an attach loop to send the browser-facing
    /// `ready` frame. A prior ready transition does not remain attachable once
    /// the child waiter has observed process exit.
    fn can_emit_browser_ready(&self) -> bool {
        self.is_ready()
            && !self.process_exit_sealed()
            && self.terminal_state().is_none()
            && !self.browser_ready_enqueue_ended()
    }

    fn browser_ready_enqueue_ended(&self) -> bool {
        self.browser_ready_enqueue_state
            .load(AtomicOrdering::Acquire)
            == BROWSER_READY_ENQUEUE_EXITED
    }

    /// A rejected ready enqueue can observe its atomic exit seal before the
    /// process-exit watch and terminal state are published. Such an attach
    /// must wait for the authoritative terminal close rather than return
    /// without a browser-visible result.
    fn ready_enqueue_lifecycle_sealed(&self) -> bool {
        self.process_exit_sealed() || self.browser_ready_enqueue_ended()
    }

    /// Claims the short, non-awaiting critical section that enqueues the
    /// browser-facing ready frame. If child exit or terminal state won first,
    /// the caller must not enqueue that frame.
    fn claim_browser_ready_enqueue(&self) -> Option<StdMutexGuard<'_, ()>> {
        let gate = self.lock_browser_ready_gate();
        if !self.can_emit_browser_ready() {
            return None;
        }
        self.browser_ready_enqueue_state
            .compare_exchange(
                BROWSER_READY_ENQUEUE_OPEN,
                BROWSER_READY_ENQUEUE_CLAIMED,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .ok()
            .map(|_| gate)
    }

    /// Commits the claimed ready frame immediately before `start_send`. The
    /// caller holds the browser-ready gate shared with `seal_process_exit`, so
    /// this `SENDING` transition and the synchronous enqueue are one ordered
    /// operation relative to process-exit publication.
    fn begin_browser_ready_enqueue(&self) -> bool {
        self.browser_ready_enqueue_state
            .compare_exchange(
                BROWSER_READY_ENQUEUE_CLAIMED,
                BROWSER_READY_ENQUEUE_SENDING,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_ok()
    }

    fn release_browser_ready_enqueue_claim(&self) {
        let _ = self.browser_ready_enqueue_state.compare_exchange(
            BROWSER_READY_ENQUEUE_CLAIMED,
            BROWSER_READY_ENQUEUE_OPEN,
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        );
        let _ = self.browser_ready_enqueue_state.compare_exchange(
            BROWSER_READY_ENQUEUE_SENDING,
            BROWSER_READY_ENQUEUE_OPEN,
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        );
    }

    /// Records a completed authenticated handshake exactly once. Holding the
    /// lifecycle transition while metrics are updated prevents a concurrently
    /// ending process from being overwritten back to `connected`.
    async fn mark_ready_and_connected(
        &self,
        metrics: &Arc<RwLock<ConnectionMetricsState>>,
        latency_ms: Option<i64>,
    ) -> bool {
        let _transition = self.lifecycle_transition.lock().await;
        if self.terminal_state().is_some()
            || self.process_exit_sealed()
            || self.browser_ready_enqueue_ended()
            || self.is_ready()
        {
            return false;
        }
        mark_connection_metrics_connected(metrics, latency_ms).await;
        let transition_rejected = {
            let _browser_ready_gate = self.lock_browser_ready_gate();
            if self.terminal_state().is_some()
                || self.process_exit_sealed()
                || self.browser_ready_enqueue_ended()
            {
                true
            } else {
                self.ready_tx.send_replace(true);
                false
            }
        };
        if transition_rejected {
            // The child may have exited while the asynchronous metric update
            // was in flight. Fail closed before publishing `ready`; the relay
            // later replaces this provisional error with its accurate terminal
            // metric once it finishes close classification.
            mark_connection_metrics_error(metrics).await;
            return false;
        }
        true
    }

    #[cfg(test)]
    async fn mark_ready(&self) -> bool {
        let _transition = self.lifecycle_transition.lock().await;
        if self.terminal_state().is_some()
            || self.process_exit_sealed()
            || self.browser_ready_enqueue_ended()
            || self.is_ready()
        {
            return false;
        }
        let _browser_ready_gate = self.lock_browser_ready_gate();
        if self.terminal_state().is_some()
            || self.process_exit_sealed()
            || self.browser_ready_enqueue_ended()
            || self.is_ready()
        {
            return false;
        }
        self.ready_tx.send_replace(true);
        true
    }

    async fn mark_ended(&self, close_code: u16, close_reason: impl Into<String>) {
        let _transition = self.lifecycle_transition.lock().await;
        let _browser_ready_gate = self.lock_browser_ready_gate();
        // Keep every terminal publication on the same ready-frame boundary.
        // `seal_process_exit` normally publishes first, but direct terminal
        // paths must not race a `SENDING` ready frame either.
        self.browser_ready_enqueue_state
            .store(BROWSER_READY_ENQUEUE_EXITED, AtomicOrdering::Release);
        if self.terminal_state().is_some() {
            return;
        }
        // `send` drops the value when no receiver exists. `send_replace`
        // makes terminal state durable for re-attach and agent callers.
        self.terminal_tx.send_replace(Some(SshTerminalState {
            close_code,
            close_reason: close_reason.into(),
        }));
    }

    pub(super) async fn write(&self, bytes: Vec<u8>) -> Result<(), AppError> {
        self.input_tx
            .send(SshRuntimeInput::Bytes(bytes))
            .await
            .map_err(|_| AppError::External("SSH runtime input channel closed".to_string()))
    }

    pub(super) async fn resize(&self, cols: u16, rows: u16) -> Result<(), AppError> {
        if cols == 0 || rows == 0 {
            return Err(AppError::Validation(
                "SSH terminal size must be positive".to_string(),
            ));
        }
        self.input_tx
            .send(SshRuntimeInput::Resize { cols, rows })
            .await
            .map_err(|_| AppError::External("SSH runtime input channel closed".to_string()))
    }

    pub(super) async fn read_since(
        &self,
        after_seq: Option<u64>,
        max_bytes: usize,
    ) -> AgentSshReadSnapshot {
        self.history
            .read()
            .await
            .snapshot_since(after_seq, max_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use crate::config::ProxyConfig;
    use serde_json::json;
    use tokio::sync::{Barrier, oneshot};

    const TEST_TARGET_TIMEOUT_MESSAGE: &str =
        "ssh: connect to host 10.10.10.251 port 22: Connection timed out";
    const TEST_TARGET_UNREACHABLE_MESSAGE: &str =
        "ssh: connect to host 10.10.10.251 port 22: No route to host";
    const TEST_NETWORK_UNREACHABLE_MESSAGE: &str =
        "ssh: connect to host 10.10.10.251 port 22: Network is unreachable";
    const TEST_RESIZE_COLS: u16 = 120;
    const TEST_RESIZE_ROWS: u16 = 42;
    const TEST_SSH_HOST: &str = "10.10.1.230";
    const TEST_SSH_PORT: u16 = 22;

    /// A deterministic sink whose `poll_ready` pauses before `start_send`.
    /// It lets the test place the child-exit seal precisely between sink
    /// writability and the browser-ready frame enqueue.
    struct ReadyFrameGateTestSink {
        ready_entered_tx: Option<oneshot::Sender<()>>,
        release_ready_rx: oneshot::Receiver<()>,
        enqueued_messages: Arc<StdMutex<usize>>,
    }

    impl Unpin for ReadyFrameGateTestSink {}

    impl Sink<axum::extract::ws::Message> for ReadyFrameGateTestSink {
        type Error = std::io::Error;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let sink = self.as_mut().get_mut();
            if let Some(ready_entered_tx) = sink.ready_entered_tx.take() {
                let _ = ready_entered_tx.send(());
            }
            match Pin::new(&mut sink.release_ready_rx).poll(cx) {
                Poll::Ready(_) => Poll::Ready(Ok(())),
                Poll::Pending => Poll::Pending,
            }
        }

        fn start_send(
            mut self: Pin<&mut Self>,
            _message: axum::extract::ws::Message,
        ) -> Result<(), Self::Error> {
            let sink = self.as_mut().get_mut();
            *sink
                .enqueued_messages
                .lock()
                .expect("test sink message counter lock") += 1;
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn drain_limited_stream_tracks_total_and_truncates_retained_bytes() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let writer_task = tokio::spawn(async move {
            writer.write_all(b"abcdef").await.expect("write stream");
        });

        let drained = drain_limited_stream(reader, 3).await.expect("drain stream");
        writer_task.await.expect("writer task");

        assert_eq!(drained.bytes, b"abc");
        assert_eq!(drained.bytes_read, 6);
        assert!(drained.truncated);
    }

    #[tokio::test]
    async fn bounded_file_writer_rejects_oversized_stream_before_writing_extra_chunk() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let local_path = temp_dir.path().join("download");
        let writer_task = tokio::spawn(async move {
            writer.write_all(b"abcdef").await.expect("write stream");
        });

        let error = write_bounded_stream_to_file(reader, local_path.clone(), 4)
            .await
            .expect_err("stream should exceed limit");
        writer_task.await.expect("writer task");

        assert!(matches!(error, AppError::PayloadTooLarge(_)));
        let bytes = tokio::fs::read(&local_path).await.expect("partial file");
        assert!(bytes.len() <= 4, "wrote too much: {}", bytes.len());
    }

    #[test]
    fn ssh_file_sidecar_mode_does_not_record_lifecycle() {
        assert!(SshProcessMetricsMode::Lifecycle.records_lifecycle());
        assert!(!SshProcessMetricsMode::Sidecar.records_lifecycle());
    }

    #[test]
    fn remote_download_commands_quote_shell_paths_and_use_option_separator() {
        assert_eq!(
            build_remote_download_preflight_command("./dir/weird ' file.txt"),
            "[ -f './dir/weird '\\'' file.txt' ] || exit 3; size=$(stat -c '%s' -- './dir/weird '\\'' file.txt') || exit 4; printf 'file\\0%s\\0' \"$size\""
        );
        assert_eq!(
            build_remote_download_command("-leading-dash.txt"),
            "cat -- '-leading-dash.txt'"
        );
        assert_eq!(
            build_remote_download_command("~/Desktop/file with spaces.png"),
            "cat -- ~/'Desktop/file with spaces.png'"
        );
    }

    #[test]
    fn remote_file_name_falls_back_for_empty_or_directory_like_paths() {
        assert_eq!(remote_file_name("/tmp/report.txt", false), "report.txt");
        assert_eq!(remote_file_name("~/Desktop/file.png", false), "file.png");
        assert_eq!(remote_file_name("~/", false), "download");
        assert_eq!(remote_file_name("/tmp/reports", true), "reports.tar.gz");
        assert_eq!(remote_file_name("/", true), "download.tar.gz");
    }

    #[test]
    fn archive_download_commands_keep_tar_members_relative() {
        assert_eq!(
            build_remote_archive_preflight_command("/var/log/app"),
            "[ -d '/var/log/app' ] || exit 3; bytes=$(du -sb -- '/var/log/app' 2>/dev/null | awk '{print $1}') || exit 4; entries=$(find '/var/log/app' -mindepth 0 -printf . 2>/dev/null | wc -c) || exit 4; printf 'directory\\0%s\\0%s\\0' \"$bytes\" \"$entries\""
        );
        assert_eq!(
            build_remote_archive_download_command("/var/log/app"),
            "target='/var/log/app'; [ -d \"$target\" ] || exit 3; parent=$(dirname -- \"$target\") || exit 4; name=$(basename -- \"$target\") || exit 4; cd -- \"$parent\" || exit 4; tar -czf - -- \"$name\""
        );
        assert_eq!(
            build_remote_archive_download_command("/"),
            "cd -- '/' || exit 4; [ -d . ] || exit 3; tar -czf - -- ."
        );
    }

    #[test]
    fn preview_and_file_action_commands_quote_paths() {
        assert_eq!(
            build_remote_preview_command("./dir/weird ' file.txt", 64),
            "[ -f './dir/weird '\\'' file.txt' ] || exit 3; size=$(stat -c '%s' -- './dir/weird '\\'' file.txt') || exit 4; printf 'file\\0%s\\0' \"$size\"; head -c 65 -- './dir/weird '\\'' file.txt'"
        );
        assert_eq!(
            build_remote_delete_command(&["/tmp/a b.txt".to_string(), "-dash".to_string()]),
            "rm -rf -- '/tmp/a b.txt' '-dash'"
        );
        assert_eq!(
            build_remote_move_command(&["/tmp/a b.txt".to_string()], "~/Desktop"),
            "mkdir -p -- ~/'Desktop' && mv -t ~/'Desktop' -- '/tmp/a b.txt'"
        );
    }

    #[test]
    fn delete_path_validation_rejects_dangerous_paths() {
        for path in ["/", ".", "..", "~", "../tmp", "/tmp/../etc", "/tmp/./file"] {
            assert!(
                validate_remote_delete_paths(vec![path.to_string()]).is_err(),
                "expected dangerous path to fail: {path}"
            );
        }
        assert_eq!(
            validate_remote_delete_paths(vec![
                "/tmp/report.txt".to_string(),
                "/tmp/report.txt".to_string(),
                "~/Desktop/report.txt".to_string(),
            ])
            .expect("valid delete paths"),
            vec![
                "/tmp/report.txt".to_string(),
                "~/Desktop/report.txt".to_string()
            ]
        );
    }

    #[test]
    fn download_preflight_parses_files_and_directory_archives() {
        let file =
            parse_download_preflight_output("/tmp/report.txt", false, b"file\x001234\x00", 2048)
                .expect("file preflight");
        assert_eq!(file.bytes, Some(1234));
        assert_eq!(file.logical_bytes, Some(1234));
        assert_eq!(file.entry_count, Some(1));
        assert_eq!(file.kind, "file");
        assert!(!file.archive);

        let directory = parse_download_preflight_output(
            "/tmp/reports",
            true,
            b"directory\x002048\x0012\x00",
            4096,
        )
        .expect("directory preflight");
        assert_eq!(directory.bytes, None);
        assert_eq!(directory.logical_bytes, Some(2048));
        assert_eq!(directory.entry_count, Some(12));
        assert_eq!(directory.kind, "directory");
        assert!(directory.archive);
    }

    #[test]
    fn preview_output_parses_text_and_rejects_binary() {
        let preview =
            parse_remote_preview_output("/tmp/app.log", b"file\x0012\x00hello world\n", 8, false)
                .expect("text preview");
        assert_eq!(preview.path, "/tmp/app.log");
        assert_eq!(preview.bytes, 12);
        assert_eq!(preview.preview_bytes, 8);
        assert_eq!(preview.content, "hello wo");
        assert!(preview.truncated);

        let binary = parse_remote_preview_output("/tmp/blob", b"file\x003\x00a\x00b", 64, false)
            .expect_err("binary preview");
        assert!(matches!(binary, AppError::Validation(_)));
    }

    #[test]
    fn download_size_limit_rejects_oversized_preflight() {
        assert!(enforce_download_size_limit(10, 10).is_ok());
        assert!(matches!(
            enforce_download_size_limit(11, 10),
            Err(AppError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn classify_ssh_timeout_message() {
        assert_eq!(
            classify_ssh_stderr_message(TEST_TARGET_TIMEOUT_MESSAGE),
            Some((WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out"))
        );
    }

    #[test]
    fn classify_ssh_startup_probe_timeout_close_frame() {
        let error = AppError::External(
            "SSH target 10.10.101.251:22 banner exchange timed out after 5s".to_string(),
        );

        assert_eq!(
            classify_ssh_startup_close_frame(&error),
            (WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out")
        );
    }

    #[test]
    fn classify_ssh_startup_validation_close_frame() {
        let error = AppError::Validation("bad ssh payload".to_string());

        assert_eq!(
            classify_ssh_startup_close_frame(&error),
            (WS_CLOSE_INVALID_PAYLOAD, "Invalid SSH connection payload")
        );
    }

    #[test]
    fn classify_ssh_startup_too_many_sessions_close_frame() {
        let error =
            AppError::TooManyRequests("Too many active SSH sessions; try again later".to_string());

        assert_eq!(
            classify_ssh_startup_close_frame(&error),
            (WS_CLOSE_TARGET_UNAVAILABLE, "Too many active SSH sessions")
        );
    }

    #[test]
    fn classify_ssh_auth_failure_message() {
        let message = "Permission denied, please try again.";
        assert_eq!(
            classify_ssh_stderr_message(message),
            Some((WS_CLOSE_INVALID_PAYLOAD, "SSH authentication failed"))
        );
    }

    #[test]
    fn unconfirmed_unsuccessful_exit_is_generic_and_never_auth_classified_from_pty_text() {
        let ordinary_remote_output = "cat: report: Permission denied";
        let diagnostic = classify_ssh_stderr_message(ordinary_remote_output);

        assert!(
            diagnostic.is_some(),
            "the text intentionally looks like an OpenSSH startup failure"
        );
        assert_eq!(
            resolve_ssh_exit_before_or_after_readiness(false, Some(1), WS_CLOSE_NORMAL),
            Some((
                WS_CLOSE_TARGET_UNAVAILABLE,
                "SSH connection ended before readiness was confirmed",
                "unconfirmed_unsuccessful_exit",
            )),
            "PTY text is not an input to terminal classification before authentication is confirmed"
        );
        assert_eq!(
            resolve_ssh_exit_before_or_after_readiness(false, None, WS_CLOSE_NORMAL),
            Some((
                WS_CLOSE_TARGET_UNAVAILABLE,
                "SSH connection ended before readiness was confirmed",
                "unconfirmed_unsuccessful_exit",
            )),
            "an unconfirmed signalled SSH exit must not look like a normal logout"
        );
    }

    #[test]
    fn classify_ssh_unreachable_message() {
        assert_eq!(
            classify_ssh_stderr_message(TEST_TARGET_UNREACHABLE_MESSAGE),
            Some((WS_CLOSE_TARGET_UNAVAILABLE, "Target is unreachable"))
        );
    }

    #[test]
    fn classify_ssh_network_unreachable_message() {
        assert_eq!(
            classify_ssh_stderr_message(TEST_NETWORK_UNREACHABLE_MESSAGE),
            Some((WS_CLOSE_TARGET_UNAVAILABLE, "Target is unreachable"))
        );
    }

    #[test]
    fn normalize_stderr_excerpt_compacts_whitespace() {
        let input = "ssh: connect\tto host 10.10.10.251 port 22:\nConnection timed out\r\n";
        assert_eq!(
            normalize_stderr_excerpt(input),
            Some("ssh: connect to host 10.10.10.251 port 22: Connection timed out".to_string())
        );
    }

    #[test]
    fn scp_remote_spec_does_not_shell_quote_argv_path() {
        let launch = SshLaunchConfig {
            host: "10.10.1.230".to_string(),
            port: 22,
            username: "root".to_string(),
            password: None,
            private_key: None,
            jump: Vec::new(),
        };

        assert_eq!(
            scp_remote_spec(&launch, "./upload with spaces.txt"),
            "root@10.10.1.230:./upload with spaces.txt"
        );
    }

    #[tokio::test]
    async fn ssh_session_material_is_unique_and_owner_only_per_generation() {
        let first = create_ssh_session_material()
            .await
            .expect("first generation material");
        let second = create_ssh_session_material()
            .await
            .expect("second generation material");

        assert_ne!(first.dir, second.dir);
        assert_ne!(first.control_path, second.control_path);
        assert_eq!(
            Path::new(&first.control_path).parent(),
            Some(first.dir.as_path()),
            "a sidecar control path is scoped to its generation directory"
        );
        assert_eq!(
            Path::new(&second.control_path).parent(),
            Some(second.dir.as_path()),
            "a successor receives a separate control path"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&first.dir)
                    .await
                    .expect("first material metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                tokio::fs::metadata(&second.dir)
                    .await
                    .expect("second material metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }

        cleanup_ssh_session_material(&first).await;
        cleanup_ssh_session_material(&second).await;
    }

    #[tokio::test]
    async fn mark_detached_and_attached_track_grace_timer() {
        let session = test_ssh_session();
        let now = Instant::now();
        assert!(session.detached_for(now).await.is_none());

        session.mark_detached().await;
        let later = now + Duration::from_secs(1);
        let elapsed = session
            .detached_for(later)
            .await
            .expect("detached duration tracked");
        assert!(elapsed >= Duration::from_millis(0));

        session.mark_attached().await;
        assert!(
            session.detached_for(later).await.is_none(),
            "re-attach must clear the detach timer"
        );
    }

    #[test]
    fn pending_ssh_input_detects_client_close_before_authenticated_ready() {
        let mut pending_input = VecDeque::new();
        let mut pending_input_bytes = 0;

        assert_eq!(
            queue_ssh_input_while_starting(
                &mut pending_input,
                &mut pending_input_bytes,
                axum::extract::ws::Message::Close(None),
            ),
            SshPendingInputResult::ClientLegDropped,
            "a WebSocket close during SSH authentication must release the attach loop"
        );
        assert!(pending_input.is_empty());

        assert_eq!(
            queue_ssh_input_while_starting(
                &mut pending_input,
                &mut pending_input_bytes,
                axum::extract::ws::Message::Text("ls\n".into()),
            ),
            SshPendingInputResult::Buffered,
            "ordinary input is retained until SSH is actually ready"
        );
        assert_eq!(pending_input.len(), 1);
    }

    #[tokio::test]
    async fn mark_ended_notifies_terminal_state_watchers() {
        let session = test_ssh_session();
        let mut terminal_rx = session.terminal_receiver();

        assert!(session.terminal_state().is_none());
        session
            .mark_ended(WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out")
            .await;

        terminal_rx.changed().await.expect("terminal change");
        let terminal = terminal_rx
            .borrow()
            .clone()
            .expect("terminal state must be published");
        assert_eq!(terminal.close_code, WS_CLOSE_TARGET_UNAVAILABLE);
        assert_eq!(terminal.close_reason, "Target connection timed out");
    }

    #[tokio::test]
    async fn terminal_state_persists_without_watch_receivers_and_is_not_reacquired() {
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let connection_id = Uuid::now_v7();
        let stale = test_ssh_session();
        assert!(stale.mark_ready().await, "test session should become ready");
        // `watch::Sender::send` used to discard terminal state in this exact
        // detached/agent-only case because there was no receiver alive.
        drop(stale.terminal_receiver());
        stale
            .mark_ended(WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out")
            .await;
        assert!(stale.terminal_state().is_some());

        state.connections.write().await.insert(
            connection_id,
            super::super::ProxyConnection {
                payload: json!({
                    "connection_type": "ssh",
                    "host": "127.0.0.1",
                    "credentials": { "username": "operator" }
                }),
                metrics: Arc::new(RwLock::new(ConnectionMetricsState::default())),
            },
        );
        state
            .ssh_sessions
            .write()
            .await
            .insert(connection_id, stale.clone());

        let starts = Arc::new(AtomicUsize::new(0));
        let replacement = get_or_start_ssh_session(state.clone(), connection_id, {
            let starts = starts.clone();
            move || async move {
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(test_ssh_session())
            }
        })
        .await
        .expect("terminal session should be replaced");

        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(
            !Arc::ptr_eq(&replacement, &stale),
            "a terminal session must never be returned to a new caller"
        );
        assert!(replacement.terminal_state().is_none());
    }

    #[tokio::test]
    async fn terminal_publication_waits_for_old_finalization_before_session_replacement() {
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let connection_id = Uuid::now_v7();
        let old_material = create_ssh_session_material()
            .await
            .expect("old generation material");
        let old_session = test_ssh_session_with_control_path(old_material.control_path.clone());
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();
        state.connections.write().await.insert(
            connection_id,
            super::super::ProxyConnection {
                payload: json!({"connection_type": "ssh"}),
                metrics: metrics.clone(),
            },
        );
        state
            .ssh_sessions
            .write()
            .await
            .insert(connection_id, old_session.clone());

        let old_marker = old_material.dir.join("old-material");
        tokio::fs::write(&old_marker, b"old")
            .await
            .expect("write old session marker");

        let mut terminal_rx = old_session.terminal_receiver();
        let (cleanup_started_tx, cleanup_started_rx) = oneshot::channel();
        let (release_cleanup_tx, release_cleanup_rx) = oneshot::channel();
        let finalizing_session = old_session.clone();
        let old_metrics = metrics.clone();
        let old_material_for_finalizer = old_material.clone();
        let old_metrics_finalized = Arc::new(AtomicBool::new(false));
        let old_metrics_finalized_for_finalizer = old_metrics_finalized.clone();
        let finalizer = tokio::spawn(async move {
            publish_ssh_terminal_after_finalization(
                &finalizing_session,
                async move {
                    let _ = cleanup_started_tx.send(());
                    release_cleanup_rx
                        .await
                        .expect("release old material cleanup");
                    mark_connection_metrics_error(&old_metrics).await;
                    old_metrics_finalized_for_finalizer.store(true, AtomicOrdering::Release);
                    cleanup_ssh_session_material(&old_material_for_finalizer).await;
                },
                WS_CLOSE_NORMAL,
                "SSH session ended".to_string(),
            )
            .await;
        });
        cleanup_started_rx
            .await
            .expect("old session finalization is blocked before cleanup");

        // Allocate the successor material while the old cleanup is blocked.
        // This models an asynchronous sidecar/replacement interleaving and
        // proves that cleanup has an exact generation target rather than a
        // connection-wide directory it could delete underneath the successor.
        let successor_material = create_ssh_session_material()
            .await
            .expect("successor generation material");
        let successor_marker = successor_material.dir.join("successor-material");
        tokio::fs::write(&successor_marker, b"successor")
            .await
            .expect("write successor material before old cleanup");
        assert_ne!(old_material.dir, successor_material.dir);
        assert_ne!(old_material.control_path, successor_material.control_path);

        let starts = Arc::new(AtomicUsize::new(0));
        let returned_while_cleaning = get_or_start_ssh_session(state.clone(), connection_id, {
            let starts = starts.clone();
            move || async move {
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(test_ssh_session())
            }
        })
        .await
        .expect("live old session remains cached before cleanup completes");
        assert!(Arc::ptr_eq(&returned_while_cleaning, &old_session));
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert!(old_session.terminal_state().is_none());
        assert!(
            tokio::fs::try_exists(&old_marker)
                .await
                .expect("old cleanup remains blocked"),
            "the successor factory must not run while old material exists"
        );

        release_cleanup_tx
            .send(())
            .expect("allow old material cleanup");
        terminal_rx
            .changed()
            .await
            .expect("terminal state is published after old finalization");
        finalizer.await.expect("old finalizer task");
        assert!(
            old_metrics_finalized.load(AtomicOrdering::Acquire),
            "the old metrics update completes before terminal makes replacement eligible"
        );
        assert!(
            !tokio::fs::try_exists(&old_material.dir)
                .await
                .expect("old material cleanup completes")
        );
        assert!(
            tokio::fs::try_exists(&successor_marker)
                .await
                .expect("successor material survives old cleanup"),
            "old generation cleanup must never delete a successor's keys, askpass helper, or control path"
        );

        let successor = get_or_start_ssh_session(state.clone(), connection_id, {
            let starts = starts.clone();
            let successor_marker = successor_marker.clone();
            let successor_control_path = successor_material.control_path.clone();
            let successor_metrics = metrics.clone();
            move || async move {
                tokio::fs::write(&successor_marker, b"successor")
                    .await
                    .map_err(|error| {
                        AppError::External(format!("write successor SSH material: {error}"))
                    })?;
                mark_connection_metrics_connected(&successor_metrics, Some(1)).await;
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(test_ssh_session_with_control_path(successor_control_path))
            }
        })
        .await
        .expect("replacement starts after old cleanup and terminal publication");

        assert!(!Arc::ptr_eq(&successor, &old_session));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(
            successor.mark_ready().await,
            "the replacement session becomes the only sidecar-authorizing generation"
        );
        assert_eq!(
            active_ssh_control_path(&state, connection_id)
                .await
                .expect("sidecars receive the replacement generation path"),
            successor_material.control_path
        );
        assert_ne!(
            old_material.control_path, successor_material.control_path,
            "an already-authorized old sidecar path cannot name the successor master"
        );
        assert!(
            tokio::fs::try_exists(&successor_marker)
                .await
                .expect("successor material check"),
            "old cleanup completed before the successor wrote its material"
        );
        let response = metrics.read().await.as_response(connection_id);
        assert_eq!(response.overall_status, "connected");
        assert!(response.links.iter().all(|link| link.status == "healthy"));
        cleanup_ssh_session_material(&successor_material).await;
    }

    #[tokio::test]
    async fn terminal_and_ready_race_leaves_metrics_in_terminal_error_state() {
        let session = test_ssh_session();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();
        let barrier = Arc::new(Barrier::new(2));

        let ready_session = session.clone();
        let ready_metrics = metrics.clone();
        let ready_barrier = barrier.clone();
        let ready = tokio::spawn(async move {
            ready_barrier.wait().await;
            ready_session
                .mark_ready_and_connected(&ready_metrics, Some(1))
                .await;
        });

        let terminal_session = session.clone();
        let terminal_metrics = metrics.clone();
        let terminal = tokio::spawn(async move {
            barrier.wait().await;
            terminal_session
                .mark_ended(WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out")
                .await;
            mark_connection_metrics_error(&terminal_metrics).await;
        });

        ready.await.expect("ready transition task");
        terminal.await.expect("terminal transition task");

        assert!(session.terminal_state().is_some());
        let response = metrics.read().await.as_response(Uuid::now_v7());
        assert_eq!(response.overall_status, "error");
        assert!(response.links.iter().all(|link| link.status == "unhealthy"));
    }

    #[tokio::test]
    async fn pty_reader_task_failure_is_classified_as_internal_error() {
        let output_state = Arc::new(Mutex::new(SshOutputState::default()));

        mark_ssh_pty_reader_task_failure(&output_state).await;

        let state = output_state.lock().await;
        assert!(state.relay_failed);
        assert_eq!(state.close_code, WS_CLOSE_INTERNAL_ERROR);
        assert_eq!(state.close_reason, "SSH terminal reader failed");
        assert_eq!(state.close_reason_source, "pty_reader_task_error");
    }

    #[tokio::test]
    async fn pty_read_io_error_is_classified_as_internal_error() {
        let temp_dir = tempfile::tempdir().expect("temporary PTY reader path");
        let path = temp_dir.path().join("write-only-pty");
        let write_only_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .expect("open a write-only file descriptor");
        let pty_reader = tokio::fs::File::from_std(write_only_file);
        let session = test_ssh_session();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();
        let (output_tx, _) = broadcast::channel(1);
        let output_state = Arc::new(Mutex::new(SshOutputState::default()));

        let result = run_ssh_pty_reader(
            Uuid::now_v7(),
            pty_reader,
            session,
            output_tx,
            metrics.clone(),
            output_state.clone(),
        )
        .await;

        assert!(
            result.is_err(),
            "PTY read failure must reach the relay task"
        );
        let state = output_state.lock().await;
        assert!(state.relay_failed);
        assert_eq!(state.close_code, WS_CLOSE_INTERNAL_ERROR);
        assert_eq!(state.close_reason, "SSH terminal read failed");
        assert_eq!(state.close_reason_source, "pty_read_error");
        drop(state);
        let response = metrics.read().await.as_response(Uuid::now_v7());
        assert_eq!(response.overall_status, "error");
        assert!(response.links.iter().all(|link| link.status == "unhealthy"));
    }

    #[tokio::test]
    async fn pty_write_io_error_is_classified_as_internal_terminal_error() {
        // `/dev/full` accepts an open write descriptor but fails every write
        // with ENOSPC, unlike a mode-only fixture whose behavior can vary
        // through Tokio's blocking adapter.
        let full_device = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("open deterministic write-failing device");
        let mut pty_writer = tokio::fs::File::from_std(full_device);
        let output_state = Arc::new(Mutex::new(SshOutputState::default()));

        let result =
            write_ssh_pty_input(&mut pty_writer, b"input", Uuid::now_v7(), &output_state).await;

        assert!(result.is_err(), "the PTY write must fail");
        let state = output_state.lock().await;
        assert!(state.relay_failed);
        assert_eq!(state.close_code, WS_CLOSE_INTERNAL_ERROR);
        assert_eq!(state.close_reason, "SSH terminal input write failed");
        assert_eq!(state.close_reason_source, "pty_write_error");
    }

    #[tokio::test]
    async fn child_wait_error_is_classified_as_internal_terminal_error() {
        let session = test_ssh_session();
        let output_state = Arc::new(Mutex::new(SshOutputState::default()));

        let status = resolve_ssh_child_exit_result(
            Ok(Err(std::io::Error::other("synthetic child wait failure"))),
            &session,
            &output_state,
            Uuid::now_v7(),
        )
        .await;

        assert!(status.is_none());
        assert!(session.process_exit_sealed());
        assert!(
            !session.can_emit_browser_ready(),
            "a failed child wait must seal browser readiness before finalization"
        );
        let state = output_state.lock().await;
        assert!(state.relay_failed);
        assert_eq!(state.close_code, WS_CLOSE_INTERNAL_ERROR);
        assert_eq!(state.close_reason, "SSH process wait failed");
        assert_eq!(state.close_reason_source, "child_wait_error");
    }

    #[tokio::test]
    async fn child_exit_watcher_channel_failure_is_internal_and_seals_lifecycle() {
        let session = test_ssh_session();
        let output_state = Arc::new(Mutex::new(SshOutputState::default()));
        let (sender, receiver) = oneshot::channel::<std::io::Result<std::process::ExitStatus>>();
        drop(sender);

        let status =
            resolve_ssh_child_exit_result(receiver.await, &session, &output_state, Uuid::now_v7())
                .await;

        assert!(status.is_none());
        assert!(session.process_exit_sealed());
        let state = output_state.lock().await;
        assert!(state.relay_failed);
        assert_eq!(state.close_code, WS_CLOSE_INTERNAL_ERROR);
        assert_eq!(state.close_reason, "SSH process exit watcher failed");
        assert_eq!(state.close_reason_source, "child_exit_watcher_error");
    }

    #[tokio::test]
    async fn control_master_readiness_retries_after_an_initial_probe_miss() {
        let session = test_ssh_session();
        let mut ready_rx = session.ready_receiver();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();

        let temp_dir = tempfile::tempdir().expect("temporary control socket directory");
        let control_path = temp_dir.path().join("control.sock");
        let delayed_listener = Arc::new(Mutex::new(None::<tokio::net::UnixListener>));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let readiness_task = tokio::spawn(run_ssh_control_master_readiness_with_probe(
            SshReadinessContext {
                connection_id: Uuid::now_v7(),
                runtime_session: session.clone(),
                metrics: metrics.clone(),
                started_at: Instant::now(),
                known_hosts: temp_dir.path().join("known_hosts"),
                seed_lines: Vec::new(),
            },
            {
                let control_path = control_path.clone();
                let delayed_listener = delayed_listener.clone();
                let probe_calls = probe_calls.clone();
                move || {
                    let control_path = control_path.clone();
                    let delayed_listener = delayed_listener.clone();
                    let probe_calls = probe_calls.clone();
                    async move {
                        if probe_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            *delayed_listener.lock().await = Some(
                                tokio::net::UnixListener::bind(&control_path)
                                    .expect("create ControlMaster socket after initial miss"),
                            );
                            false
                        } else {
                            UnixStream::connect(&control_path).await.is_ok()
                        }
                    }
                }
            },
        ));

        timeout(Duration::from_secs(1), ready_rx.changed())
            .await
            .expect("a delayed ControlMaster must be probed again")
            .expect("ready sender remains alive");
        readiness_task
            .await
            .expect("readiness task completes after ready");

        assert_eq!(
            probe_calls.load(Ordering::SeqCst),
            2,
            "the immediate first interval tick may miss, but must not stop the watcher"
        );
        assert!(session.is_ready());
        assert!(session.can_emit_browser_ready());
        assert_eq!(
            metrics
                .read()
                .await
                .as_response(Uuid::now_v7())
                .overall_status,
            "connected"
        );
    }

    #[tokio::test]
    async fn child_exit_seal_rejects_a_pre_exit_control_probe_before_ready_transition() {
        let session = test_ssh_session();
        let browser_ready_rx = session.ready_receiver();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();

        let child = Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("spawn child held alive until the pre-exit probe completes");
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let child_exit_rx =
            spawn_ssh_child_exit_watcher(child, session.clone(), metrics.clone(), shutdown_rx);

        let temp_dir = tempfile::tempdir().expect("temporary control socket directory");
        let control_path = temp_dir.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&control_path)
            .expect("bind simulated ControlMaster socket");
        let accepted_probe = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept control probe");
            drop(stream);
        });
        let control_master_probe = UnixStream::connect(&control_path).await.is_ok();
        accepted_probe.await.expect("control probe accept task");
        assert!(
            control_master_probe,
            "the ControlMaster probe succeeded while the child was still alive"
        );
        assert!(
            tokio::fs::metadata(&control_path).await.is_ok(),
            "closing the listener leaves the successful probe's socket pathname stale"
        );
        // No assertion is made that a second connect to the stale pathname fails:
        // this test binary is one process, and a concurrently running test's
        // forked child inherits a duplicate of the listening descriptor until
        // execve closes it via SOCK_CLOEXEC, so the connect could succeed by
        // accident. The seal, not the kernel, is what this test verifies.
        shutdown_tx
            .send(())
            .expect("request child exit after the successful control probe");
        timeout(Duration::from_secs(1), child_exit_rx)
            .await
            .expect("child exit watcher observes process completion")
            .expect("child exit watcher remains available")
            .expect("child wait succeeds");

        assert!(
            session.process_exit_sealed(),
            "the actual child exit is published before terminal classification"
        );
        assert!(
            !try_mark_ssh_session_ready_after_control_master_probe(
                &session,
                &metrics,
                Instant::now(),
                control_master_probe,
            )
            .await,
            "a successful probe observed before exit cannot revive the stale session"
        );
        assert!(
            session.terminal_state().is_none(),
            "this models the delay between child exit and terminal publication"
        );
        assert!(
            !session.is_ready(),
            "the exit seal must reject readiness before terminal classification finishes"
        );
        assert!(
            !session.can_emit_browser_ready(),
            "the attach boundary must not emit a browser-ready frame after child exit"
        );
        assert!(
            !browser_ready_rx
                .has_changed()
                .expect("ready sender remains alive"),
            "the browser-facing ready signal must stay silent"
        );
        assert_eq!(
            metrics
                .read()
                .await
                .as_response(Uuid::now_v7())
                .overall_status,
            "error",
            "a child-exit race must fail closed before terminal classification"
        );

        session
            .mark_ended(WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out")
            .await;
        assert!(session.terminal_state().is_some());
        assert!(!session.is_ready());
    }

    #[tokio::test]
    async fn authentication_marker_prevents_removed_socket_output_from_becoming_auth_failure() {
        let material = create_ssh_session_material()
            .await
            .expect("generation material");
        let listener = std::os::unix::net::UnixListener::bind(&material.control_path)
            .expect("bind simulated authenticated ControlMaster socket");
        drop(listener);
        tokio::fs::remove_file(&material.control_path)
            .await
            .expect("remove short-lived ControlMaster socket before a poll can observe it");
        tokio::fs::write(&material.authentication_marker, b"")
            .await
            .expect("simulate post-connect LocalCommand authentication marker");

        let session = test_ssh_session_with_control_path(material.control_path.clone());
        session.seal_process_exit();

        assert!(!session.is_ready());
        assert!(session.process_exit_sealed());
        let authenticated_handshake =
            ssh_session_authenticated_handshake(&session, &material).await;
        assert!(
            authenticated_handshake,
            "a post-connect marker survives a socket removed between readiness polls"
        );
        assert_eq!(
            classify_ssh_stderr_message("Permission denied"),
            Some((WS_CLOSE_INVALID_PAYLOAD, "SSH authentication failed")),
            "the simulated remote output intentionally looks like an auth error"
        );
        assert_eq!(
            resolve_ssh_exit_before_or_after_readiness(
                authenticated_handshake,
                Some(1),
                WS_CLOSE_NORMAL,
            ),
            None,
            "a forced remote command may emit an auth-looking phrase and exit nonzero after real authentication"
        );

        cleanup_ssh_session_material(&material).await;
    }

    #[tokio::test]
    async fn child_exit_seal_wins_over_browser_ready_enqueue_after_sink_writability() {
        let session = test_ssh_session();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();
        assert!(
            session.mark_ready_and_connected(&metrics, Some(1)).await,
            "the ControlMaster transition completed before child exit"
        );

        let child = Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("spawn child held alive until the ready-to-exit ordering is set");
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let child_exit_rx =
            spawn_ssh_child_exit_watcher(child, session.clone(), metrics.clone(), shutdown_rx);

        let (sink_ready_tx, sink_ready_rx) = oneshot::channel();
        let (release_sink_tx, release_sink_rx) = oneshot::channel();
        let enqueued_messages = Arc::new(StdMutex::new(0usize));
        let ready_send_session = session.clone();
        let ready_send_task = tokio::spawn({
            let enqueued_messages = enqueued_messages.clone();
            async move {
                let mut sink = ReadyFrameGateTestSink {
                    ready_entered_tx: Some(sink_ready_tx),
                    release_ready_rx: release_sink_rx,
                    enqueued_messages,
                };
                enqueue_browser_ready_frame(&ready_send_session, &mut sink).await
            }
        });
        timeout(Duration::from_secs(1), sink_ready_rx)
            .await
            .expect("browser-ready send reaches the sink-writability boundary")
            .expect("test sink reports the boundary");

        shutdown_tx
            .send(())
            .expect("request child exit before the attach loop sends ready");
        timeout(Duration::from_secs(1), child_exit_rx)
            .await
            .expect("child exit watcher observes process completion")
            .expect("child exit watcher remains available")
            .expect("child wait succeeds");
        release_sink_tx
            .send(())
            .expect("release the ready enqueue after exit seal");
        assert!(
            !ready_send_task
                .await
                .expect("browser-ready enqueue task")
                .expect("test sink stays writable"),
            "the ready frame must be suppressed when exit wins before start_send"
        );

        assert!(
            session.is_ready(),
            "the historical handshake remains recorded for final close classification"
        );
        assert!(session.process_exit_sealed());
        assert!(
            !session.can_emit_browser_ready(),
            "an attach that has not emitted ready yet must observe the exit seal"
        );
        assert_eq!(
            *enqueued_messages
                .lock()
                .expect("test sink message counter lock"),
            0,
            "no browser-ready frame reaches start_send after the exit seal"
        );
        assert_eq!(
            metrics
                .read()
                .await
                .as_response(Uuid::now_v7())
                .overall_status,
            "error",
            "the exit seal removes the stale healthy state before final classification"
        );

        session
            .mark_ended(WS_CLOSE_NORMAL, "SSH session ended")
            .await;
    }

    #[tokio::test]
    async fn browser_ready_enqueue_defers_process_exit_seal_after_sending_before_start_send() {
        let session = test_ssh_session();
        assert!(
            session.mark_ready().await,
            "the session is ready before the deterministic enqueue race"
        );

        let (release_sink_tx, release_sink_rx) = oneshot::channel();
        release_sink_tx
            .send(())
            .expect("make the recording sink writable immediately");
        let enqueued_messages = Arc::new(StdMutex::new(0usize));
        let mut sink = ReadyFrameGateTestSink {
            ready_entered_tx: None,
            release_ready_rx: release_sink_rx,
            enqueued_messages: enqueued_messages.clone(),
        };
        let session_for_assertion = session.clone();
        let (exit_gate_attempt_tx, exit_gate_attempt_rx) = std::sync::mpsc::channel();
        let exit_thread: Arc<StdMutex<Option<std::thread::JoinHandle<()>>>> =
            Arc::new(StdMutex::new(None));
        let exit_thread_for_send = exit_thread.clone();

        assert!(
            enqueue_browser_ready_frame_after_claim(&session, &mut sink, move || {
                assert_eq!(
                    session_for_assertion
                        .browser_ready_enqueue_state
                        .load(AtomicOrdering::Acquire),
                    BROWSER_READY_ENQUEUE_SENDING,
                    "the real exit seal starts after the final ready CAS"
                );
                session_for_assertion.arm_browser_ready_gate_waiter(exit_gate_attempt_tx);
                let exit_session = session_for_assertion.clone();
                let handle = std::thread::spawn(move || {
                    exit_session.seal_process_exit();
                });
                *exit_thread_for_send
                    .lock()
                    .expect("test exit thread handle lock") = Some(handle);
                exit_gate_attempt_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("the real exit seal attempts the shared ready gate");
                assert!(
                    !session_for_assertion.process_exit_sealed(),
                    "the real exit seal must wait until start_send releases the shared gate"
                );
            })
            .await
            .expect("recording sink stays writable"),
            "start_send owns the shared linearization boundary before the exit seal"
        );
        exit_thread
            .lock()
            .expect("test exit thread handle lock")
            .take()
            .expect("the real exit seal thread is spawned")
            .join()
            .expect("the real exit seal thread joins");
        assert!(session.process_exit_sealed());
        assert_eq!(
            *enqueued_messages
                .lock()
                .expect("test sink message counter lock"),
            1,
            "the ready frame is enqueued before the real exit seal publishes"
        );
    }

    #[tokio::test]
    async fn process_exit_seal_waits_for_a_claimed_ready_enqueue_boundary() {
        let session = test_ssh_session();
        assert!(
            session.mark_ready().await,
            "the session is ready before the deterministic exit interleaving"
        );
        let _ready_claim = session
            .claim_browser_ready_enqueue()
            .expect("the ready enqueue claim is open before child exit");
        let (exit_gate_attempt_tx, exit_gate_attempt_rx) = std::sync::mpsc::channel();
        session.arm_browser_ready_gate_waiter(exit_gate_attempt_tx);
        let exit_session = session.clone();
        let exit_thread = std::thread::spawn(move || {
            exit_session.seal_process_exit();
        });

        assert_eq!(
            session
                .browser_ready_enqueue_state
                .load(AtomicOrdering::Acquire),
            BROWSER_READY_ENQUEUE_CLAIMED,
            "the ready claim remains intact until its synchronous boundary completes"
        );
        exit_gate_attempt_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the real exit seal attempts the gate while the ready claim owns it");
        assert!(
            !session.process_exit_sealed(),
            "the same-thread claim test must not deadlock; the real exit seal waits instead"
        );

        session.release_browser_ready_enqueue_claim();
        drop(_ready_claim);
        exit_thread
            .join()
            .expect("the real exit seal thread joins after claim release");
        assert!(session.process_exit_sealed());
        assert!(session.browser_ready_enqueue_ended());
    }

    #[tokio::test]
    async fn browser_ready_enqueue_reopens_after_a_successful_send_for_reattach() {
        let session = test_ssh_session();
        assert!(session.mark_ready().await);
        let enqueued_messages = Arc::new(StdMutex::new(0usize));
        let make_writable_sink = |enqueued_messages: Arc<StdMutex<usize>>| {
            let (release_sink_tx, release_sink_rx) = oneshot::channel();
            release_sink_tx
                .send(())
                .expect("make recording sink writable immediately");
            ReadyFrameGateTestSink {
                ready_entered_tx: None,
                release_ready_rx: release_sink_rx,
                enqueued_messages,
            }
        };

        let mut initial_sink = make_writable_sink(enqueued_messages.clone());
        assert!(
            enqueue_browser_ready_frame(&session, &mut initial_sink)
                .await
                .expect("initial recording sink stays writable")
        );
        assert!(
            session.can_emit_browser_ready(),
            "a completed ready send must reopen the lifecycle for reattach"
        );

        let mut reattach_sink = make_writable_sink(enqueued_messages.clone());
        assert!(
            enqueue_browser_ready_frame(&session, &mut reattach_sink)
                .await
                .expect("reattach recording sink stays writable")
        );
        assert_eq!(
            *enqueued_messages
                .lock()
                .expect("test sink message counter lock"),
            2,
            "each serial attach receives its own ready frame while the session is live"
        );
    }

    #[tokio::test]
    async fn reaper_removes_only_sessions_past_grace() {
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));

        // Detached past grace: should be reaped.
        let expired_id = Uuid::now_v7();
        let (expired_session, mut expired_shutdown_rx) = test_ssh_session_with_receiver();
        // Still attached (no detach timestamp): must survive.
        let attached_id = Uuid::now_v7();
        let attached_session = test_ssh_session();
        // Detached but within grace: must survive.
        let fresh_id = Uuid::now_v7();
        let fresh_session = test_ssh_session();

        for id in [expired_id, attached_id, fresh_id] {
            state.connections.write().await.insert(
                id,
                super::super::ProxyConnection {
                    payload: json!({"type": "ssh"}),
                    metrics: Arc::new(RwLock::new(ConnectionMetricsState::default())),
                },
            );
        }
        {
            let mut sessions = state.ssh_sessions.write().await;
            sessions.insert(expired_id, expired_session.clone());
            sessions.insert(attached_id, attached_session.clone());
            sessions.insert(fresh_id, fresh_session.clone());
        }

        // expired_session: detached_at set ~120 s in the past — exceeds the 60 s grace.
        // fresh_session: detached_at set to now — within the 60 s grace, must survive.
        *expired_session.detached_at.lock().await = Some(Instant::now() - Duration::from_secs(120));
        fresh_session.mark_detached().await;

        let reaped = reap_detached_ssh_sessions(&state, Duration::from_secs(60)).await;
        assert_eq!(reaped, 1, "exactly the over-grace session is reaped");

        let sessions = state.ssh_sessions.read().await;
        assert!(
            !sessions.contains_key(&expired_id),
            "expired session removed"
        );
        assert!(
            sessions.contains_key(&attached_id),
            "attached session survives"
        );
        assert!(
            sessions.contains_key(&fresh_id),
            "within-grace session survives"
        );
        drop(sessions);

        // The reaped session was shut down.
        assert!(matches!(
            timeout(Duration::from_secs(1), expired_shutdown_rx.recv()).await,
            Ok(Some(SshRuntimeInput::Shutdown))
        ));
    }

    #[tokio::test]
    async fn reaper_skips_session_reattached_before_removal() {
        // Verifies the TOCTOU guard: a session that was expired at scan time
        // but re-attached (detached_at cleared) before removal must NOT be reaped.
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let id = Uuid::now_v7();
        let session = test_ssh_session();

        state.connections.write().await.insert(
            id,
            super::super::ProxyConnection {
                payload: json!({"type": "ssh"}),
                metrics: Arc::new(RwLock::new(ConnectionMetricsState::default())),
            },
        );
        state.ssh_sessions.write().await.insert(id, session.clone());

        // Mark detached well past grace.
        *session.detached_at.lock().await = Some(Instant::now() - Duration::from_secs(120));

        // Simulate re-attach clearing the timer before the reaper's removal step.
        session.mark_attached().await;

        let reaped = reap_detached_ssh_sessions(&state, Duration::from_secs(60)).await;
        assert_eq!(reaped, 0, "re-attached session must not be reaped");
        assert!(
            state.ssh_sessions.read().await.contains_key(&id),
            "session must still be registered"
        );
    }

    #[tokio::test]
    async fn get_or_start_ssh_session_serializes_concurrent_starts() {
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let connection_id = Uuid::now_v7();
        state.connections.write().await.insert(
            connection_id,
            super::super::ProxyConnection {
                payload: json!({"type": "ssh"}),
                metrics: Arc::new(RwLock::new(ConnectionMetricsState::default())),
            },
        );
        let start_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(8));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let state = state.clone();
            let start_count = start_count.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                get_or_start_ssh_session(state, connection_id, move || {
                    let start_count = start_count.clone();
                    async move {
                        start_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        Ok(test_ssh_session())
                    }
                })
                .await
                .expect("session")
            }));
        }

        let mut sessions = Vec::new();
        for handle in handles {
            sessions.push(handle.await.expect("join"));
        }
        let first = sessions.first().expect("at least one session");
        assert!(sessions.iter().all(|session| Arc::ptr_eq(session, first)));
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cached_ready_agent_session_preserves_connected_metrics_on_reacquire() {
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let connection_id = Uuid::now_v7();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        let session = test_ssh_session();
        assert!(
            session.mark_ready().await,
            "test session must become ready once"
        );
        mark_connection_metrics_connected(&metrics, Some(37)).await;
        state.connections.write().await.insert(
            connection_id,
            super::super::ProxyConnection {
                payload: json!({
                    "connection_type": "ssh",
                    "host": "127.0.0.1",
                    "credentials": { "username": "operator" }
                }),
                metrics: metrics.clone(),
            },
        );
        state
            .ssh_sessions
            .write()
            .await
            .insert(connection_id, session.clone());

        let reacquired = agent_ssh_session(state, connection_id)
            .await
            .expect("cached SSH session");

        assert!(Arc::ptr_eq(&reacquired, &session));
        let response = metrics.read().await.as_response(connection_id);
        assert_eq!(response.overall_status, "connected");
        assert!(response.links.iter().all(|link| link.status == "healthy"));
        assert_eq!(
            response
                .links
                .iter()
                .find(|link| link.link_type == "proxy_protocol")
                .and_then(|link| link.latency_ms),
            Some(37),
            "re-acquiring a ready session must retain its measured latency"
        );
    }

    #[tokio::test]
    async fn active_ssh_control_path_releases_only_a_ready_live_sidecar_session() {
        fn launch_sidecar_if_permitted(
            control_path: Result<String, AppError>,
            launches: &AtomicUsize,
        ) -> Result<String, AppError> {
            control_path.inspect(|_| {
                // This is the seam at which a file sidecar would spawn
                // ssh/scp. A rejected lifecycle must never reach it with a
                // missing control path that could trigger a direct fallback.
                launches.fetch_add(1, Ordering::SeqCst);
            })
        }

        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let connection_id = Uuid::now_v7();
        let control_path = format!("/tmp/piping-sidecar-{connection_id}");
        let session = test_ssh_session_with_control_path(control_path.clone());
        state
            .ssh_sessions
            .write()
            .await
            .insert(connection_id, session.clone());
        let launches = AtomicUsize::new(0);

        let pending = launch_sidecar_if_permitted(
            active_ssh_control_path(&state, connection_id).await,
            &launches,
        );
        assert!(matches!(
            pending,
            Err(AppError::Conflict(ref reason))
                if reason == "SSH ControlMaster session is not ready"
        ));
        assert_eq!(
            launches.load(Ordering::SeqCst),
            0,
            "a pending handshake must not launch a direct sidecar"
        );

        assert!(session.mark_ready().await, "test session becomes ready");
        let ready = launch_sidecar_if_permitted(
            active_ssh_control_path(&state, connection_id).await,
            &launches,
        )
        .expect("ready live session releases its ControlMaster path");
        assert_eq!(ready, control_path);
        assert_eq!(launches.load(Ordering::SeqCst), 1);

        session.seal_process_exit();
        assert!(
            session.is_ready(),
            "exit sealing deliberately retains historical handshake readiness"
        );
        let sealed = launch_sidecar_if_permitted(
            active_ssh_control_path(&state, connection_id).await,
            &launches,
        );
        assert!(matches!(
            sealed,
            Err(AppError::Conflict(ref reason))
                if reason == "SSH ControlMaster session is ending"
        ));
        assert_eq!(
            launches.load(Ordering::SeqCst),
            1,
            "a sealed process must not launch a sidecar against stale material"
        );

        let terminal = test_ssh_session_with_control_path(format!("{control_path}-terminal"));
        assert!(
            terminal.mark_ready().await,
            "terminal fixture becomes ready"
        );
        terminal
            .mark_ended(WS_CLOSE_NORMAL, "SSH session ended")
            .await;
        state
            .ssh_sessions
            .write()
            .await
            .insert(connection_id, terminal);

        let ended = launch_sidecar_if_permitted(
            active_ssh_control_path(&state, connection_id).await,
            &launches,
        );
        assert!(matches!(
            ended,
            Err(AppError::Conflict(ref reason))
                if reason == "SSH ControlMaster session has ended"
        ));
        assert_eq!(
            launches.load(Ordering::SeqCst),
            1,
            "a terminal session must not launch a sidecar through stale keys"
        );
    }

    #[tokio::test]
    async fn sidecar_exec_rejects_a_missing_control_master_before_spawn() {
        let result = run_ssh_exec_runtime(
            Uuid::now_v7(),
            &json!({
                "connection_type": "ssh",
                "host": TEST_SSH_HOST,
                "port": TEST_SSH_PORT,
                "credentials": { "username": "operator" }
            }),
            Arc::new(RwLock::new(ConnectionMetricsState::default())),
            AgentSshExecRequest {
                stdin: None,
                command: "true".to_string(),
                timeout: Duration::from_secs(1),
                max_bytes_per_stream: 1,
            },
            SshProcessMetricsMode::Sidecar,
            None,
        )
        .await;

        assert!(matches!(
            result,
            Err(AppError::Internal(ref reason))
                if reason == "SSH sidecar requires an authenticated ControlMaster session"
        ));
    }

    #[tokio::test]
    async fn remove_connection_waits_for_initializing_ssh_session_and_shutdowns_it() {
        let state = Arc::new(ProxyState::new(&ProxyConfig::default()).expect("proxy state"));
        let connection_id = Uuid::now_v7();
        state.connections.write().await.insert(
            connection_id,
            super::super::ProxyConnection {
                payload: json!({"type": "ssh"}),
                metrics: Arc::new(RwLock::new(ConnectionMetricsState::default())),
            },
        );
        let (create_started_tx, create_started_rx) = oneshot::channel();
        let (finish_create_tx, finish_create_rx) = oneshot::channel();
        let shutdown_rx_slot = Arc::new(Mutex::new(None));
        let shutdown_rx_slot_for_create = shutdown_rx_slot.clone();

        let start_state = state.clone();
        let start_handle = tokio::spawn(async move {
            get_or_start_ssh_session(start_state, connection_id, move || async move {
                let _ = create_started_tx.send(());
                let _ = finish_create_rx.await;
                let (session, shutdown_rx) = test_ssh_session_with_receiver();
                *shutdown_rx_slot_for_create.lock().await = Some(shutdown_rx);
                Ok(session)
            })
            .await
        });

        create_started_rx.await.expect("create started");
        let delete_state = state.clone();
        let delete_handle = tokio::spawn(async move {
            super::super::router::remove_connection_state(delete_state, connection_id).await
        });

        finish_create_tx.send(()).expect("finish create");
        let _session = start_handle.await.expect("join").expect("session");
        assert!(delete_handle.await.expect("delete join"));

        let mut shutdown_rx = shutdown_rx_slot
            .lock()
            .await
            .take()
            .expect("shutdown receiver");
        assert!(matches!(
            timeout(Duration::from_secs(1), shutdown_rx.recv()).await,
            Ok(Some(SshRuntimeInput::Shutdown))
        ));
        assert!(!state.connections.read().await.contains_key(&connection_id));
        assert!(!state.ssh_sessions.read().await.contains_key(&connection_id));
    }

    #[tokio::test]
    async fn start_ssh_session_runtime_removes_temp_keys_on_shutdown_after_start() {
        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": "127.0.0.1",
            "port": 1,
            "credentials": {
                "username": "operator",
                "private_key": "-----BEGIN TEST KEY-----\nabc\n-----END TEST KEY-----"
            },
            "timeout": 1
        });
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));

        let session = start_ssh_session_runtime(connection_id, &payload, metrics)
            .await
            .expect("session starts without a separate reachability probe");
        let mut terminal_rx = session.terminal_receiver();
        let material_dir = PathBuf::from(
            session
                .control_path()
                .expect("interactive session has a generation-scoped control path"),
        )
        .parent()
        .expect("control socket has a material parent")
        .to_path_buf();

        session
            .input_tx
            .send(SshRuntimeInput::Shutdown)
            .await
            .expect("shutdown send");
        timeout(Duration::from_secs(3), terminal_rx.changed())
            .await
            .expect("ssh process should stop promptly")
            .expect("terminal state should update");
        assert!(
            !tokio::fs::try_exists(&material_dir)
                .await
                .expect("check generation material dir")
        );
    }

    #[tokio::test]
    async fn spawned_ssh_without_a_handshake_keeps_target_metrics_connecting() {
        // Accept the TCP connection but deliberately never send an SSH banner.
        // This proves that process spawn (and even TCP acceptance) is not an
        // established SSH session.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let listener_task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("ssh client should connect");
            let _ = accepted_tx.send(());
            let _ = release_rx.await;
        });

        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": "127.0.0.1",
            "port": port,
            "credentials": { "username": "operator" },
            "timeout": 1
        });
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();

        let session = start_ssh_session_runtime(connection_id, &payload, metrics.clone())
            .await
            .expect("ssh process should spawn while the handshake is pending");
        timeout(Duration::from_secs(2), accepted_rx)
            .await
            .expect("ssh process should reach the TCP listener")
            .expect("listener should observe the SSH TCP connection");

        let response = metrics.read().await.as_response(connection_id);
        assert_eq!(response.overall_status, "connecting");
        assert!(
            response
                .links
                .iter()
                .all(|link| link.status == "connecting"),
            "a spawned SSH process without a handshake must not report a healthy link: {response:?}"
        );

        let mut terminal_rx = session.terminal_receiver();
        session.shutdown().await;
        timeout(Duration::from_secs(3), terminal_rx.changed())
            .await
            .expect("ssh process should stop promptly")
            .expect("terminal state should update");
        let _ = release_tx.send(());
        listener_task.await.expect("listener task should finish");
    }

    #[tokio::test]
    async fn pre_auth_pty_output_cannot_mark_ssh_metrics_healthy() {
        let session = test_ssh_session();
        let connection_id = Uuid::now_v7();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();

        // An SSH banner/prompt is bytes from a spawned client process, not
        // authenticated target traffic. This must remain true even before a
        // browser attaches and no watcher receiver exists.
        record_ssh_pty_output_traffic_if_ready(&session, &metrics, 17).await;

        let response = metrics.read().await.as_response(connection_id);
        assert_eq!(response.overall_status, "connecting");
        assert!(
            response
                .links
                .iter()
                .all(|link| link.status == "connecting")
        );
        assert!(response.links.iter().all(|link| link.rx_bytes == Some(0)));
    }

    #[tokio::test]
    async fn start_ssh_session_runtime_rejects_timeout_validation_before_spawn() {
        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": "127.0.0.1",
            "port": TEST_SSH_PORT,
            "credentials": {
                "username": "operator",
                "private_key": "-----BEGIN TEST KEY-----\nabc\n-----END TEST KEY-----"
            },
            "timeout": "invalid"
        });
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));

        let result = start_ssh_session_runtime(connection_id, &payload, metrics).await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    fn test_ssh_session() -> Arc<SshSession> {
        test_ssh_session_with_receiver().0
    }

    fn test_ssh_session_with_control_path(control_path: String) -> Arc<SshSession> {
        test_ssh_session_with_receiver_for_control_path(Some(control_path)).0
    }

    fn test_ssh_session_with_receiver() -> (Arc<SshSession>, mpsc::Receiver<SshRuntimeInput>) {
        test_ssh_session_with_receiver_for_control_path(None)
    }

    fn test_ssh_session_with_receiver_for_control_path(
        control_path: Option<String>,
    ) -> (Arc<SshSession>, mpsc::Receiver<SshRuntimeInput>) {
        let (input_tx, input_rx) = mpsc::channel::<SshRuntimeInput>(1);
        let (output_tx, _) = broadcast::channel::<SshHistoryChunk>(1);
        let (terminal_tx, _) = watch::channel(None);
        let (ready_tx, _) = watch::channel(false);
        let (process_exit_tx, _) = watch::channel(false);
        (
            Arc::new(SshSession {
                input_tx,
                output_tx,
                history: RwLock::new(SshHistory::new()),
                attach_lock: Mutex::new(()),
                control_path,
                detached_at: Mutex::new(None),
                terminal_tx,
                ready_tx,
                process_exit_tx,
                process_exit_seal: AtomicBool::new(false),
                browser_ready_gate: StdMutex::new(()),
                #[cfg(test)]
                browser_ready_gate_waiter: StdMutex::new(None),
                browser_ready_enqueue_state: AtomicU8::new(BROWSER_READY_ENQUEUE_OPEN),
                lifecycle_transition: Mutex::new(()),
            }),
            input_rx,
        )
    }

    #[test]
    fn parse_resize_control_message_with_dimensions() {
        let payload = format!(
            "{WS_CONTROL_PREFIX}{{\"type\":\"resize\",\"cols\":{TEST_RESIZE_COLS},\"rows\":{TEST_RESIZE_ROWS}}}"
        );
        let control = parse_ws_control_message(&payload);
        assert!(
            control.is_some(),
            "control message should parse: {control:?}"
        );
        assert_eq!(
            control
                .as_ref()
                .and_then(WsControlMessage::resize_dimensions),
            Some((TEST_RESIZE_COLS, TEST_RESIZE_ROWS))
        );
    }

    #[test]
    fn parse_resize_control_message_rejects_zero_dimensions() {
        let payload = format!(
            "{WS_CONTROL_PREFIX}{{\"type\":\"resize\",\"cols\":0,\"rows\":{TEST_RESIZE_ROWS}}}"
        );
        let control = parse_ws_control_message(&payload);
        assert!(
            control.is_some(),
            "control message should parse: {control:?}"
        );
        assert_eq!(
            control
                .as_ref()
                .and_then(WsControlMessage::resize_dimensions),
            None
        );
    }

    #[test]
    fn parse_control_message_ignores_non_prefixed_payloads() {
        assert!(
            parse_ws_control_message("{\"type\":\"resize\",\"cols\":120,\"rows\":42}").is_none()
        );
    }

    #[test]
    fn normalize_ssh_private_key_rewrites_crlf_and_appends_newline() {
        let input =
            "  -----BEGIN OPENSSH PRIVATE KEY-----\r\nabc\r\n-----END OPENSSH PRIVATE KEY-----  ";
        assert_eq!(
            normalize_ssh_private_key(input),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n"
        );
    }

    #[test]
    fn ssh_history_snapshot_since_returns_cursor_limited_tail() {
        let mut history = SshHistory::new();
        history.push(b"one".to_vec());
        history.push(b"two".to_vec());
        history.push(b"three".to_vec());

        let snapshot = history.snapshot_since(Some(0), 16);
        assert_eq!(snapshot.next_seq, 3);
        assert_eq!(snapshot.history_start_seq, Some(0));
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.chunks.len(), 2);
        assert_eq!(snapshot.chunks[0].seq, 1);
        assert_eq!(snapshot.chunks[0].bytes, b"two");

        let tail = history.snapshot_since(None, 4);
        assert!(tail.truncated);
        assert_eq!(tail.chunks.len(), 1);
        assert_eq!(tail.chunks[0].seq, 2);
        assert_eq!(tail.chunks[0].bytes, b"three");
    }

    #[test]
    fn history_replay_drops_evicted_live_prefix_before_replayed_tail() {
        let mut history = SshHistory::new();
        let evicted_pre_snapshot = history.push(vec![0; SSH_HISTORY_LIMIT_BYTES + 1]);
        let replayed_tail_first = history.push(b"tail-first".to_vec());
        let replayed_tail_last = history.push(b"tail-last".to_vec());
        let replayed_history_end = history.snapshot().last().map(|chunk| chunk.seq);
        let post_snapshot = history.push(b"after-snapshot".to_vec());

        assert!(
            ssh_output_precedes_or_was_replayed_from_history(
                evicted_pre_snapshot.seq,
                replayed_history_end,
            ),
            "the receiver's stale prefix must not follow the replayed tail"
        );
        assert!(ssh_output_precedes_or_was_replayed_from_history(
            replayed_tail_first.seq,
            replayed_history_end,
        ));
        assert!(ssh_output_precedes_or_was_replayed_from_history(
            replayed_tail_last.seq,
            replayed_history_end,
        ));
        assert!(
            !ssh_output_precedes_or_was_replayed_from_history(
                post_snapshot.seq,
                replayed_history_end,
            ),
            "chunks produced after the snapshot remain live output"
        );
        assert!(!ssh_output_precedes_or_was_replayed_from_history(0, None));
    }

    #[test]
    fn extract_ssh_launch_config_reads_nested_username() {
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": {
                "username": "operator",
                "password": "secret"
            }
        });

        let launch = extract_ssh_launch_config(&payload);
        assert!(launch.is_ok(), "launch config should be valid: {launch:?}");
        assert_eq!(
            launch.as_ref().ok().map(|value| value.username.as_str()),
            Some("operator")
        );
    }

    #[test]
    fn validate_ssh_host_accepts_ip_rejects_hostname_and_options() {
        assert_eq!(validate_ssh_host("10.0.0.5").as_deref(), Ok("10.0.0.5"));
        assert_eq!(
            validate_ssh_host(" [2001:db8::1] ").as_deref(),
            Ok("2001:db8::1")
        );
        assert_eq!(validate_ssh_host("10.0.0.5/24").as_deref(), Ok("10.0.0.5"));
        assert!(validate_ssh_host("evil.example.com").is_err());
        assert!(validate_ssh_host("-oProxyCommand=touch /tmp/pwn").is_err());
        assert!(validate_ssh_host("").is_err());
    }

    #[test]
    fn validate_ssh_username_rejects_option_like() {
        assert_eq!(validate_ssh_username(" root ").as_deref(), Ok("root"));
        assert!(validate_ssh_username("-oProxyCommand=x").is_err());
        assert!(validate_ssh_username("   ").is_err());
    }

    #[test]
    fn identities_only_is_added_only_when_private_keys_are_configured() {
        let mut password_command = Command::new("ssh");
        append_identities_only_when_keyed(&mut password_command, false);
        assert!(password_command.as_std().get_args().next().is_none());

        let mut keyed_command = Command::new("ssh");
        append_identities_only_when_keyed(&mut keyed_command, true);
        let args = keyed_command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, vec!["-o", "IdentitiesOnly=yes"]);
    }

    #[test]
    fn interactive_authentication_marker_uses_a_shell_safe_post_connect_command() {
        let marker = Path::new("/tmp/piping ssh/%h'authenticated");
        let local_command = ssh_authentication_marker_local_command(marker);
        assert_eq!(
            local_command,
            "/usr/bin/touch -- '/tmp/piping ssh/%%h'\\''authenticated'"
        );

        let mut command = Command::new("ssh");
        append_ssh_authentication_marker_options(&mut command, marker);
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let shell = command.as_std().get_envs().find_map(|(name, value)| {
            (name.to_string_lossy() == "SHELL")
                .then(|| value.map(|value| value.to_string_lossy().into_owned()))
                .flatten()
        });

        assert_eq!(shell.as_deref(), Some("/bin/sh"));
        assert_eq!(
            args,
            vec![
                "-o".to_string(),
                "PermitLocalCommand=yes".to_string(),
                "-o".to_string(),
                format!("LocalCommand={local_command}"),
            ]
        );
    }

    #[test]
    fn mux_only_sidecar_options_block_socket_loss_direct_connection() {
        let control_path = "/tmp/piping-control-master.sock";
        let fallback_blocker = format!("ProxyCommand={SSH_MUX_ONLY_FALLBACK_BLOCKER}");

        let mut ssh_command = Command::new("ssh");
        append_ssh_control_options(&mut ssh_command, Some(control_path));
        let ssh_args = ssh_command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let ssh_shell = ssh_command.as_std().get_envs().find_map(|(name, value)| {
            if name.to_string_lossy() == "SHELL" {
                value.map(|value| value.to_string_lossy().into_owned())
            } else {
                None
            }
        });
        assert_eq!(ssh_shell.as_deref(), Some("/bin/sh"));
        assert_eq!(
            ssh_args,
            vec![
                "-F".to_string(),
                "/dev/null".to_string(),
                "-S".to_string(),
                control_path.to_string(),
                "-o".to_string(),
                "ControlMaster=no".to_string(),
                "-o".to_string(),
                fallback_blocker.clone(),
                "-o".to_string(),
                "ProxyUseFdpass=no".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
            ],
            "when the control socket vanishes after the lifecycle gate, OpenSSH must fail its direct fallback before opening a target connection"
        );

        let mut scp_command = Command::new("scp");
        append_scp_control_options(&mut scp_command, Some(control_path));
        let scp_control_path = format!("ControlPath={control_path}");
        let scp_args = scp_command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            scp_args,
            vec![
                "-F".to_string(),
                "/dev/null".to_string(),
                "-o".to_string(),
                scp_control_path,
                "-o".to_string(),
                "ControlMaster=no".to_string(),
                "-o".to_string(),
                fallback_blocker,
                "-o".to_string(),
                "ProxyUseFdpass=no".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
            ],
            "scp must use the same mux-only direct-fallback blocker"
        );
    }

    #[tokio::test]
    async fn mux_only_sidecar_does_not_open_tcp_after_control_socket_loss() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();
        let control_dir = tempfile::tempdir().expect("temporary control socket directory");
        let control_path = control_dir.path().join("missing.sock");

        let mut command = Command::new("ssh");
        command
            .arg("-o")
            .arg("ConnectTimeout=1")
            .arg("-p")
            .arg(port.to_string());
        append_ssh_control_options(&mut command, control_path.to_str());
        command
            .arg("127.0.0.1")
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let status = timeout(Duration::from_secs(2), command.status())
            .await
            .expect("socket-loss sidecar should fail promptly")
            .expect("ssh process should run");
        assert!(
            !status.success(),
            "a missing mux socket must fail rather than run the command directly"
        );
        assert!(
            timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err(),
            "the direct-fallback target must not receive a TCP connection"
        );
    }

    #[tokio::test]
    async fn prepared_scp_sidecar_omits_direct_authentication_material() {
        let connection_id = Uuid::now_v7();
        let control_path = format!("/tmp/piping-control-master-{connection_id}.sock");
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": {
                "username": "operator",
                "password": "synthetic-sidecar-password",
                "private_key": "synthetic-sidecar-key"
            },
            "jump": [{
                "host": "10.10.1.231",
                "port": 2222,
                "credentials": {
                    "username": "jump-operator",
                    "private_key": "synthetic-sidecar-jump-key"
                }
            }]
        });

        let prepared = prepare_scp_process(connection_id, &payload, control_path.clone())
            .await
            .expect("mux-only scp command should be prepared without contacting a target");
        let PreparedScpProcess {
            command,
            material_dir,
            material_paths,
            ..
        } = prepared;
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let env_names = command
            .as_std()
            .get_envs()
            .filter_map(|(name, _)| name.to_str())
            .collect::<Vec<_>>();

        assert!(args.contains(&format!("ControlPath={control_path}")));
        assert!(args.contains(&format!("ProxyCommand={SSH_MUX_ONLY_FALLBACK_BLOCKER}")));
        assert!(
            !args.iter().any(|arg| arg == "-J" || arg == "-i"),
            "a sidecar must not receive target or jump credentials for a direct fallback: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("PreferredAuthentications=")),
            "a sidecar must not configure password authentication: {args:?}"
        );
        assert!(
            !env_names.contains(&"PIPING_SSH_ASKPASS_SECRET"),
            "a sidecar must not receive an askpass secret"
        );
        assert!(
            material_paths.is_empty(),
            "mux-only scp must not write target or jump private-key material"
        );

        cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
        assert!(
            !tokio::fs::try_exists(&material_dir)
                .await
                .expect("check mux-only scp material directory")
        );
    }

    #[test]
    fn extract_ssh_jump_chain_rejects_non_ip_jump_host() {
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": { "username": "operator" },
            "jump": [ {
                "host": "bastion.internal",
                "credentials": { "username": "jump" }
            } ]
        });
        let result = extract_ssh_jump_chain(&payload);
        assert!(
            matches!(&result, Err(message) if message.contains("not a valid IP")),
            "non-IP jump host must be rejected: {result:?}"
        );
    }

    #[test]
    fn extract_ssh_jump_chain_accepts_ip_jump_host() {
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": { "username": "operator" },
            "jump": [ {
                "host": "10.0.0.9",
                "port": 2222,
                "credentials": { "username": "jump" }
            } ]
        });
        let hops = extract_ssh_jump_chain(&payload).expect("valid jump chain");
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].host, "10.0.0.9");
        assert_eq!(hops[0].port, 2222);
    }

    #[test]
    fn extract_ssh_jump_chain_rejects_option_like_jump_username() {
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": { "username": "operator" },
            "jump": [ {
                "host": "10.0.0.9",
                "credentials": { "username": "-oProxyCommand=x" }
            } ]
        });
        assert!(extract_ssh_jump_chain(&payload).is_err());
    }

    #[test]
    fn extract_ssh_launch_config_rejects_missing_username() {
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": {
                "password": "secret"
            }
        });

        let result = extract_ssh_launch_config(&payload);
        assert!(
            matches!(&result, Err(message) if message.contains("credentials.username")),
            "unexpected result: {result:?}"
        );
    }

    // -- SSH probe banner tests --

    #[tokio::test]
    async fn probe_rejects_non_ssh_service() {
        // Start a TCP listener that sends an HTTP response instead of an SSH banner.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n").await;
            }
        });

        let result = probe_ssh_target_reachability("127.0.0.1", port, 2).await;
        assert!(result.is_err(), "should reject non-SSH service");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("did not send a valid SSH banner"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn probe_accepts_valid_ssh_banner() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").await;
            }
        });

        let result = probe_ssh_target_reachability("127.0.0.1", port, 2).await;
        assert!(result.is_ok(), "should accept valid SSH banner: {result:?}");
    }

    #[tokio::test]
    async fn probe_detects_immediate_close() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                drop(stream); // close immediately
            }
        });

        let result = probe_ssh_target_reachability("127.0.0.1", port, 2).await;
        assert!(result.is_err(), "should detect immediate close");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("closed connection") || msg.contains("banner"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn probe_detects_unreachable_port() {
        // Use a port that is definitely not listening.
        let result = probe_ssh_target_reachability("127.0.0.1", 1, 1).await;
        assert!(result.is_err(), "should detect unreachable port");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unreachable") || msg.contains("refused"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn probe_detects_banner_timeout() {
        // Start a listener that accepts but never sends anything.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let port = listener
            .local_addr()
            .expect("listener should expose local address")
            .port();

        tokio::spawn(async move {
            if let Ok((_stream, _)) = listener.accept().await {
                // Hold the connection open without sending data.
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });

        let result = probe_ssh_target_reachability("127.0.0.1", port, 1).await;
        assert!(result.is_err(), "should detect banner timeout");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("banner exchange timed out"),
            "unexpected error: {msg}"
        );
    }

    type ProbeFuture = Pin<Box<dyn Future<Output = bool> + Send>>;
    type ScriptedProbe = Box<dyn FnMut() -> ProbeFuture + Send>;

    /// Deterministic probe driver for the reachability-watchdog tests: pops
    /// scripted results front-to-back, falling back to `default_when_empty`
    /// once the script is exhausted (so the watchdog can be steered into a
    /// terminal state without an infinite script). Records how many probes ran.
    fn scripted_probe(
        script: Vec<bool>,
        default_when_empty: bool,
    ) -> (ScriptedProbe, Arc<AtomicUsize>) {
        let queue = Arc::new(Mutex::new(VecDeque::from(script)));
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_probe = calls.clone();
        let probe: ScriptedProbe = Box::new(move || {
            let queue = queue.clone();
            let calls = calls_for_probe.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                queue.lock().await.pop_front().unwrap_or(default_when_empty)
            }) as ProbeFuture
        });
        (probe, calls)
    }

    const TEST_WATCHDOG_INTERVAL: Duration = Duration::from_millis(10);
    const TEST_WATCHDOG_THRESHOLD: u32 = 3;
    const TEST_CONNECTION_ID: Uuid = Uuid::nil();

    #[tokio::test]
    async fn reachability_watchdog_fires_on_threshold_consecutive_failures() {
        let (_cancel_tx, cancel_rx) = watch::channel(None);
        let (probe, calls) = scripted_probe(Vec::new(), false); // always unreachable
        let fired: Arc<Mutex<Option<SshTerminalState>>> = Arc::new(Mutex::new(None));
        let fired_cb = fired.clone();

        run_ssh_reachability_watchdog(
            TEST_CONNECTION_ID,
            cancel_rx,
            probe,
            TEST_WATCHDOG_INTERVAL,
            TEST_WATCHDOG_THRESHOLD,
            move |terminal| {
                let fired_cb = fired_cb.clone();
                async move {
                    *fired_cb.lock().await = Some(terminal);
                }
            },
        )
        .await;

        let terminal = fired
            .lock()
            .await
            .clone()
            .expect("watchdog must fire after threshold failures");
        assert_eq!(terminal.close_code, WS_CLOSE_TARGET_UNAVAILABLE);
        assert_eq!(terminal.close_reason, "Upstream connection lost");
        // It must not fire before the threshold is actually reached.
        assert!(
            calls.load(Ordering::SeqCst) >= TEST_WATCHDOG_THRESHOLD as usize,
            "watchdog fired before threshold consecutive failures"
        );
    }

    #[tokio::test]
    async fn reachability_watchdog_success_resets_failure_counter() {
        // fail, fail, success (reset), then reachable forever: never reaches
        // THRESHOLD=3 CONSECUTIVE failures, so it must never fire.
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let (probe, _calls) = scripted_probe(vec![false, false, true], true);
        let fired: Arc<Mutex<Option<SshTerminalState>>> = Arc::new(Mutex::new(None));
        let fired_cb = fired.clone();

        let handle = tokio::spawn(async move {
            run_ssh_reachability_watchdog(
                TEST_CONNECTION_ID,
                cancel_rx,
                probe,
                TEST_WATCHDOG_INTERVAL,
                TEST_WATCHDOG_THRESHOLD,
                move |terminal| {
                    let fired_cb = fired_cb.clone();
                    async move {
                        *fired_cb.lock().await = Some(terminal);
                    }
                },
            )
            .await;
        });

        // Let many probe intervals elapse, then end the session to stop the task.
        tokio::time::sleep(TEST_WATCHDOG_INTERVAL * 20).await;
        cancel_tx
            .send(Some(SshTerminalState::session_ended()))
            .expect("cancel send");
        handle
            .await
            .expect("watchdog task should complete on cancel");

        assert!(
            fired.lock().await.is_none(),
            "watchdog must NOT fire when a success resets the counter below threshold"
        );
    }

    #[tokio::test]
    async fn reachability_watchdog_cancels_cleanly_without_firing() {
        // Probe always fails, but cancellation arrives before the task can leak
        // or fire spuriously after session end.
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let (probe, _calls) = scripted_probe(Vec::new(), false);
        let fired: Arc<Mutex<Option<SshTerminalState>>> = Arc::new(Mutex::new(None));
        let fired_cb = fired.clone();

        // Cancel essentially immediately, before the first (delayed) probe tick.
        cancel_tx
            .send(Some(SshTerminalState::session_ended()))
            .expect("cancel send");

        let handle = tokio::spawn(async move {
            run_ssh_reachability_watchdog(
                TEST_CONNECTION_ID,
                cancel_rx,
                probe,
                TEST_WATCHDOG_INTERVAL,
                TEST_WATCHDOG_THRESHOLD,
                move |terminal| {
                    let fired_cb = fired_cb.clone();
                    async move {
                        *fired_cb.lock().await = Some(terminal);
                    }
                },
            )
            .await;
        });

        // The task must complete promptly (no leak) without firing.
        timeout(Duration::from_secs(1), handle)
            .await
            .expect("watchdog task must not leak; it should return on cancel")
            .expect("watchdog task should not panic");
        assert!(
            fired.lock().await.is_none(),
            "watchdog must NOT fire when the session was already ended"
        );
    }

    // -- PWA-14 host key pinning tests --

    fn pin_test_material(dir: &str) -> SshSessionMaterial {
        SshSessionMaterial {
            dir: PathBuf::from(dir),
            control_path: format!("{dir}/control.sock"),
            authentication_marker: PathBuf::from(format!("{dir}/{SSH_AUTHENTICATION_MARKER_FILE}")),
        }
    }

    fn pin_test_launch() -> SshLaunchConfig {
        SshLaunchConfig {
            host: TEST_SSH_HOST.to_string(),
            port: TEST_SSH_PORT,
            username: "operator".to_string(),
            password: None,
            private_key: None,
            jump: Vec::new(),
        }
    }

    fn pin_test_key_paths() -> SshSessionKeyPaths {
        SshSessionKeyPaths {
            target_private_key: None,
            jump_private_keys: Vec::new(),
        }
    }

    fn pin_test_args(command: &Command) -> Vec<String> {
        command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[tokio::test]
    async fn interactive_ssh_command_pins_known_hosts_and_never_disables_checking() {
        let material = pin_test_material("/tmp/piping-ssh-pin-test");
        let launch = pin_test_launch();
        let key_paths = pin_test_key_paths();

        let command = build_interactive_ssh_command(&material, &launch, &key_paths, None, 30)
            .await
            .expect("interactive ssh command builds");
        let args = pin_test_args(&command);

        assert!(
            args.contains(&"UserKnownHostsFile=/tmp/piping-ssh-pin-test/known_hosts".to_string()),
            "interactive ssh must pin the job known_hosts file: {args:?}"
        );
        assert!(
            args.contains(&"StrictHostKeyChecking=accept-new".to_string()),
            "interactive ssh must trust new hosts only on first use: {args:?}"
        );
        assert!(
            args.contains(&"GlobalKnownHostsFile=/dev/null".to_string()),
            "interactive ssh must suppress the image global known_hosts: {args:?}"
        );
        assert!(
            args.contains(&"HashKnownHosts=no".to_string()),
            "interactive ssh must keep harvested lines readable: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg == concat!("StrictHostKeyChecking", "=no")),
            "interactive ssh must never disable host key checking: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "UserKnownHostsFile=/dev/null"),
            "interactive ssh must never disable the known_hosts file: {args:?}"
        );
    }

    #[tokio::test]
    async fn scp_command_pins_known_hosts_and_seeds_the_job_file() {
        let connection_id = Uuid::now_v7();
        let control_path = format!("/tmp/piping-control-master-{connection_id}.sock");
        let seed_line = "[10.10.1.230]:22 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKdwFDb7uXDNTayo3mt/sJyW1Hut13OuWSjKUWvi64be";
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": { "username": "operator" },
            "known_hosts_lines": [seed_line],
        });

        let prepared = prepare_scp_process(connection_id, &payload, control_path)
            .await
            .expect("scp command builds without contacting a target");
        let args = pin_test_args(&prepared.command);
        let known_hosts = prepared.material_dir.join("known_hosts");

        assert!(
            args.contains(&format!("UserKnownHostsFile={}", known_hosts.display())),
            "scp must pin the per-operation known_hosts file: {args:?}"
        );
        assert!(
            args.contains(&"StrictHostKeyChecking=accept-new".to_string()),
            "scp must trust new hosts only on first use: {args:?}"
        );
        assert!(
            args.contains(&"GlobalKnownHostsFile=/dev/null".to_string()),
            "scp must suppress the image global known_hosts: {args:?}"
        );
        assert!(
            args.contains(&"HashKnownHosts=no".to_string()),
            "scp must keep harvested lines readable: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg == concat!("StrictHostKeyChecking", "=no")),
            "scp must never disable host key checking: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "UserKnownHostsFile=/dev/null"),
            "scp must never disable the known_hosts file: {args:?}"
        );
        let seeded = tokio::fs::read_to_string(&known_hosts)
            .await
            .expect("the seed file is materialized for every operation");
        assert_eq!(
            seeded.lines().collect::<Vec<_>>(),
            vec![seed_line],
            "the seed file contains exactly the payload lines"
        );

        cleanup_ssh_transfer_material(&prepared.material_dir, &prepared.material_paths).await;
    }

    #[test]
    fn ssh_mismatch_stderr_yields_refusal_with_repair() {
        const ED25519_BLOB: &str =
            "AAAAC3NzaC1lZDI1NTE5AAAAIKdwFDb7uXDNTayo3mt/sJyW1Hut13OuWSjKUWvi64be";
        let seed = vec![format!("[10.10.1.230]:22 ssh-ed25519 {ED25519_BLOB}")];

        let changed = "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
            @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
            @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
            Host key for 10.10.1.230 has changed and you have requested strict checking.";
        let error = classify_ssh_host_key_failure(changed, "10.10.1.230", 22, &seed)
            .expect("a changed host key must be refused");
        let message = error.to_string();
        assert!(message.contains("10.10.1.230:22"), "{message}");
        assert!(
            message.contains(
                "DELETE FROM public.ssh_host_key_pin WHERE host = '10.10.1.230' AND port = 22;"
            ),
            "the refusal must name the executable repair SQL: {message}"
        );
        assert!(message.contains("reconnect"), "{message}");

        let failed = "Host key verification failed.";
        let error = classify_ssh_host_key_failure(failed, "10.10.1.230", 22, &[])
            .expect("a verification failure must be refused");
        assert!(error.to_string().contains("10.10.1.230:22"), "{error}");
        assert!(
            error.to_string().contains(
                "DELETE FROM public.ssh_host_key_pin WHERE host = '10.10.1.230' AND port = 22;"
            ),
            "{error}"
        );
        assert!(error.to_string().contains("reconnect"), "{error}");

        assert!(
            classify_ssh_host_key_failure("Permission denied (publickey).", "10.10.1.230", 22, &[])
                .is_none(),
            "an authentication failure is not a host key refusal"
        );
    }

    #[test]
    fn successful_ssh_exec_stderr_marker_is_not_a_host_key_refusal() {
        const ED25519_BLOB: &str =
            "AAAAC3NzaC1lZDI1NTE5AAAAIKdwFDb7uXDNTayo3mt/sJyW1Hut13OuWSjKUWvi64be";
        let seed = vec![format!("[10.10.1.230]:22 ssh-ed25519 {ED25519_BLOB}")];
        let marker = "Host key verification failed.";

        // The exec path spawns a real ssh child that no test can drive, so the
        // regression drives the exact decision helper the call site uses: a
        // successful process whose remote command printed the marker must stay
        // successful.
        assert!(
            classify_failed_ssh_host_key_failure(false, true, marker, "10.10.1.230", 22, &seed)
                .is_none(),
            "stderr from a successful process is never a host key refusal"
        );

        let error =
            classify_failed_ssh_host_key_failure(false, false, marker, "10.10.1.230", 22, &seed)
                .expect("a failed process carrying the marker is still refused");
        assert!(error.to_string().contains("10.10.1.230:22"), "{error}");
    }

    #[tokio::test]
    async fn pty_reader_keeps_host_key_failure_excerpt_after_earlier_output() {
        let temp_dir = tempfile::tempdir().expect("temporary PTY reader path");
        let path = temp_dir.path().join("pty-output");
        // Fill the first read with benign banner text so the marker only lands
        // in a later chunk: the PTY buffer is `SSH_PTY_BUFFER_SIZE`, so only the
        // marker-aware replacement can surface it from the captured excerpt.
        let mut fixture = vec![b'A'; SSH_PTY_BUFFER_SIZE];
        fixture.extend_from_slice(
            b"@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
              @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
              @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
              Host key verification failed.\n",
        );
        tokio::fs::write(&path, fixture)
            .await
            .expect("write the PTY output fixture");
        let pty_reader = tokio::fs::File::open(&path)
            .await
            .expect("open the PTY output fixture");
        let session = test_ssh_session();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        metrics.write().await.mark_connecting();
        let (output_tx, _) = broadcast::channel(8);
        let output_state = Arc::new(Mutex::new(SshOutputState::default()));

        run_ssh_pty_reader(
            Uuid::now_v7(),
            pty_reader,
            session,
            output_tx,
            metrics.clone(),
            output_state.clone(),
        )
        .await
        .expect("the PTY reader drains the fixture");

        let state = output_state.lock().await;
        let excerpt = state
            .stderr_excerpt
            .as_deref()
            .expect("the reader captures a diagnostic excerpt");
        assert!(
            excerpt.contains(SSH_HOST_KEY_CHANGED_MARKER),
            "a host-key failure after earlier output must replace the excerpt: {excerpt}"
        );
    }

    #[tokio::test]
    async fn websocket_input_forwarding_preserves_data_resize_and_detach_boundaries() {
        use axum::extract::ws::Message;

        let connection_id = Uuid::now_v7();
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        let (input_tx, mut input_rx) = mpsc::channel(8);

        assert!(
            forward_ssh_input_message(
                connection_id,
                1,
                Message::Text("hello".into()),
                &input_tx,
                &metrics,
            )
            .await
        );
        match input_rx.recv().await {
            Some(SshRuntimeInput::Bytes(bytes)) => assert_eq!(bytes, b"hello"),
            _ => panic!("text input must become SSH bytes"),
        }

        let resize = format!("{WS_CONTROL_PREFIX}{{\"type\":\"resize\",\"cols\":120,\"rows\":40}}");
        assert!(
            forward_ssh_input_message(
                connection_id,
                2,
                Message::Text(resize.into()),
                &input_tx,
                &metrics,
            )
            .await
        );
        match input_rx.recv().await {
            Some(SshRuntimeInput::Resize { cols, rows }) => {
                assert_eq!((cols, rows), (120, 40));
            }
            _ => panic!("resize control must stay a control input"),
        }

        assert!(
            forward_ssh_input_message(
                connection_id,
                3,
                Message::Binary(vec![1, 2, 3].into()),
                &input_tx,
                &metrics,
            )
            .await
        );
        match input_rx.recv().await {
            Some(SshRuntimeInput::Bytes(bytes)) => assert_eq!(bytes, vec![1, 2, 3]),
            _ => panic!("binary input must become SSH bytes"),
        }

        let detach = format!("{WS_CONTROL_PREFIX}{{\"type\":\"detach\"}}");
        assert!(
            !forward_ssh_input_message(
                connection_id,
                4,
                Message::Text(detach.into()),
                &input_tx,
                &metrics,
            )
            .await
        );
        assert!(
            !forward_ssh_input_message(
                connection_id,
                5,
                Message::Close(None),
                &input_tx,
                &metrics,
            )
            .await
        );
    }

    #[tokio::test]
    async fn prepared_exec_keeps_credentials_ephemeral_and_mux_exec_credential_free() {
        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": {
                "username": "operator",
                "password": "secret",
                "private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n-----END OPENSSH PRIVATE KEY-----\n"
            }
        });
        let request = AgentSshExecRequest {
            stdin: None,
            command: "printf ready".to_string(),
            timeout: Duration::from_secs(10),
            max_bytes_per_stream: 1024,
        };

        let direct = prepare_exec(connection_id, &payload, &request, None, false)
            .await
            .expect("direct exec preparation should materialize configured credentials");
        let direct_args = pin_test_args(&direct.command);
        assert!(direct.material.private_key_path.is_some());
        assert!(direct.material.password.is_some());
        assert_eq!(direct.material.material_paths.len(), 2);
        assert!(
            direct_args
                .contains(&"PreferredAuthentications=password,keyboard-interactive".to_string())
        );
        assert!(direct_args.contains(&"IdentitiesOnly=yes".to_string()));
        assert_eq!(direct_args.last().map(String::as_str), Some("printf ready"));
        cleanup_ssh_transfer_material(
            &direct.material.material_dir,
            &direct.material.material_paths,
        )
        .await;

        let mux = prepare_exec(
            connection_id,
            &payload,
            &request,
            Some("/tmp/piping-control.sock"),
            true,
        )
        .await
        .expect("mux exec preparation should reuse the authenticated session");
        let mux_args = pin_test_args(&mux.command);
        assert!(mux.material.private_key_path.is_none());
        assert!(mux.material.password.is_none());
        assert!(mux.material.material_paths.is_empty());
        assert!(mux_args.contains(&"ControlMaster=no".to_string()));
        assert!(
            mux_args
                .iter()
                .any(|arg| arg.contains("/tmp/piping-control.sock"))
        );
        assert!(!mux_args.iter().any(|arg| arg == "IdentitiesOnly=yes"));
        cleanup_ssh_transfer_material(&mux.material.material_dir, &mux.material.material_paths)
            .await;
    }

    #[tokio::test]
    async fn prepared_exec_preserves_jump_before_target_identity_order() {
        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": {
                "username": "operator",
                "private_key": "-----BEGIN TEST KEY-----\ntarget\n-----END TEST KEY-----"
            },
            "jump": [{
                "host": "10.0.0.9",
                "port": 2222,
                "credentials": {
                    "username": "jump",
                    "private_key": "-----BEGIN TEST KEY-----\njump\n-----END TEST KEY-----"
                }
            }]
        });
        let request = AgentSshExecRequest {
            stdin: None,
            command: "true".to_string(),
            timeout: Duration::from_secs(10),
            max_bytes_per_stream: 1024,
        };

        let prepared = prepare_exec(connection_id, &payload, &request, None, false)
            .await
            .expect("keyed jump exec preparation should succeed");
        let args = pin_test_args(&prepared.command);
        let jump_key = &prepared.material.jump_key_paths[0];
        let target_key = prepared
            .material
            .private_key_path
            .as_ref()
            .expect("target key should be materialized");
        let jump_position = args
            .iter()
            .position(|arg| arg == jump_key)
            .expect("jump identity should be passed to ssh");
        let target_position = args
            .iter()
            .position(|arg| arg == target_key)
            .expect("target identity should be passed to ssh");
        assert!(jump_position < target_position);
        assert!(prepared.material.password.is_none());
        cleanup_ssh_transfer_material(
            &prepared.material.material_dir,
            &prepared.material.material_paths,
        )
        .await;
    }

    #[tokio::test]
    async fn exec_preparation_validation_failure_removes_material() {
        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": {
                "username": "operator",
                "private_key": "-----BEGIN TEST KEY-----\ntarget\n-----END TEST KEY-----"
            },
            "timeout": "invalid"
        });
        let request = AgentSshExecRequest {
            stdin: None,
            command: "true".to_string(),
            timeout: Duration::from_secs(10),
            max_bytes_per_stream: 1024,
        };

        let result = prepare_exec(connection_id, &payload, &request, None, false).await;
        assert!(matches!(result, Err(AppError::Validation(_))));
        let prefix = format!("piping-ssh-exec-{connection_id}-");
        let leaked = std::fs::read_dir(std::env::temp_dir())
            .expect("temporary directory should be readable")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix));
        assert!(!leaked, "failed preparation must remove operation material");
    }

    #[tokio::test]
    async fn prepared_download_requires_mux_and_preserves_archive_command() {
        let connection_id = Uuid::now_v7();
        let payload = json!({
            "connection_type": "ssh",
            "host": TEST_SSH_HOST,
            "port": TEST_SSH_PORT,
            "credentials": { "username": "operator" }
        });

        let missing_mux =
            match prepare_ssh_download(connection_id, &payload, "/srv/data", true, "").await {
                Ok(_) => panic!("downloads must not open a second unauthenticated connection"),
                Err(error) => error,
            };
        assert!(missing_mux.to_string().contains("ControlMaster"));

        let prepared = prepare_ssh_download(
            connection_id,
            &payload,
            "/srv/data",
            true,
            "/tmp/piping-control.sock",
        )
        .await
        .expect("archive download should prepare a mux-only command");
        let args = pin_test_args(&prepared.command);
        assert!(
            args.iter()
                .any(|arg| arg.contains("/tmp/piping-control.sock"))
        );
        assert!(args.last().is_some_and(|arg| arg.contains("tar")));
        cleanup_ssh_transfer_material(&prepared.material_dir, &prepared.material_paths).await;
    }
}
