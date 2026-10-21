//! Connection proxy service: WebSocket bridges for SSH, RDP, VNC and application sessions, with agent-facing session helpers.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use std::time::Instant;

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use openssl::{memcmp, sha::sha256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::time::Duration;
use uuid::Uuid;

use crate::config::ProxyConfig;
use crate::errors::AppError;
use crate::integrations::proxy::{
    EnvProxySharedSecretProvider, PROXY_NODE_ID_ENV_KEY, PROXY_NODE_SECRET_ENV_KEY,
    ProxySharedSecretProvider,
};

pub(crate) use crate::integrations::proxy::{ProxyLinkMetricsResponse, ProxyMetricsResponse};

mod guac;
mod metrics;
mod pty_unix;
mod rdp;
mod router;
mod ssh;
mod tcp_relay;
mod unix_sys;
mod vnc;
mod ws;

use metrics::ConnectionMetricsState;

#[cfg(test)]
use metrics::derive_link_quality;

use ws::{proxy_ws_loop, send_close_frame, shutdown_signal};

pub use router::{
    build_proxy_router, handle_create_connection, handle_delete_connection, handle_get_metrics,
    serve_proxy, ws_proxy_handler,
};

/// Per-session async-init mutex held in the shared session registries.
type SessionInitLockMap = Arc<RwLock<HashMap<Uuid, Arc<Mutex<()>>>>>;
/// Active RFB clients keyed by VNC session id.
type VncSessionMap = Arc<RwLock<HashMap<Uuid, Arc<Mutex<vnc::RfbClient>>>>>;

#[derive(Clone)]
pub struct ProxyState {
    shared_secret_provider: Arc<dyn ProxySharedSecretProvider>,
    ws_token_ttl_seconds: u64,
    ssh_detach_grace_seconds: u64,
    guacd_host: String,
    guacd_port: u16,
    connections: Arc<RwLock<HashMap<Uuid, ProxyConnection>>>,
    active_bridges: Arc<RwLock<HashSet<Uuid>>>,
    ssh_sessions: Arc<RwLock<HashMap<Uuid, Arc<ssh::SshSession>>>>,
    ssh_session_init_locks: SessionInitLockMap,
    ssh_file_sidecars: Arc<Semaphore>,
    ssh_file_session_sidecars: Arc<RwLock<HashMap<Uuid, Arc<Semaphore>>>>,
    ssh_file_sidecars_per_session: usize,
    vnc_sessions: VncSessionMap,
    vnc_session_init_locks: SessionInitLockMap,
}

#[derive(Debug, Clone)]
struct ProxyConnection {
    payload: Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
}

struct SshFileSidecarPermit {
    _total: OwnedSemaphorePermit,
    _session: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ProxyConnectionType {
    Ssh,
    Rdp,
    Vnc,
    App,
}

const DEFAULT_PROXY_CONNECT_TIMEOUT_SECS: f64 = crate::config::DEFAULT_PROXY_CONNECT_TIMEOUT_SECS;
const MIN_PROXY_CONNECT_TIMEOUT_SECS: f64 = crate::config::MIN_PROXY_CONNECT_TIMEOUT_SECS;
const MAX_PROXY_CONNECT_TIMEOUT_SECS: f64 = crate::config::MAX_PROXY_CONNECT_TIMEOUT_SECS;

fn configure_interactive_tcp_stream(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
}

fn elapsed_latency_ms_since(started_at: Instant) -> i64 {
    i64::try_from(started_at.elapsed().as_millis())
        .unwrap_or(i64::MAX)
        .max(1)
}

#[derive(Debug, Deserialize)]
pub struct ProxyWsQuery {
    pub token: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub dpi: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct RdpDisplayParams {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub dpi: Option<u32>,
}

impl RdpDisplayParams {
    pub fn has_any(self) -> bool {
        self.width.is_some() || self.height.is_some() || self.dpi.is_some()
    }
}

#[derive(Debug, Serialize)]
struct ProxyCreateResponse {
    connection_id: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Clone)]
pub struct AgentSshReadChunk {
    pub seq: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct AgentSshReadSnapshot {
    pub chunks: Vec<AgentSshReadChunk>,
    pub next_seq: u64,
    pub history_start_seq: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct AgentSshExecRequest {
    pub command: String,
    pub timeout: Duration,
    pub max_bytes_per_stream: usize,
    /// Bytes written to the remote command's stdin before it is closed. Used by
    /// the Linux device driver to feed `chpasswd` (and a `sudo -S` become
    /// password) without ever placing a secret on the command line. Never logged
    /// or persisted.
    pub stdin: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct AgentSshExecStream {
    pub bytes: Vec<u8>,
    pub bytes_read: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct AgentSshExecResult {
    pub stdout: AgentSshExecStream,
    pub stderr: AgentSshExecStream,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub timed_out: bool,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
pub struct AgentSshFileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: Option<u64>,
    pub modified_at: Option<String>,
    pub permissions: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentSshFileList {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<AgentSshFileEntry>,
}

#[derive(Debug, Clone)]
pub struct AgentSshFileTransferResult {
    pub bytes: u64,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
pub struct AgentSshFileDownloadPreflight {
    pub path: String,
    pub file_name: String,
    pub bytes: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub entry_count: Option<u64>,
    pub kind: String,
    pub archive: bool,
}

#[derive(Debug, Clone)]
pub struct AgentSshFilePreview {
    pub path: String,
    pub bytes: u64,
    pub preview_bytes: u64,
    pub truncated: bool,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct AgentSshFileDeleteResult {
    pub paths: Vec<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
pub struct AgentSshFileMoveResult {
    pub paths: Vec<String>,
    pub destination: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
pub struct AgentVncCapture {
    pub width: u16,
    pub height: u16,
    pub source_width: u16,
    pub source_height: u16,
    pub scaled: bool,
    pub mime_type: &'static str,
    pub image_bytes: Vec<u8>,
    pub format: &'static str,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AgentVncCaptureFormat {
    Png,
    Bmp,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentVncPointerEvent {
    pub x: u16,
    pub y: u16,
    pub button_mask: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentVncKeyEvent {
    pub keysym: u32,
    pub down: bool,
}

impl ProxyState {
    pub fn new(config: &ProxyConfig) -> Result<Self, AppError> {
        let shared_secret_provider: Arc<dyn ProxySharedSecretProvider> = Arc::new(
            EnvProxySharedSecretProvider::new(PROXY_NODE_ID_ENV_KEY, PROXY_NODE_SECRET_ENV_KEY),
        );
        Ok(Self {
            shared_secret_provider,
            ws_token_ttl_seconds: config.ws_token_ttl_seconds,
            ssh_detach_grace_seconds: config.ssh_detach_grace_seconds,
            guacd_host: config.guacd_host.clone(),
            guacd_port: config.guacd_port,
            connections: Arc::new(RwLock::new(HashMap::new())),
            active_bridges: Arc::new(RwLock::new(HashSet::new())),
            ssh_sessions: Arc::new(RwLock::new(HashMap::new())),
            ssh_session_init_locks: Arc::new(RwLock::new(HashMap::new())),
            ssh_file_sidecars: Arc::new(Semaphore::new(
                config.ssh_file_transfer.max_sidecars_total,
            )),
            ssh_file_session_sidecars: Arc::new(RwLock::new(HashMap::new())),
            ssh_file_sidecars_per_session: config.ssh_file_transfer.max_sidecars_per_session,
            vnc_sessions: Arc::new(RwLock::new(HashMap::new())),
            vnc_session_init_locks: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn new_with_provider(
        config: &ProxyConfig,
        shared_secret_provider: Arc<dyn ProxySharedSecretProvider>,
    ) -> Result<Self, AppError> {
        let _ = shared_secret_provider.get()?;
        Ok(Self {
            shared_secret_provider,
            ws_token_ttl_seconds: config.ws_token_ttl_seconds,
            ssh_detach_grace_seconds: config.ssh_detach_grace_seconds,
            guacd_host: config.guacd_host.clone(),
            guacd_port: config.guacd_port,
            connections: Arc::new(RwLock::new(HashMap::new())),
            active_bridges: Arc::new(RwLock::new(HashSet::new())),
            ssh_sessions: Arc::new(RwLock::new(HashMap::new())),
            ssh_session_init_locks: Arc::new(RwLock::new(HashMap::new())),
            ssh_file_sidecars: Arc::new(Semaphore::new(
                config.ssh_file_transfer.max_sidecars_total,
            )),
            ssh_file_session_sidecars: Arc::new(RwLock::new(HashMap::new())),
            ssh_file_sidecars_per_session: config.ssh_file_transfer.max_sidecars_per_session,
            vnc_sessions: Arc::new(RwLock::new(HashMap::new())),
            vnc_session_init_locks: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Overrides guacd target (useful for tests).
    pub fn with_guacd_target(mut self, host: String, port: u16) -> Self {
        self.guacd_host = host;
        self.guacd_port = port;
        self
    }

    async fn acquire_ssh_file_sidecar(
        &self,
        connection_id: Uuid,
    ) -> Result<SshFileSidecarPermit, AppError> {
        let total = self
            .ssh_file_sidecars
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                AppError::TooManyRequests(
                    "Too many SSH file operations are running. Try again shortly.".to_string(),
                )
            })?;
        let session_semaphore = {
            let mut guard = self.ssh_file_session_sidecars.write().await;
            guard
                .entry(connection_id)
                .or_insert_with(|| Arc::new(Semaphore::new(self.ssh_file_sidecars_per_session)))
                .clone()
        };
        let session = session_semaphore.try_acquire_owned().map_err(|_| {
            AppError::TooManyRequests(
                "Too many SSH file operations are running for this session. Try again shortly."
                    .to_string(),
            )
        })?;
        Ok(SshFileSidecarPermit {
            _total: total,
            _session: session,
        })
    }
}

/// Interval between detach-grace reaper sweeps.
const SSH_DETACH_REAPER_INTERVAL: Duration = Duration::from_secs(5);

/// Spawns the single proxy-wide detach-grace reaper.
///
/// Every `SSH_DETACH_REAPER_INTERVAL` it sweeps the SSH session registry and
/// shuts down + removes any session whose client leg has been detached longer
/// than `proxy.ssh_detach_grace_seconds`. This bounds the lifetime of an
/// abandoned PTY + child `ssh` process when a client never re-attaches. A grace
/// of zero disables reaping (treated as "never reap"), though config validation
/// rejects zero in real deployments.
pub fn spawn_ssh_detach_reaper(state: Arc<ProxyState>) {
    let grace = Duration::from_secs(state.ssh_detach_grace_seconds);
    if grace.is_zero() {
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SSH_DETACH_REAPER_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            ssh::reap_detached_ssh_sessions(&state, grace).await;
        }
    });
}

pub async fn agent_ssh_start(state: Arc<ProxyState>, connection_id: Uuid) -> Result<(), AppError> {
    ssh::agent_ssh_session(state, connection_id)
        .await
        .map(|_| ())
}

pub async fn agent_ssh_write(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    bytes: Vec<u8>,
) -> Result<(), AppError> {
    let session = ssh::agent_ssh_session(state, connection_id).await?;
    session.write(bytes).await
}

pub async fn agent_ssh_resize(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<(), AppError> {
    let session = ssh::agent_ssh_session(state, connection_id).await?;
    session.resize(cols, rows).await
}

pub async fn agent_ssh_read(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    after_seq: Option<u64>,
    max_bytes: usize,
) -> Result<AgentSshReadSnapshot, AppError> {
    let session = ssh::agent_ssh_session(state, connection_id).await?;
    Ok(session.read_since(after_seq, max_bytes).await)
}

pub async fn agent_ssh_exec(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    request: AgentSshExecRequest,
) -> Result<AgentSshExecResult, AppError> {
    ssh::agent_ssh_exec(state, connection_id, request).await
}

/// Runs a one-shot SSH exec against a caller-supplied proxy payload that is not
/// backed by a registered proxy connection.
///
/// The device-management Linux driver resolves launch material for a target and
/// has no connection session to register, so this uses a throwaway connection id
/// and metrics state: lifecycle recording there never reaches a registered
/// connection.
pub async fn ssh_exec_payload(
    payload: Value,
    request: AgentSshExecRequest,
) -> Result<AgentSshExecResult, AppError> {
    ssh::ssh_exec_payload(payload, request).await
}

pub async fn agent_ssh_list_dir(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    path: String,
) -> Result<AgentSshFileList, AppError> {
    ssh::agent_ssh_list_dir(state, connection_id, path).await
}

pub async fn agent_ssh_upload_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    local_path: PathBuf,
    remote_path: String,
) -> Result<AgentSshFileTransferResult, AppError> {
    ssh::agent_ssh_upload_file(state, connection_id, local_path, remote_path).await
}

pub async fn agent_ssh_ensure_dir(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
) -> Result<AgentSshFileTransferResult, AppError> {
    ssh::agent_ssh_ensure_dir(state, connection_id, remote_path).await
}

pub async fn agent_ssh_preflight_download_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
    max_bytes: u64,
    archive: bool,
) -> Result<AgentSshFileDownloadPreflight, AppError> {
    ssh::agent_ssh_preflight_download_file(state, connection_id, remote_path, max_bytes, archive)
        .await
}

pub async fn agent_ssh_preview_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
) -> Result<AgentSshFilePreview, AppError> {
    ssh::agent_ssh_preview_file(state, connection_id, remote_path).await
}

pub async fn agent_ssh_delete_files(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_paths: Vec<String>,
) -> Result<AgentSshFileDeleteResult, AppError> {
    ssh::agent_ssh_delete_files(state, connection_id, remote_paths).await
}

pub async fn agent_ssh_move_files(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_paths: Vec<String>,
    destination: String,
) -> Result<AgentSshFileMoveResult, AppError> {
    ssh::agent_ssh_move_files(state, connection_id, remote_paths, destination).await
}

pub async fn agent_ssh_download_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
    local_path: PathBuf,
    max_bytes: u64,
    archive: bool,
) -> Result<AgentSshFileTransferResult, AppError> {
    ssh::agent_ssh_download_file(
        state,
        connection_id,
        remote_path,
        local_path,
        max_bytes,
        archive,
    )
    .await
}

pub fn agent_ssh_download_file_name(path: &str, archive: bool) -> String {
    ssh::remote_file_name(path, archive)
}

pub async fn agent_vnc_capture(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    format: AgentVncCaptureFormat,
) -> Result<AgentVncCapture, AppError> {
    vnc::agent_vnc_capture(state, connection_id, format).await
}

pub async fn agent_vnc_pointer_events(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    events: Vec<AgentVncPointerEvent>,
) -> Result<(), AppError> {
    vnc::agent_vnc_pointer_events(state, connection_id, events).await
}

pub async fn agent_vnc_key_events(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    events: Vec<AgentVncKeyEvent>,
) -> Result<(), AppError> {
    vnc::agent_vnc_key_events(state, connection_id, events).await
}

pub async fn agent_vnc_text(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    text: String,
) -> Result<(), AppError> {
    vnc::agent_vnc_text(state, connection_id, text).await
}

fn parse_connection_type(payload: &Value) -> Result<ProxyConnectionType, &'static str> {
    let Value::Object(map) = payload else {
        return Err("Proxy payload must be a JSON object");
    };
    let Some(Value::String(raw)) = map.get("connection_type") else {
        return Err("Proxy payload must include connection_type");
    };
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "ssh" => Ok(ProxyConnectionType::Ssh),
        "rdp" => Ok(ProxyConnectionType::Rdp),
        "vnc" => Ok(ProxyConnectionType::Vnc),
        "app" => Ok(ProxyConnectionType::App),
        _ => Err("Unsupported connection_type"),
    }
}

fn extract_connection_id(payload: &Value) -> Result<Uuid, &'static str> {
    let Value::Object(map) = payload else {
        return Err("Proxy payload must be a JSON object");
    };
    let Some(Value::String(raw)) = map.get("connection_id") else {
        return Err("Proxy payload must include connection_id");
    };
    Uuid::parse_str(raw).map_err(|_| "Invalid connection_id")
}

pub(crate) fn extract_host_port(payload: &Value) -> Result<(String, u16), &'static str> {
    let Value::Object(map) = payload else {
        return Err("Proxy payload must be a JSON object");
    };
    let host = map
        .get("host")
        .and_then(Value::as_str)
        .ok_or("Proxy payload host is required")?;
    let port = map
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(0);
    let connection_type = parse_connection_type(payload)?;
    let default_port = match connection_type {
        ProxyConnectionType::Rdp => 3389,
        ProxyConnectionType::Vnc => 5900,
        ProxyConnectionType::Ssh | ProxyConnectionType::App => 22,
    };
    Ok((
        host.to_string(),
        if port == 0 { default_port } else { port },
    ))
}

fn extract_connect_timeout_duration(payload: &Value) -> Result<Duration, &'static str> {
    extract_connect_timeout_seconds(payload).map(Duration::from_secs_f64)
}

fn extract_connect_timeout_seconds(payload: &Value) -> Result<f64, &'static str> {
    let Some(timeout_value) = payload.as_object().and_then(|map| map.get("timeout")) else {
        return Ok(DEFAULT_PROXY_CONNECT_TIMEOUT_SECS);
    };
    let parsed = parse_timeout_seconds(timeout_value)?;

    Ok(parsed.clamp(
        MIN_PROXY_CONNECT_TIMEOUT_SECS,
        MAX_PROXY_CONNECT_TIMEOUT_SECS,
    ))
}

fn parse_timeout_seconds(value: &Value) -> Result<f64, &'static str> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .filter(|timeout| *timeout > 0.0)
            .ok_or("Invalid timeout value"),
        _ => Err("Invalid timeout value"),
    }
}

fn require_secret(state: &ProxyState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let node_id = headers
        .get("x-piping-proxy-node-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let secret = headers
        .get("x-piping-proxy-secret")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let expected = state.shared_secret_provider.get().map_err(|_| {
        Box::new(error_response(
            StatusCode::UNAUTHORIZED,
            "Proxy node credential is not configured",
        ))
    })?;
    if node_id != expected.node_id
        || !memcmp::eq(
            &sha256(secret.as_bytes()),
            &sha256(expected.secret.as_bytes()),
        )
    {
        Err(Box::new(error_response(
            StatusCode::UNAUTHORIZED,
            "Invalid proxy node credential",
        )))
    } else {
        Ok(())
    }
}

fn error_response<T: Into<String>>(status: StatusCode, message: T) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::guac::looks_like_guacamole_instruction;
    use super::metrics::{LinkType, TrafficDirection};
    use super::{
        ConnectionMetricsState, DEFAULT_PROXY_CONNECT_TIMEOUT_SECS, MAX_PROXY_CONNECT_TIMEOUT_SECS,
        MIN_PROXY_CONNECT_TIMEOUT_SECS, ProxyConnectionType, derive_link_quality,
        elapsed_latency_ms_since, extract_connect_timeout_seconds, parse_connection_type,
    };

    #[test]
    fn connect_timeout_uses_default_when_missing() {
        let payload = json!({
            "connection_id": "019c44bf-53f9-73c1-93ab-e326ff780741",
            "connection_type": "vnc",
            "host": "10.10.10.251",
            "port": 5900
        });
        assert_eq!(
            extract_connect_timeout_seconds(&payload).expect("default timeout"),
            DEFAULT_PROXY_CONNECT_TIMEOUT_SECS
        );
    }

    #[test]
    fn connect_timeout_respects_numeric_value() {
        let payload = json!({ "timeout": 7 });
        assert_eq!(
            extract_connect_timeout_seconds(&payload).expect("timeout"),
            7.0
        );
    }

    #[test]
    fn connect_timeout_clamps_out_of_range_values() {
        let too_small = json!({ "timeout": 0.1 });
        let too_large = json!({ "timeout": 9999 });
        assert_eq!(
            extract_connect_timeout_seconds(&too_small).expect("timeout"),
            MIN_PROXY_CONNECT_TIMEOUT_SECS
        );
        assert_eq!(
            extract_connect_timeout_seconds(&too_large).expect("timeout"),
            MAX_PROXY_CONNECT_TIMEOUT_SECS
        );
    }

    #[test]
    fn connect_timeout_rejects_non_numeric_value() {
        let payload = json!({ "timeout": "7" });
        let error = extract_connect_timeout_seconds(&payload).expect_err("invalid timeout");
        assert!(
            error.contains("Invalid timeout"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn detects_guacamole_instruction_prefix() {
        assert!(looks_like_guacamole_instruction("6.select,3.rdp;"));
        assert!(looks_like_guacamole_instruction("7.connect,1.a;"));
        assert!(!looks_like_guacamole_instruction("select rdp"));
    }

    #[test]
    fn derive_link_quality_maps_latency_ranges() {
        assert_eq!(
            derive_link_quality(Some(10)),
            (Some("good".to_string()), Some(100))
        );
        assert_eq!(
            derive_link_quality(Some(100)),
            (Some("warning".to_string()), Some(75))
        );
        assert_eq!(
            derive_link_quality(Some(400)),
            (Some("poor".to_string()), Some(40))
        );
        assert_eq!(derive_link_quality(None), (None, None));
    }

    #[test]
    fn elapsed_latency_ms_since_floors_successful_measurements_to_one_ms() {
        assert!(elapsed_latency_ms_since(std::time::Instant::now()) >= 1);
    }

    #[test]
    fn metrics_state_tracks_link_traffic_and_status() {
        let mut state = ConnectionMetricsState::default();
        state.mark_connecting();
        state.mark_connected(Some(42));
        state.record_traffic(LinkType::ClientProxy, TrafficDirection::Tx, 128);
        state.record_traffic(LinkType::ProxyProtocol, TrafficDirection::Tx, 128);
        state.record_traffic(LinkType::ProxyProtocol, TrafficDirection::Rx, 256);
        state.record_traffic(LinkType::ClientProxy, TrafficDirection::Rx, 256);

        let snapshot = state.as_response(uuid::Uuid::nil());
        assert_eq!(snapshot.overall_status, "connected");
        assert_eq!(snapshot.links.len(), 2);

        let client_link = snapshot
            .links
            .iter()
            .find(|link| link.link_type == "client_proxy")
            .expect("client_proxy link must exist");
        assert_eq!(client_link.tx_bytes, Some(128));
        assert_eq!(client_link.rx_bytes, Some(256));
        assert_eq!(client_link.status, "healthy");

        let proxy_link = snapshot
            .links
            .iter()
            .find(|link| link.link_type == "proxy_protocol")
            .expect("proxy_protocol link must exist");
        assert_eq!(proxy_link.latency_ms, Some(42));
        assert_eq!(proxy_link.status, "healthy");
    }

    #[test]
    fn metrics_state_reports_connecting_links_during_bridge_startup() {
        let mut state = ConnectionMetricsState::default();

        state.mark_connecting();

        let snapshot = state.as_response(uuid::Uuid::nil());
        assert_eq!(snapshot.overall_status, "connecting");
        assert!(
            snapshot
                .links
                .iter()
                .all(|link| link.status == "connecting")
        );
    }

    #[test]
    fn parse_connection_type_requires_field() {
        let payload = json!({ "host": "10.10.1.230" });
        let error = parse_connection_type(&payload).expect_err("missing type must fail");
        assert!(
            error.contains("connection_type"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parse_connection_type_rejects_unknown_value() {
        let payload = json!({ "connection_type": "telnet" });
        let error = parse_connection_type(&payload).expect_err("unknown type must fail");
        assert!(error.contains("Unsupported"), "unexpected error: {error}");
    }

    #[test]
    fn parse_connection_type_accepts_case_insensitive_input() {
        let payload = json!({ "connection_type": "RDP" });
        let parsed = parse_connection_type(&payload).expect("rdp type must parse");
        assert_eq!(parsed, ProxyConnectionType::Rdp);
    }
}
