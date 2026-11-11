//! Shared native desktop runtime for RDP and browser VNC.
//!
//! One guacd connection owns the target login for the lifetime of a proxy
//! connection generation. Its `ready` identifier is retained so that a
//! re-attaching operator or a read-only observer joins the existing desktop
//! through the native Guacamole `select <id>` handshake instead of logging in
//! again. The owner guacd task survives a browser detach within the configured
//! detach grace and ends observers when the runtime ends.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use chrono::{DateTime, Utc};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use uuid::Uuid;

use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_error_event, log_event};

use super::super::guac::{
    GUAC_STATUS_CLIENT_BAD_TYPE, GUAC_STATUS_RESOURCE_CLOSED, GUAC_STATUS_UPSTREAM_ERROR,
    GUAC_STATUS_UPSTREAM_TIMEOUT, GUAC_STATUS_UPSTREAM_UNAVAILABLE, SliceBuffer,
    encode_guac_instruction, guac_opcode, is_guac_internal_ping, looks_like_guacamole_instruction,
    parse_guac_instruction,
};
use super::super::metrics::{
    ConnectionMetricsState, LinkType, TrafficDirection, mark_connection_metrics_connected,
    mark_connection_metrics_disconnected, mark_connection_metrics_error,
    mark_connection_metrics_proxy_unhealthy, record_connection_traffic,
};
use super::super::{
    ProxyConnection, ProxyConnectionType, ProxyRuntimePresence, ProxyState,
    configure_interactive_tcp_stream, extract_connect_timeout_duration, extract_host_port,
    parse_connection_type, send_close_frame,
};
use super::{
    DesktopGeometry, GUAC_READ_BUFFER_SIZE, GuacClientCapabilities, GuacdHandshakeFailure,
    GuacdReadyInstruction, IMAGE_MIMETYPES, NO_MIMETYPES, RdpConnectContext,
    build_guacd_connect_elements, cleanup_embedded_rdp_drive, extract_guacd_version,
    extract_rdp_client_options, extract_rdp_launch_config, extract_vnc_client_options,
    forward_ready_to_client, log_empty_rdp_credentials, perform_guacd_handshake,
    probe_rdp_target_latency_logged, rdp_drive_base_path, resolve_rdp_security_mode,
    send_client_handshake, send_guac_error_and_close, send_tunnel_uuid,
};

type Metrics = Arc<RwLock<ConnectionMetricsState>>;
type WsSender = SplitSink<WebSocket, Message>;
type SharedWsSender = Arc<Mutex<WsSender>>;

const MAX_DESKTOP_INSTRUCTION_BYTES: usize = 32 * 1024 * 1024;
const MAX_OBSERVER_PENDING_SYNCS: usize = 512;
const MAX_OBSERVER_STREAMS: usize = 4096;
const MAX_OBSERVER_INPUT_BYTES: usize = 4096;
const OBSERVER_CLOSE_INVALID_INPUT: u16 = 1008;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(in crate::proxy) enum DesktopProtocol {
    Rdp,
    Vnc,
}

impl DesktopProtocol {
    fn selector(self) -> &'static str {
        match self {
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
        }
    }

    fn from_connection_type(value: ProxyConnectionType) -> Option<Self> {
        match value {
            ProxyConnectionType::Rdp => Some(Self::Rdp),
            ProxyConnectionType::Vnc => Some(Self::Vnc),
            ProxyConnectionType::Ssh | ProxyConnectionType::App => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum DesktopState {
    Connecting,
    Live,
    Detached,
    Ended,
}

struct DesktopRuntime {
    state: DesktopState,
    operator_attached: bool,
    last_operator_activity_at: Option<DateTime<Utc>>,
    detached_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
}

struct DesktopUpstream {
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
    stream_buf: SliceBuffer,
    buffer: Vec<u8>,
    ready: GuacdReadyInstruction,
}

/// One browser leg. The owner leg is the first operator and is owned by the
/// guacd task; later operator legs and observers join the retained ready ID.
#[derive(Clone, Copy)]
enum LegMode {
    Operator { epoch: u64, size: DesktopGeometry },
    Observer { size: DesktopGeometry },
}

/// Which side of the shared desktop a leg represents when choosing the client
/// capability set. Owner and operator both drive the desktop; observers never
/// do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopRole {
    Owner,
    Operator,
    Observer,
}

impl LegMode {
    fn read_only(self) -> bool {
        matches!(self, Self::Observer { .. })
    }

    fn is_observer(self) -> bool {
        matches!(self, Self::Observer { .. })
    }

    fn size(self) -> DesktopGeometry {
        match self {
            Self::Operator { size, .. } | Self::Observer { size } => size,
        }
    }

    fn role(self) -> DesktopRole {
        match self {
            Self::Operator { .. } => DesktopRole::Operator,
            Self::Observer { .. } => DesktopRole::Observer,
        }
    }

    fn capabilities(self, protocol: DesktopProtocol) -> GuacClientCapabilities {
        desktop_client_capabilities(protocol, self.role(), self.size())
    }
}

/// The single owner of the client capability set for every desktop leg.
///
/// The size is always present and non-zero: guacd copies it into the joining
/// user's `info`, and the VNC/RDP plugins divide by values derived from it.
/// Audio and video are always sent with an empty codec list to mean "not
/// supported"; guacd initializes those lists only when the instruction arrives.
fn desktop_client_capabilities(
    protocol: DesktopProtocol,
    role: DesktopRole,
    size: DesktopGeometry,
) -> GuacClientCapabilities {
    let timezone = match (protocol, role) {
        (_, DesktopRole::Observer) => None,
        (_, DesktopRole::Operator) => Some("Etc/UTC"),
        (DesktopProtocol::Rdp, DesktopRole::Owner) => Some("Etc/UTC"),
        (DesktopProtocol::Vnc, DesktopRole::Owner) => None,
    };
    GuacClientCapabilities {
        size,
        audio: NO_MIMETYPES,
        video: NO_MIMETYPES,
        image: IMAGE_MIMETYPES,
        timezone,
    }
}

enum DesktopAttach {
    Established {
        session: Arc<DesktopSession>,
        upstream: DesktopUpstream,
        ready: GuacdReadyInstruction,
        epoch: u64,
    },
    Existing {
        session: Arc<DesktopSession>,
    },
}

enum AttachedOutcome {
    Detached,
    Ended,
    Error,
}

enum KeeperOutcome {
    Ended,
    Error,
}

enum OwnerOutcome {
    Ended,
    Error,
}

enum ForwardStatus {
    Forwarded,
    BrowserGone,
}

pub(in crate::proxy) struct DesktopSession {
    connection_id: Uuid,
    generation: Uuid,
    protocol: DesktopProtocol,
    guacd_connection_id: String,
    /// Geometry advertised by the owner when the shared desktop was created.
    /// Joining legs reuse it so their handshake capabilities are always
    /// non-zero without inventing a per-leg size.
    owner_geometry: DesktopGeometry,
    enable_drive: bool,
    grace_seconds: u64,
    runtime: Mutex<DesktopRuntime>,
    operator_epoch: AtomicU64,
    ended_tx: watch::Sender<bool>,
    owner_handle: StdMutex<Option<JoinHandle<()>>>,
}

impl DesktopSession {
    fn new(
        connection_id: Uuid,
        generation: Uuid,
        protocol: DesktopProtocol,
        guacd_connection_id: String,
        owner_geometry: DesktopGeometry,
        enable_drive: bool,
        grace_seconds: u64,
    ) -> Self {
        let (ended_tx, _) = watch::channel(false);
        Self {
            connection_id,
            generation,
            protocol,
            guacd_connection_id,
            owner_geometry,
            enable_drive,
            grace_seconds,
            runtime: Mutex::new(DesktopRuntime {
                state: DesktopState::Connecting,
                operator_attached: false,
                last_operator_activity_at: None,
                detached_at: Some(Utc::now()),
                ended_at: None,
            }),
            operator_epoch: AtomicU64::new(0),
            ended_tx,
            owner_handle: StdMutex::new(None),
        }
    }

    fn generation(&self) -> Uuid {
        self.generation
    }

    fn protocol(&self) -> DesktopProtocol {
        self.protocol
    }

    fn subscribe_ended(&self) -> watch::Receiver<bool> {
        self.ended_tx.subscribe()
    }

    async fn is_ended(&self) -> bool {
        self.runtime.lock().await.state == DesktopState::Ended
    }

    fn grace(&self) -> chrono::Duration {
        // Config validation bounds the grace to one hour; clamp defensively so
        // an out-of-range persisted value cannot overflow DateTime arithmetic.
        chrono::Duration::seconds(i64::try_from(self.grace_seconds.min(3_600)).unwrap_or(3_600))
    }

    async fn try_begin_operator_attach(&self) -> Option<u64> {
        let mut runtime = self.runtime.lock().await;
        if runtime.state == DesktopState::Ended || runtime.operator_attached {
            return None;
        }
        let epoch = self.operator_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        runtime.state = DesktopState::Live;
        runtime.operator_attached = true;
        runtime.detached_at = None;
        runtime.last_operator_activity_at = Some(Utc::now());
        Some(epoch)
    }

    async fn mark_operator_detached(&self, epoch: u64) {
        let mut runtime = self.runtime.lock().await;
        if self.operator_epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        if runtime.state == DesktopState::Ended {
            return;
        }
        runtime.state = DesktopState::Detached;
        runtime.operator_attached = false;
        runtime.detached_at = Some(Utc::now());
    }

    async fn note_operator_activity(&self, epoch: u64) {
        let mut runtime = self.runtime.lock().await;
        if self.operator_epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        if runtime.state == DesktopState::Live {
            runtime.last_operator_activity_at = Some(Utc::now());
        }
    }

    async fn reconnect_expired(&self) -> bool {
        if self.grace_seconds == 0 {
            return false;
        }
        let runtime = self.runtime.lock().await;
        if runtime.state == DesktopState::Ended || runtime.operator_attached {
            return false;
        }
        let Some(detached_at) = runtime.detached_at else {
            return false;
        };
        Utc::now() >= detached_at + self.grace()
    }

    async fn mark_ended(&self) {
        let mut runtime = self.runtime.lock().await;
        if runtime.state == DesktopState::Ended {
            return;
        }
        runtime.state = DesktopState::Ended;
        runtime.operator_attached = false;
        runtime.detached_at = None;
        runtime.ended_at = Some(Utc::now());
        drop(runtime);
        self.ended_tx.send_replace(true);
    }

    /// Ends the owner runtime and every attached browser leg. Idempotent.
    pub(in crate::proxy) async fn shutdown(&self) {
        self.mark_ended().await;
        let handle = self
            .owner_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }
    }

    async fn runtime_presence(&self) -> ProxyRuntimePresence {
        let runtime = self.runtime.lock().await;
        let state = match runtime.state {
            DesktopState::Connecting => "connecting",
            DesktopState::Live => "live",
            DesktopState::Detached => "detached",
            DesktopState::Ended => "ended",
        };
        let reconnect_deadline = match runtime.state {
            DesktopState::Connecting | DesktopState::Detached => runtime
                .detached_at
                .map(|detached_at| (detached_at + self.grace()).to_rfc3339()),
            DesktopState::Live | DesktopState::Ended => None,
        };
        ProxyRuntimePresence {
            state: state.to_string(),
            operator_attached: Some(runtime.operator_attached),
            last_operator_activity_at: runtime.last_operator_activity_at.map(|at| at.to_rfc3339()),
            reconnect_deadline,
            ended_at: runtime.ended_at.map(|at| at.to_rfc3339()),
            ..Default::default()
        }
    }

    fn spawn_owner(
        self: &Arc<Self>,
        upstream: DesktopUpstream,
        socket: Option<WebSocket>,
        epoch: u64,
        metrics: Metrics,
    ) {
        let session = self.clone();
        let handle = tokio::spawn(owner_loop(session, upstream, socket, epoch, metrics));
        *self
            .owner_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
    }

    async fn attach_operator(
        self: &Arc<Self>,
        state: &Arc<ProxyState>,
        mut socket: WebSocket,
        payload: &Value,
        metrics: Metrics,
    ) -> Result<(), AppError> {
        let Some(epoch) = self.try_begin_operator_attach().await else {
            let reason = if self.is_ended().await {
                "Desktop session has ended"
            } else {
                super::super::ws::WS_REASON_CONCURRENT_ATTACH_REJECTED
            };
            send_close_frame(&mut socket, 1008, reason).await;
            return Ok(());
        };
        let _guard = OperatorLegGuard {
            session: self.clone(),
            epoch,
        };
        let mode = self.operator_join_mode(payload, epoch)?;
        let guacd_target = format!("{}:{}", state.guacd_host, state.guacd_port);
        let result = self
            .run_leg(socket, payload, mode, &guacd_target, metrics, None)
            .await;
        self.mark_operator_detached(epoch).await;
        result
    }

    async fn attach_observer(
        self: &Arc<Self>,
        state: &Arc<ProxyState>,
        socket: WebSocket,
        metrics: Metrics,
        deadline: tokio::time::Instant,
    ) -> Result<(), AppError> {
        let guacd_target = format!("{}:{}", state.guacd_host, state.guacd_port);
        let payload = Value::Null;
        self.run_leg(
            socket,
            &payload,
            LegMode::Observer {
                size: self.owner_geometry,
            },
            &guacd_target,
            metrics,
            Some(deadline),
        )
        .await
    }

    fn operator_join_mode(&self, payload: &Value, epoch: u64) -> Result<LegMode, AppError> {
        let size = match self.protocol {
            DesktopProtocol::Rdp => extract_rdp_client_options(payload)?.geometry,
            DesktopProtocol::Vnc => self.owner_geometry,
        };
        Ok(LegMode::Operator { epoch, size })
    }

    /// Joins the retained ready ID. Never sends a protocol `select`, never
    /// supplies target credentials and never starts a second target login.
    async fn run_leg(
        self: &Arc<Self>,
        mut socket: WebSocket,
        payload: &Value,
        mode: LegMode,
        guacd_target: &str,
        metrics: Metrics,
        observer_deadline: Option<tokio::time::Instant>,
    ) -> Result<(), AppError> {
        // Retain the socket until JOIN is ready so the lease also bounds the
        // handshake and output backpressure, not just the final relay loop.
        let prepare = async {
            let connect_timeout = extract_connect_timeout_duration(payload)
                .map_err(|error| AppError::Validation(error.to_string()))?;
            let stream = match connect_upstream(guacd_target, connect_timeout, &metrics).await {
                Ok(stream) => stream,
                Err(error) => {
                    send_guac_error_and_close(
                        &mut socket,
                        GUAC_STATUS_UPSTREAM_UNAVAILABLE,
                        "Desktop backend is unavailable",
                    )
                    .await;
                    return Err(error);
                }
            };
            let (mut reader, mut writer) = stream.into_split();
            let mut buffer = vec![0u8; GUAC_READ_BUFFER_SIZE];
            let mut stream_buf = SliceBuffer::new();

            let args = match perform_guacd_handshake(
                &metrics,
                &mut reader,
                &mut writer,
                &mut stream_buf,
                &mut buffer,
                connect_timeout,
                &self.guacd_connection_id,
            )
            .await
            {
                Ok(args) => args,
                Err(GuacdHandshakeFailure::TimedOut) => {
                    send_guac_error_and_close(
                        &mut socket,
                        GUAC_STATUS_UPSTREAM_TIMEOUT,
                        "Desktop handshake timed out",
                    )
                    .await;
                    return Err(AppError::External(format!(
                        "guacd join handshake timed out after {}ms",
                        connect_timeout.as_millis()
                    )));
                }
                Err(GuacdHandshakeFailure::Failed(error)) => {
                    send_guac_error_and_close(
                        &mut socket,
                        GUAC_STATUS_UPSTREAM_ERROR,
                        "Desktop handshake failed",
                    )
                    .await;
                    return Err(error);
                }
            };
            if args.is_empty() {
                send_guac_error_and_close(
                    &mut socket,
                    GUAC_STATUS_UPSTREAM_ERROR,
                    "Desktop handshake failed",
                )
                .await;
                return Err(AppError::External(
                    "guacd join handshake returned empty args".to_string(),
                ));
            }
            let (version, args) = match extract_guacd_version(args) {
                Ok(value) => value,
                Err(error) => {
                    send_guac_error_and_close(
                        &mut socket,
                        GUAC_STATUS_UPSTREAM_ERROR,
                        "Desktop handshake failed",
                    )
                    .await;
                    return Err(error);
                }
            };
            if args.is_empty() {
                send_guac_error_and_close(
                    &mut socket,
                    GUAC_STATUS_UPSTREAM_ERROR,
                    "Desktop handshake failed",
                )
                .await;
                return Err(AppError::External(
                    "guacd join handshake returned empty parameter list".to_string(),
                ));
            }

            let capabilities = mode.capabilities(self.protocol);
            let elements = build_join_connect_elements(&args, &version, self.protocol, mode);
            send_client_handshake(&mut writer, &metrics, &capabilities, &elements).await?;
            let ready = match wait_ready(
                &mut reader,
                &mut stream_buf,
                &mut buffer,
                &metrics,
                connect_timeout,
            )
            .await
            {
                Ok(ready) => ready,
                Err(error) => {
                    send_guac_error_and_close(
                        &mut socket,
                        GUAC_STATUS_UPSTREAM_ERROR,
                        "Desktop handshake failed",
                    )
                    .await;
                    return Err(error);
                }
            };
            if ready.args.first().map(String::as_str) != Some(self.guacd_connection_id.as_str()) {
                send_guac_error_and_close(
                    &mut socket,
                    GUAC_STATUS_UPSTREAM_ERROR,
                    "Desktop session identity mismatch",
                )
                .await;
                return Err(AppError::External(
                    "guacd joined a different desktop connection".to_string(),
                ));
            }
            forward_ready_to_client(&mut socket, &metrics, &ready).await?;

            Ok::<_, AppError>((reader, writer, stream_buf, buffer))
        };
        let (reader, writer, stream_buf, buffer) = match observer_deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, prepare).await {
                Ok(result) => result?,
                Err(_) => {
                    super::super::observation::send_observer_lease_close(&mut socket).await;
                    return Ok(());
                }
            },
            None => prepare.await?,
        };

        relay_join_leg(
            self.clone(),
            socket,
            reader,
            writer,
            stream_buf,
            buffer,
            mode,
            metrics,
            observer_deadline,
        )
        .await
    }
}

struct OperatorLegGuard {
    session: Arc<DesktopSession>,
    epoch: u64,
}

impl Drop for OperatorLegGuard {
    fn drop(&mut self) {
        let session = self.session.clone();
        let epoch = self.epoch;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { session.mark_operator_detached(epoch).await });
        }
    }
}

pub(in crate::proxy) async fn runtime_presence(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
) -> Option<ProxyRuntimePresence> {
    let session = state
        .desktop_sessions
        .read()
        .await
        .get(&connection_id)
        .cloned()?;
    Some(session.runtime_presence().await)
}

pub(in crate::proxy) async fn proxy_ws_desktop_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    mut socket: WebSocket,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) -> Result<(), AppError> {
    if send_tunnel_uuid(&mut socket, connection_id).await.is_err() {
        return Ok(());
    }
    let protocol = match parse_connection_type(payload) {
        Ok(connection_type) => match DesktopProtocol::from_connection_type(connection_type) {
            Some(protocol) => protocol,
            None => {
                send_close_frame(&mut socket, 1008, "Desktop session is not supported").await;
                return Ok(());
            }
        },
        Err(error) => {
            send_close_frame(&mut socket, 1008, error).await;
            return Ok(());
        }
    };

    let attach = match get_or_establish(&state, connection_id, protocol, payload, &metrics).await {
        Ok(attach) => attach,
        Err(error) => {
            send_guac_error_and_close(
                &mut socket,
                GUAC_STATUS_UPSTREAM_UNAVAILABLE,
                "Desktop session is unavailable",
            )
            .await;
            log_error_event(
                "proxy.desktop.session.unavailable",
                "proxy_desktop_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("proxy_ws_desktop_loop")
                    .with_resource("proxy_connection", connection_id.to_string()),
            );
            return Err(error);
        }
    };

    match attach {
        DesktopAttach::Established {
            session,
            upstream,
            ready,
            epoch,
        } => {
            let ready_sent = forward_ready_to_client(&mut socket, &metrics, &ready)
                .await
                .is_ok();
            if !ready_sent {
                session.mark_operator_detached(epoch).await;
            }
            session.spawn_owner(
                upstream,
                if ready_sent { Some(socket) } else { None },
                epoch,
                metrics,
            );
            Ok(())
        }
        DesktopAttach::Existing { session } => {
            session
                .attach_operator(&state, socket, payload, metrics)
                .await
        }
    }
}

pub(in crate::proxy) async fn proxy_ws_desktop_observe_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    mut socket: WebSocket,
    expected_generation: Uuid,
    deadline: tokio::time::Instant,
) -> Result<(), AppError> {
    let prepare = async {
        let (session, metrics) =
            match resolve_observer_session(&state, connection_id, expected_generation).await {
                Ok(value) => value,
                Err(error) => {
                    send_close_frame(&mut socket, 1008, "Desktop session is not available").await;
                    return Err(error);
                }
            };
        send_tunnel_uuid(&mut socket, connection_id).await?;
        Ok((session, metrics))
    };
    let (session, metrics) = match tokio::time::timeout_at(deadline, prepare).await {
        Ok(result) => result?,
        Err(_) => {
            super::super::observation::send_observer_lease_close(&mut socket).await;
            return Ok(());
        }
    };
    session
        .attach_observer(&state, socket, metrics, deadline)
        .await
}

/// Resolves the connection generation and the desktop runtime under the same
/// connection read lock. A delete/recreate cannot swap the runtime between the
/// generation check and the cloned session.
async fn resolve_observer_session(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
    expected_generation: Uuid,
) -> Result<(Arc<DesktopSession>, Metrics), AppError> {
    let connections = state.connections.read().await;
    let entry = connections
        .get(&connection_id)
        .ok_or_else(|| AppError::NotFound("Desktop session is not available".to_string()))?;
    if entry.generation != expected_generation {
        return Err(AppError::Forbidden(
            "Observation generation is no longer available".to_string(),
        ));
    }
    let sessions = state.desktop_sessions.read().await;
    let session = sessions
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| AppError::NotFound("Desktop session is not ready".to_string()))?;
    let metrics = entry.metrics.clone();
    drop(sessions);
    drop(connections);
    if session.generation() != expected_generation {
        return Err(AppError::Forbidden(
            "Observation generation is no longer available".to_string(),
        ));
    }
    if session.is_ended().await {
        return Err(AppError::NotFound("Desktop session has ended".to_string()));
    }
    Ok((session, metrics))
}

async fn existing_session_for_attach(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
    entry: &ProxyConnection,
    protocol: DesktopProtocol,
) -> Result<Option<Arc<DesktopSession>>, AppError> {
    let session = state
        .desktop_sessions
        .read()
        .await
        .get(&connection_id)
        .cloned();
    let Some(session) = session else {
        return Ok(None);
    };
    if session.generation() != entry.generation {
        return Err(AppError::Conflict(
            "Desktop session generation no longer matches the connection".to_string(),
        ));
    }
    if session.protocol() != protocol {
        return Err(AppError::Validation(
            "Desktop session protocol does not match the connection".to_string(),
        ));
    }
    if session.is_ended().await {
        // A resume must not silently start a fresh upstream login under the
        // same proxy connection generation.
        return Err(AppError::NotFound(
            "Desktop session has ended; create a new connection to log in again".to_string(),
        ));
    }
    Ok(Some(session))
}

async fn get_or_establish(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
    protocol: DesktopProtocol,
    payload: &Value,
    metrics: &Metrics,
) -> Result<DesktopAttach, AppError> {
    let init_lock = {
        let mut locks = state.desktop_session_init_locks.write().await;
        locks
            .entry(connection_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _init_guard = init_lock.lock().await;

    let entry = {
        let connections = state.connections.read().await;
        connections.get(&connection_id).cloned()
    };
    let Some(entry) = entry else {
        return Err(AppError::NotFound(
            "Proxy connection is not registered".to_string(),
        ));
    };
    if let Some(session) =
        existing_session_for_attach(state, connection_id, &entry, protocol).await?
    {
        return Ok(DesktopAttach::Existing { session });
    }

    let guacd_target = format!("{}:{}", state.guacd_host, state.guacd_port);
    let (upstream, enable_drive, owner_geometry) =
        establish_owner_upstream(connection_id, protocol, payload, &guacd_target, metrics).await?;
    let ready_id =
        upstream.ready.args.first().cloned().ok_or_else(|| {
            AppError::External("guacd ready carried no connection id".to_string())
        })?;
    log_event(
        LogLevel::Info,
        "proxy.desktop.session.established",
        EventFields::default()
            .with_operation("proxy_ws_desktop_loop")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_status("connected")
            .with_context("protocol", protocol.label().to_string()),
    );
    let session = Arc::new(DesktopSession::new(
        connection_id,
        entry.generation,
        protocol,
        ready_id,
        owner_geometry,
        enable_drive,
        state.ssh_detach_grace_seconds,
    ));
    state
        .desktop_sessions
        .write()
        .await
        .insert(connection_id, session.clone());
    let Some(epoch) = session.try_begin_operator_attach().await else {
        session.shutdown().await;
        return Err(AppError::Conflict(
            "Desktop session could not accept the operator".to_string(),
        ));
    };
    let ready = upstream_ready(&upstream);
    Ok(DesktopAttach::Established {
        session,
        upstream,
        ready,
        epoch,
    })
}

fn upstream_ready(upstream: &DesktopUpstream) -> GuacdReadyInstruction {
    GuacdReadyInstruction {
        consumed: upstream.ready.consumed,
        args: upstream.ready.args.clone(),
        raw: upstream.ready.raw.clone(),
    }
}

struct VncOwnerConfig {
    host: String,
    port: u16,
    password: Option<String>,
    color_depth: Option<u32>,
    force_lossless: bool,
}

async fn establish_owner_upstream(
    connection_id: Uuid,
    protocol: DesktopProtocol,
    payload: &Value,
    guacd_target: &str,
    metrics: &Metrics,
) -> Result<(DesktopUpstream, bool, DesktopGeometry), AppError> {
    let connect_timeout = extract_connect_timeout_duration(payload)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    match protocol {
        DesktopProtocol::Rdp => {
            let launch = extract_rdp_launch_config(payload)
                .map_err(|error| AppError::Validation(format!("Invalid RDP payload: {error}")))?;
            let options = extract_rdp_client_options(payload)?;
            let rdp_target = format!("{}:{}", launch.host, launch.port);
            let target_latency = probe_rdp_target_latency_logged(
                connection_id,
                &launch,
                &rdp_target,
                connect_timeout,
            )
            .await;
            let stream = connect_upstream(guacd_target, connect_timeout, metrics).await?;
            mark_connection_metrics_connected(metrics, target_latency).await;
            let (mut reader, mut writer) = stream.into_split();
            let mut buffer = vec![0u8; GUAC_READ_BUFFER_SIZE];
            let mut stream_buf = SliceBuffer::new();

            let args = match perform_guacd_handshake(
                metrics,
                &mut reader,
                &mut writer,
                &mut stream_buf,
                &mut buffer,
                connect_timeout,
                protocol.selector(),
            )
            .await
            {
                Ok(args) => args,
                Err(GuacdHandshakeFailure::TimedOut) => {
                    return Err(AppError::External(format!(
                        "guacd handshake timed out after {}ms",
                        connect_timeout.as_millis()
                    )));
                }
                Err(GuacdHandshakeFailure::Failed(error)) => return Err(error),
            };
            if args.is_empty() {
                return Err(AppError::External(
                    "guacd handshake returned empty args".to_string(),
                ));
            }
            let (version, args) = extract_guacd_version(args)?;
            if args.is_empty() {
                return Err(AppError::External(
                    "guacd handshake returned empty parameter list".to_string(),
                ));
            }
            log_empty_rdp_credentials(connection_id, &launch, &options, &rdp_target);
            let owner_geometry = options.geometry;
            let capabilities =
                desktop_client_capabilities(protocol, DesktopRole::Owner, owner_geometry);
            let resolved_security = resolve_rdp_security_mode(options.security.as_deref());
            let drive_base_path = rdp_drive_base_path();
            let connect_ctx = RdpConnectContext {
                resolved_security: &resolved_security,
                drive_base_path: &drive_base_path,
                connection_id,
                version: &version,
            };
            let elements = build_guacd_connect_elements(&args, &launch, &options, &connect_ctx);
            send_client_handshake(&mut writer, metrics, &capabilities, &elements).await?;
            let ready = wait_ready(
                &mut reader,
                &mut stream_buf,
                &mut buffer,
                metrics,
                connect_timeout,
            )
            .await?;
            Ok((
                DesktopUpstream {
                    reader,
                    writer,
                    stream_buf,
                    buffer,
                    ready,
                },
                options.enable_drive,
                owner_geometry,
            ))
        }
        DesktopProtocol::Vnc => {
            let (host, port) = extract_host_port(payload)
                .map_err(|error| AppError::Validation(error.to_string()))?;
            let password = super::super::vnc::vnc_password(payload);
            let options = extract_vnc_client_options(payload)?;
            let stream = connect_upstream(guacd_target, connect_timeout, metrics).await?;
            mark_connection_metrics_connected(metrics, None).await;
            let (mut reader, mut writer) = stream.into_split();
            let mut buffer = vec![0u8; GUAC_READ_BUFFER_SIZE];
            let mut stream_buf = SliceBuffer::new();

            let args = match perform_guacd_handshake(
                metrics,
                &mut reader,
                &mut writer,
                &mut stream_buf,
                &mut buffer,
                connect_timeout,
                protocol.selector(),
            )
            .await
            {
                Ok(args) => args,
                Err(GuacdHandshakeFailure::TimedOut) => {
                    return Err(AppError::External(format!(
                        "guacd handshake timed out after {}ms",
                        connect_timeout.as_millis()
                    )));
                }
                Err(GuacdHandshakeFailure::Failed(error)) => return Err(error),
            };
            if args.is_empty() {
                return Err(AppError::External(
                    "guacd handshake returned empty args".to_string(),
                ));
            }
            let (version, args) = extract_guacd_version(args)?;
            if args.is_empty() {
                return Err(AppError::External(
                    "guacd handshake returned empty parameter list".to_string(),
                ));
            }
            let owner_geometry = DesktopGeometry::default_desktop();
            let capabilities =
                desktop_client_capabilities(protocol, DesktopRole::Owner, owner_geometry);
            let elements = build_vnc_owner_connect_elements(
                &args,
                &version,
                &VncOwnerConfig {
                    host,
                    port,
                    password,
                    color_depth: options.color_depth,
                    force_lossless: options.force_lossless,
                },
            );
            send_client_handshake(&mut writer, metrics, &capabilities, &elements).await?;
            let ready = wait_ready(
                &mut reader,
                &mut stream_buf,
                &mut buffer,
                metrics,
                connect_timeout,
            )
            .await?;
            Ok((
                DesktopUpstream {
                    reader,
                    writer,
                    stream_buf,
                    buffer,
                    ready,
                },
                false,
                owner_geometry,
            ))
        }
    }
}

async fn connect_upstream(
    guacd_target: &str,
    connect_timeout: Duration,
    metrics: &Metrics,
) -> Result<TcpStream, AppError> {
    match tokio::time::timeout(connect_timeout, TcpStream::connect(guacd_target)).await {
        Ok(Ok(stream)) => {
            configure_interactive_tcp_stream(&stream);
            Ok(stream)
        }
        Ok(Err(error)) => {
            mark_connection_metrics_proxy_unhealthy(metrics, None).await;
            Err(AppError::External(format!(
                "Failed to connect to guacd: {error}"
            )))
        }
        Err(_) => {
            mark_connection_metrics_proxy_unhealthy(metrics, None).await;
            Err(AppError::External(format!(
                "Connection to guacd timed out after {}ms",
                connect_timeout.as_millis()
            )))
        }
    }
}

async fn wait_ready<R: AsyncRead + Unpin>(
    reader: &mut R,
    stream_buf: &mut SliceBuffer,
    buffer: &mut [u8],
    metrics: &Metrics,
    connect_timeout: Duration,
) -> Result<GuacdReadyInstruction, AppError> {
    match tokio::time::timeout(
        connect_timeout,
        super::wait_for_guacd_ready(reader, stream_buf, buffer, metrics),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(AppError::External(format!(
            "guacd ready timed out after {}ms",
            connect_timeout.as_millis()
        ))),
    }
}

fn build_join_connect_elements(
    args: &[String],
    version: &str,
    protocol: DesktopProtocol,
    mode: LegMode,
) -> Vec<String> {
    let mut elements = Vec::with_capacity(args.len().saturating_add(2));
    elements.push("connect".to_string());
    elements.push(version.to_string());
    for key in args {
        elements.push(match protocol {
            DesktopProtocol::Rdp => rdp_join_value(key, mode),
            DesktopProtocol::Vnc => vnc_join_value(key, mode),
        });
    }
    elements
}

fn rdp_join_value(key: &str, mode: LegMode) -> String {
    match key {
        "read-only" => mode.read_only().to_string(),
        "width" => mode.size().width().to_string(),
        "height" => mode.size().height().to_string(),
        "dpi" => mode.size().dpi().to_string(),
        // Observation never carries clipboard or file side channels. Native
        // read-only is defence in depth; the proxy handler stays authoritative.
        "disable-copy" | "disable-paste" | "disable-upload" | "disable-download" => {
            if mode.is_observer() {
                "true".to_string()
            } else {
                String::new()
            }
        }
        "enable-drive" | "enable-sftp" | "enable-audio-input" => "false".to_string(),
        _ => String::new(),
    }
}

fn vnc_join_value(key: &str, mode: LegMode) -> String {
    match key {
        "read-only" => mode.read_only().to_string(),
        "disable-copy" | "disable-paste" | "disable-display-resize" => {
            if mode.is_observer() {
                "true".to_string()
            } else {
                String::new()
            }
        }
        "enable-sftp" => "false".to_string(),
        _ => String::new(),
    }
}

fn build_vnc_owner_connect_elements(
    args: &[String],
    version: &str,
    config: &VncOwnerConfig,
) -> Vec<String> {
    let mut elements = Vec::with_capacity(args.len().saturating_add(2));
    elements.push("connect".to_string());
    elements.push(version.to_string());
    for key in args {
        elements.push(match key.as_str() {
            "hostname" | "host" => config.host.clone(),
            "port" => config.port.to_string(),
            "password" => config.password.clone().unwrap_or_default(),
            "color-depth" => config
                .color_depth
                .map(|value| value.to_string())
                .unwrap_or_default(),
            "force-lossless" => config.force_lossless.to_string(),
            "read-only" => "false".to_string(),
            _ => String::new(),
        });
    }
    elements
}

async fn owner_loop(
    session: Arc<DesktopSession>,
    upstream: DesktopUpstream,
    socket: Option<WebSocket>,
    epoch: u64,
    metrics: Metrics,
) {
    let DesktopUpstream {
        mut reader,
        mut writer,
        mut stream_buf,
        mut buffer,
        ..
    } = upstream;

    let outcome = match socket {
        Some(socket) => match owner_attached_phase(
            &session,
            socket,
            &mut reader,
            &mut writer,
            &mut stream_buf,
            &mut buffer,
            epoch,
            &metrics,
        )
        .await
        {
            AttachedOutcome::Detached => {
                session.mark_operator_detached(epoch).await;
                keeper_outcome(
                    owner_keeper_phase(
                        &session,
                        &mut reader,
                        &mut writer,
                        &mut stream_buf,
                        &mut buffer,
                        &metrics,
                    )
                    .await,
                )
            }
            AttachedOutcome::Ended => OwnerOutcome::Ended,
            AttachedOutcome::Error => OwnerOutcome::Error,
        },
        None => keeper_outcome(
            owner_keeper_phase(
                &session,
                &mut reader,
                &mut writer,
                &mut stream_buf,
                &mut buffer,
                &metrics,
            )
            .await,
        ),
    };

    let _ = writer.shutdown().await;
    cleanup_embedded_rdp_drive(session.connection_id, session.enable_drive).await;
    session.mark_ended().await;
    match outcome {
        OwnerOutcome::Error => {
            log_error_event(
                "proxy.desktop.session.failed",
                "proxy_desktop_error",
                "Desktop runtime ended after an upstream or transport failure".to_string(),
                EventFields::default()
                    .with_operation("desktop_owner")
                    .with_resource("proxy_connection", session.connection_id.to_string())
                    .with_context("protocol", session.protocol.label().to_string()),
            );
            mark_connection_metrics_error(&metrics).await;
        }
        OwnerOutcome::Ended => mark_connection_metrics_disconnected(&metrics).await,
    }
}

fn keeper_outcome(outcome: KeeperOutcome) -> OwnerOutcome {
    match outcome {
        KeeperOutcome::Ended => OwnerOutcome::Ended,
        KeeperOutcome::Error => OwnerOutcome::Error,
    }
}

async fn owner_attached_phase(
    session: &Arc<DesktopSession>,
    socket: WebSocket,
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    stream_buf: &mut SliceBuffer,
    buffer: &mut [u8],
    epoch: u64,
    metrics: &Metrics,
) -> AttachedOutcome {
    let (sender, mut receiver) = socket.split();
    let sender: SharedWsSender = Arc::new(Mutex::new(sender));
    let mut ended = session.subscribe_ended();
    let mut keepalive = interval(Duration::from_secs(super::KEEPALIVE_INTERVAL_SECS));
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    keepalive.tick().await;

    loop {
        if *ended.borrow() {
            return AttachedOutcome::Ended;
        }
        // The ready handshake can read the filesystem declaration together
        // with ready. Deliver buffered instructions before waiting for more.
        match forward_operator_instructions(stream_buf, &sender, metrics).await {
            Ok(ForwardStatus::Forwarded) => {}
            Ok(ForwardStatus::BrowserGone) => return AttachedOutcome::Detached,
            Err(_) => return AttachedOutcome::Error,
        }
        tokio::select! {
            _ = ended.changed() => return AttachedOutcome::Ended,
            _ = keepalive.tick() => {
                let keepalive_msg = encode_guac_instruction(&["nop".to_string()]);
                if writer.write_all(&keepalive_msg).await.is_err() {
                    return AttachedOutcome::Error;
                }
                record_connection_traffic(
                    metrics,
                    LinkType::ProxyProtocol,
                    TrafficDirection::Tx,
                    keepalive_msg.len(),
                )
                .await;
            }
            read = reader.read(&mut *buffer) => {
                match read {
                    Ok(0) => {
                        send_leg_error(&sender, "Desktop session closed", GUAC_STATUS_RESOURCE_CLOSED).await;
                        return AttachedOutcome::Ended;
                    }
                    Ok(read) => {
                        stream_buf.extend_from_slice(&buffer[..read]);
                        match forward_operator_instructions(stream_buf, &sender, metrics).await {
                            Ok(ForwardStatus::Forwarded) => {}
                            Ok(ForwardStatus::BrowserGone) => return AttachedOutcome::Detached,
                            Err(_) => {
                                send_leg_error(&sender, "Desktop backend read failed", GUAC_STATUS_UPSTREAM_ERROR).await;
                                return AttachedOutcome::Error;
                            }
                        }
                    }
                    Err(_) => {
                        send_leg_error(&sender, "Desktop backend read failed", GUAC_STATUS_UPSTREAM_ERROR).await;
                        return AttachedOutcome::Error;
                    }
                }
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    return AttachedOutcome::Detached;
                };
                let message = match message {
                    Ok(message) => message,
                    Err(_) => return AttachedOutcome::Detached,
                };
                match message {
                    Message::Text(text) => {
                        if is_guac_internal_ping(text.as_str()) {
                            if sender.lock().await.send(Message::Text(text)).await.is_err() {
                                return AttachedOutcome::Detached;
                            }
                            continue;
                        }
                        if handle_operator_text(text.as_str(), writer, session, epoch, metrics)
                            .await
                            .is_err()
                        {
                            return AttachedOutcome::Error;
                        }
                    }
                    Message::Binary(_) => {
                        send_leg_error(&sender, "Binary websocket frames are not supported", GUAC_STATUS_CLIENT_BAD_TYPE).await;
                        return AttachedOutcome::Detached;
                    }
                    Message::Close(_) => return AttachedOutcome::Detached,
                    Message::Ping(payload) => {
                        if sender.lock().await.send(Message::Pong(payload)).await.is_err() {
                            return AttachedOutcome::Detached;
                        }
                    }
                    Message::Pong(_) => {}
                }
            }
        }
    }
}

async fn owner_keeper_phase(
    session: &Arc<DesktopSession>,
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    stream_buf: &mut SliceBuffer,
    buffer: &mut [u8],
    metrics: &Metrics,
) -> KeeperOutcome {
    let mut ended = session.subscribe_ended();
    let mut keepalive = interval(Duration::from_secs(super::KEEPALIVE_INTERVAL_SECS));
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    keepalive.tick().await;
    let mut grace = interval(Duration::from_secs(1));
    grace.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        if *ended.borrow() {
            return KeeperOutcome::Ended;
        }
        // Process frames left by the handshake or an operator disconnect even
        // when the upstream desktop is otherwise idle.
        if drain_owner_instructions(stream_buf, writer, metrics)
            .await
            .is_err()
        {
            return KeeperOutcome::Error;
        }
        tokio::select! {
            _ = ended.changed() => return KeeperOutcome::Ended,
            _ = keepalive.tick() => {
                let keepalive_msg = encode_guac_instruction(&["nop".to_string()]);
                if writer.write_all(&keepalive_msg).await.is_err() {
                    return KeeperOutcome::Error;
                }
                record_connection_traffic(
                    metrics,
                    LinkType::ProxyProtocol,
                    TrafficDirection::Tx,
                    keepalive_msg.len(),
                )
                .await;
            }
            _ = grace.tick() => {
                if session.reconnect_expired().await {
                    return KeeperOutcome::Ended;
                }
            }
            read = reader.read(&mut *buffer) => {
                match read {
                    Ok(0) => return KeeperOutcome::Ended,
                    Ok(read) => {
                        stream_buf.extend_from_slice(&buffer[..read]);
                        if drain_owner_instructions(stream_buf, writer, metrics).await.is_err() {
                            return KeeperOutcome::Error;
                        }
                    }
                    Err(_) => return KeeperOutcome::Error,
                }
            }
        }
    }
}

async fn relay_join_leg(
    session: Arc<DesktopSession>,
    socket: WebSocket,
    mut reader: OwnedReadHalf,
    mut writer: OwnedWriteHalf,
    mut stream_buf: SliceBuffer,
    mut buffer: Vec<u8>,
    mode: LegMode,
    metrics: Metrics,
    observer_deadline: Option<tokio::time::Instant>,
) -> Result<(), AppError> {
    let (sender, mut receiver) = socket.split();
    let sender: SharedWsSender = Arc::new(Mutex::new(sender));
    let policy = mode
        .is_observer()
        .then(|| Arc::new(Mutex::new(ObserverPolicy::default())));
    let mut ended_tcp = session.subscribe_ended();
    let mut ended_ws = session.subscribe_ended();

    let relay = async {
        tokio::select! {
            result = relay_tcp_to_ws(
                &mut reader,
                &mut stream_buf,
                &mut buffer,
                &sender,
                policy.as_ref(),
                &mut ended_tcp,
                &metrics,
            ) => result,
            result = relay_ws_to_tcp(
                &mut receiver,
                &mut writer,
                &sender,
                policy.as_ref(),
                &session,
                &mut ended_ws,
                &metrics,
                mode,
            ) => result,
        }
    };
    let result = match observer_deadline {
        Some(deadline) => match tokio::time::timeout_at(deadline, relay).await {
            Ok(result) => result,
            Err(_) => {
                let _ = tokio::time::timeout(Duration::from_secs(1), async {
                    let mut guard = sender.lock().await;
                    super::super::observation::send_observer_lease_close(&mut *guard).await;
                })
                .await;
                Ok(())
            }
        },
        None => relay.await,
    };

    let _ = writer.shutdown().await;
    result
}

async fn relay_tcp_to_ws(
    reader: &mut OwnedReadHalf,
    stream_buf: &mut SliceBuffer,
    buffer: &mut [u8],
    sender: &SharedWsSender,
    policy: Option<&Arc<Mutex<ObserverPolicy>>>,
    ended: &mut watch::Receiver<bool>,
    metrics: &Metrics,
) -> Result<(), AppError> {
    loop {
        if *ended.borrow() {
            return Ok(());
        }
        let status = match policy {
            Some(policy) => {
                forward_observer_instructions(stream_buf, sender, policy, metrics).await?
            }
            None => forward_operator_instructions(stream_buf, sender, metrics).await?,
        };
        if matches!(status, ForwardStatus::BrowserGone) {
            return Ok(());
        }

        tokio::select! {
            _ = ended.changed() => return Ok(()),
            read = reader.read(&mut *buffer) => {
                match read {
                    Ok(0) => {
                        send_leg_error(sender, "Desktop session closed", GUAC_STATUS_RESOURCE_CLOSED).await;
                        return Ok(());
                    }
                    Ok(read) => stream_buf.extend_from_slice(&buffer[..read]),
                    Err(error) => {
                        send_leg_error(sender, "Desktop backend read failed", GUAC_STATUS_UPSTREAM_ERROR).await;
                        return Err(AppError::External(format!("guacd TCP read failed: {error}")));
                    }
                }
            }
        }
    }
}

async fn relay_ws_to_tcp(
    receiver: &mut SplitStream<WebSocket>,
    writer: &mut OwnedWriteHalf,
    sender: &SharedWsSender,
    policy: Option<&Arc<Mutex<ObserverPolicy>>>,
    session: &Arc<DesktopSession>,
    ended: &mut watch::Receiver<bool>,
    metrics: &Metrics,
    mode: LegMode,
) -> Result<(), AppError> {
    let mut keepalive = interval(Duration::from_secs(super::KEEPALIVE_INTERVAL_SECS));
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    keepalive.tick().await;

    loop {
        if *ended.borrow() {
            return Ok(());
        }
        tokio::select! {
            _ = ended.changed() => return Ok(()),
            _ = keepalive.tick() => {
                let keepalive_msg = encode_guac_instruction(&["nop".to_string()]);
                writer
                    .write_all(&keepalive_msg)
                    .await
                    .map_err(|error| {
                        AppError::External(format!("guacd TCP write failed (keepalive): {error}"))
                    })?;
                record_connection_traffic(
                    metrics,
                    LinkType::ProxyProtocol,
                    TrafficDirection::Tx,
                    keepalive_msg.len(),
                )
                .await;
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    return Ok(());
                };
                let message = message.map_err(|error| {
                    AppError::External(format!("Client websocket receive failed: {error}"))
                })?;
                match message {
                    Message::Text(text) => {
                        if is_guac_internal_ping(text.as_str()) {
                            let _ = sender.lock().await.send(Message::Text(text)).await;
                            continue;
                        }
                        if mode.is_observer() {
                            // The tunnel's initial connect-data string is not a
                            // Guacamole instruction and is never routed.
                            if !looks_like_guacamole_instruction(text.as_str()) {
                                continue;
                            }
                            let Some(policy) = policy else {
                                return Err(AppError::Forbidden(
                                    "Observer input policy is missing".to_string(),
                                ));
                            };
                            let action = {
                                let mut guard = policy.lock().await;
                                guard.validate_input(text.as_str())
                            };
                            match action {
                                Ok(ObserverInput::Consume) => continue,
                                Ok(ObserverInput::Forward) => {}
                                Err(_) => {
                                    close_leg(sender, OBSERVER_CLOSE_INVALID_INPUT, "Observer input forbidden").await;
                                    return Ok(());
                                }
                            }
                            writer
                                .write_all(text.as_bytes())
                                .await
                                .map_err(|error| {
                                    AppError::External(format!("guacd TCP write failed: {error}"))
                                })?;
                            record_connection_traffic(
                                metrics,
                                LinkType::ClientProxy,
                                TrafficDirection::Tx,
                                text.len(),
                            )
                            .await;
                            record_connection_traffic(
                                metrics,
                                LinkType::ProxyProtocol,
                                TrafficDirection::Tx,
                                text.len(),
                            )
                            .await;
                        } else if let LegMode::Operator { epoch, .. } = mode {
                            handle_operator_text(
                                text.as_str(),
                                writer,
                                session,
                                epoch,
                                metrics,
                            )
                            .await?;
                        }
                    }
                    Message::Binary(_) => {
                        send_leg_error(sender, "Binary websocket frames are not supported", GUAC_STATUS_CLIENT_BAD_TYPE).await;
                        return Ok(());
                    }
                    Message::Close(_) => return Ok(()),
                    Message::Ping(payload) => {
                        let _ = sender.lock().await.send(Message::Pong(payload)).await;
                    }
                    Message::Pong(_) => {}
                }
            }
        }
    }
}

async fn handle_operator_text(
    text: &str,
    writer: &mut OwnedWriteHalf,
    session: &Arc<DesktopSession>,
    epoch: u64,
    metrics: &Metrics,
) -> Result<(), AppError> {
    if is_guac_internal_ping(text) {
        return Ok(());
    }
    if !looks_like_guacamole_instruction(text) {
        return Ok(());
    }
    if matches!(guac_opcode(text), Some("select" | "connect")) {
        return Ok(());
    }
    writer
        .write_all(text.as_bytes())
        .await
        .map_err(|error| AppError::External(format!("guacd TCP write failed: {error}")))?;
    record_connection_traffic(
        metrics,
        LinkType::ClientProxy,
        TrafficDirection::Tx,
        text.len(),
    )
    .await;
    record_connection_traffic(
        metrics,
        LinkType::ProxyProtocol,
        TrafficDirection::Tx,
        text.len(),
    )
    .await;
    session.note_operator_activity(epoch).await;
    Ok(())
}

async fn forward_operator_instructions(
    stream_buf: &mut SliceBuffer,
    sender: &SharedWsSender,
    metrics: &Metrics,
) -> Result<ForwardStatus, AppError> {
    loop {
        let Some((consumed, _elements)) = parse_guac_instruction(stream_buf.available())? else {
            break;
        };
        let raw = stream_buf.take_prefix(consumed);
        let text = String::from_utf8_lossy(raw).to_string();
        stream_buf.consume(consumed);
        if sender
            .lock()
            .await
            .send(Message::Text(text.into()))
            .await
            .is_err()
        {
            return Ok(ForwardStatus::BrowserGone);
        }
        record_connection_traffic(
            metrics,
            LinkType::ProxyProtocol,
            TrafficDirection::Rx,
            consumed,
        )
        .await;
        record_connection_traffic(
            metrics,
            LinkType::ClientProxy,
            TrafficDirection::Rx,
            consumed,
        )
        .await;
    }
    if stream_buf.available().len() > MAX_DESKTOP_INSTRUCTION_BYTES {
        return Err(AppError::External(
            "guacd instruction buffer exceeded".to_string(),
        ));
    }
    Ok(ForwardStatus::Forwarded)
}

async fn forward_observer_instructions(
    stream_buf: &mut SliceBuffer,
    sender: &SharedWsSender,
    policy: &Arc<Mutex<ObserverPolicy>>,
    metrics: &Metrics,
) -> Result<ForwardStatus, AppError> {
    loop {
        let Some((consumed, elements)) = parse_guac_instruction(stream_buf.available())? else {
            break;
        };
        // Do not admit an ACK until its corresponding blob has been sent.
        let mut guard = policy.lock().await;
        let forward = guard.outbound(&elements)?;
        let raw = stream_buf.take_prefix(consumed);
        let text = String::from_utf8_lossy(raw).to_string();
        stream_buf.consume(consumed);
        if forward {
            if sender
                .lock()
                .await
                .send(Message::Text(text.into()))
                .await
                .is_err()
            {
                return Ok(ForwardStatus::BrowserGone);
            }
        }
        drop(guard);
        if forward {
            record_connection_traffic(
                metrics,
                LinkType::ClientProxy,
                TrafficDirection::Rx,
                consumed,
            )
            .await;
        }
        record_connection_traffic(
            metrics,
            LinkType::ProxyProtocol,
            TrafficDirection::Rx,
            consumed,
        )
        .await;
    }
    if stream_buf.available().len() > MAX_DESKTOP_INSTRUCTION_BYTES {
        return Err(AppError::External(
            "guacd instruction buffer exceeded".to_string(),
        ));
    }
    Ok(ForwardStatus::Forwarded)
}

async fn drain_owner_instructions(
    stream_buf: &mut SliceBuffer,
    writer: &mut OwnedWriteHalf,
    metrics: &Metrics,
) -> Result<(), AppError> {
    loop {
        let Some((consumed, elements)) = parse_guac_instruction(stream_buf.available())? else {
            break;
        };
        stream_buf.consume(consumed);
        record_connection_traffic(
            metrics,
            LinkType::ProxyProtocol,
            TrafficDirection::Rx,
            consumed,
        )
        .await;
        // No browser is attached: acknowledge frame boundaries so guacd does
        // not stall the retained connection, and discard every other frame.
        if elements.first().map(String::as_str) == Some("sync")
            && let Some(timestamp) = elements.get(1).filter(|value| value.parse::<u64>().is_ok())
        {
            let response = encode_guac_instruction(&["sync".to_string(), timestamp.clone()]);
            writer.write_all(&response).await.map_err(|error| {
                AppError::External(format!("guacd TCP write failed (sync): {error}"))
            })?;
            record_connection_traffic(
                metrics,
                LinkType::ProxyProtocol,
                TrafficDirection::Tx,
                response.len(),
            )
            .await;
        }
    }
    if stream_buf.available().len() > MAX_DESKTOP_INSTRUCTION_BYTES {
        return Err(AppError::External(
            "guacd instruction buffer exceeded".to_string(),
        ));
    }
    Ok(())
}

async fn send_leg_error(sender: &SharedWsSender, message: &str, status: u16) {
    let bytes =
        encode_guac_instruction(&["error".to_string(), message.to_string(), status.to_string()]);
    let text = String::from_utf8_lossy(&bytes).to_string();
    let mut guard = sender.lock().await;
    let _ = guard.send(Message::Text(text.into())).await;
    let _ = guard.send(Message::Close(None)).await;
}

async fn close_leg(sender: &SharedWsSender, code: u16, reason: &str) {
    let _ = sender
        .lock()
        .await
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

/// Observer output/input boundary.
///
/// Outbound clipboard and file/object streams are suppressed as complete
/// streams: the start instruction, every `blob` and the final `end`. Inbound
/// input is limited to a `sync` whose timestamp was actually emitted to this
/// observer, or a locally consumed success `ack` for a delivered display blob.
///
/// Display streams are not uniformly acknowledged. Lacking
/// `window.createImageBitmap`, guacamole-common-js decodes with
/// `DataURIReader`, whose `blob` handler appends synchronously and sends no
/// `ack`; with the bitmap path it uses `BlobReader`, which acks each blob. Both
/// readers consume every earlier blob before the client answers a server
/// `sync`, and the client flushes the display frame covering every earlier
/// `end` before sending that answer. A matching `sync` therefore retires the
/// credits issued before it was forwarded, including ack-free data. The sync
/// instruction carries no sequence or generation, so the local watermark
/// recorded at forward time is the only safe boundary.
#[derive(Debug, PartialEq, Eq)]
enum ObserverInput {
    Forward,
    Consume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSync {
    timestamp: u64,
    covered_display_blobs: u64,
}

#[derive(Default)]
struct ObserverPolicy {
    pending_sync: Vec<PendingSync>,
    display_streams: HashSet<u64>,
    /// Outstanding display credits per stream, in delivery order.
    display_ack_credits: HashMap<u64, VecDeque<u64>>,
    pending_display_acks: usize,
    display_blobs_issued: u64,
    forbidden_streams: HashSet<u64>,
}

impl ObserverPolicy {
    fn outbound(&mut self, elements: &[String]) -> Result<bool, AppError> {
        let opcode = elements.first().map(String::as_str).unwrap_or_default();
        match opcode {
            "clipboard" => Ok(false),
            "filesystem" => Ok(false),
            "file" | "pipe" | "argv" | "audio" | "video" => {
                if let Some(stream) = index_value(elements, 1) {
                    self.forbid_start(stream)?;
                }
                Ok(false)
            }
            "body" => {
                if let Some(stream) = index_value(elements, 2) {
                    self.forbid_start(stream)?;
                }
                Ok(false)
            }
            "img" => {
                if let Some(stream) = index_value(elements, 1) {
                    self.allow_stream(stream)?;
                }
                Ok(true)
            }
            "blob" | "end" => {
                let Some(stream) = index_value(elements, 1) else {
                    return Ok(false);
                };
                if self.display_streams.contains(&stream) {
                    if opcode == "blob" {
                        if self.pending_display_acks >= MAX_OBSERVER_STREAMS {
                            return Err(AppError::Forbidden(
                                "Observer display acknowledgement limit reached".to_string(),
                            ));
                        }
                        self.display_blobs_issued += 1;
                        self.display_ack_credits
                            .entry(stream)
                            .or_default()
                            .push_back(self.display_blobs_issued);
                        self.pending_display_acks += 1;
                    } else {
                        self.display_streams.remove(&stream);
                    }
                    Ok(true)
                } else {
                    if opcode == "end" {
                        self.forbidden_streams.remove(&stream);
                    } else {
                        self.forbid_stream(stream)?;
                    }
                    Ok(false)
                }
            }
            "ack" | "user" => Ok(false),
            "sync" => {
                let Some(timestamp) = index_value(elements, 1) else {
                    return Err(AppError::Validation(
                        "Desktop sync carried no numeric timestamp".to_string(),
                    ));
                };
                self.remember_sync(timestamp)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn validate_input(&mut self, text: &str) -> Result<ObserverInput, AppError> {
        if text.len() > MAX_OBSERVER_INPUT_BYTES {
            return Err(AppError::Forbidden(
                "Observer input is too large".to_string(),
            ));
        }
        let Some((consumed, elements)) = parse_guac_instruction(text.as_bytes())? else {
            return Err(AppError::Forbidden(
                "Observer input is not a complete instruction".to_string(),
            ));
        };
        if consumed != text.len() {
            return Err(AppError::Forbidden(
                "Observer input must be exactly one instruction".to_string(),
            ));
        }
        match elements.first().map(String::as_str) {
            Some("sync") => {
                if elements.len() != 2 {
                    return Err(AppError::Forbidden(
                        "Observer sync has an invalid shape".to_string(),
                    ));
                }
                let timestamp = index_value(&elements, 1).ok_or_else(|| {
                    AppError::Forbidden("Observer sync timestamp is not numeric".to_string())
                })?;
                let position = self
                    .pending_sync
                    .iter()
                    .position(|pending| pending.timestamp == timestamp)
                    .ok_or_else(|| {
                        AppError::Forbidden(
                            "Observer sync does not match a delivered frame".to_string(),
                        )
                    })?;
                let pending = self.pending_sync.remove(position);
                self.retire_display_credits_through(pending.covered_display_blobs);
                Ok(ObserverInput::Forward)
            }
            Some("ack") => {
                if elements.len() != 4 {
                    return Err(AppError::Forbidden(
                        "Observer ack has an invalid shape".to_string(),
                    ));
                }
                let stream = index_value(&elements, 1).ok_or_else(|| {
                    AppError::Forbidden("Observer ack stream is not numeric".to_string())
                })?;
                if elements[2] != "OK" && !elements[2].is_empty() {
                    return Err(AppError::Forbidden(
                        "Observer ack message is not permitted".to_string(),
                    ));
                }
                if elements[3] != "0" {
                    return Err(AppError::Forbidden(
                        "Observer ack status is not success".to_string(),
                    ));
                }
                let credits = self.display_ack_credits.get_mut(&stream).ok_or_else(|| {
                    AppError::Forbidden("Observer ack has no outstanding display blob".to_string())
                })?;
                // The map only holds non-empty queues, so this consumes the
                // single oldest credit delivered for this stream.
                let _ = credits.pop_front();
                self.pending_display_acks -= 1;
                if credits.is_empty() {
                    self.display_ack_credits.remove(&stream);
                }
                Ok(ObserverInput::Consume)
            }
            _ => Err(AppError::Forbidden(
                "Observer input is not permitted".to_string(),
            )),
        }
    }

    fn remember_sync(&mut self, timestamp: u64) -> Result<(), AppError> {
        if self.pending_sync.len() >= MAX_OBSERVER_PENDING_SYNCS {
            return Err(AppError::Forbidden(
                "Observer is not acknowledging frames".to_string(),
            ));
        }
        self.pending_sync.push(PendingSync {
            timestamp,
            covered_display_blobs: self.display_blobs_issued,
        });
        Ok(())
    }

    /// Retires the display credits issued before the acknowledged sync was
    /// forwarded. Credits issued for frames forwarded after that sync keep
    /// their place until their own ack or a later sync.
    fn retire_display_credits_through(&mut self, watermark: u64) {
        let mut retired = 0;
        self.display_ack_credits.retain(|_, credits| {
            while credits
                .front()
                .is_some_and(|sequence| *sequence <= watermark)
            {
                let _ = credits.pop_front();
                retired += 1;
            }
            !credits.is_empty()
        });
        self.pending_display_acks -= retired;
    }

    fn allow_stream(&mut self, stream: u64) -> Result<(), AppError> {
        if self.display_streams.contains(&stream) || self.forbidden_streams.contains(&stream) {
            return Err(AppError::Forbidden(
                "Observer stream reused before completion".to_string(),
            ));
        }
        if self.display_streams.len() >= MAX_OBSERVER_STREAMS {
            return Err(AppError::Forbidden(
                "Observer display stream limit reached".to_string(),
            ));
        }
        self.display_streams.insert(stream);
        Ok(())
    }

    fn forbid_start(&mut self, stream: u64) -> Result<(), AppError> {
        if self.forbidden_streams.contains(&stream) {
            return Err(AppError::Forbidden(
                "Observer stream reused before completion".to_string(),
            ));
        }
        self.forbid_stream(stream)
    }

    fn forbid_stream(&mut self, stream: u64) -> Result<(), AppError> {
        if self.display_streams.contains(&stream) {
            return Err(AppError::Forbidden(
                "Observer stream reused before completion".to_string(),
            ));
        }
        if !self.forbidden_streams.contains(&stream)
            && self.forbidden_streams.len() >= MAX_OBSERVER_STREAMS
        {
            return Err(AppError::Forbidden(
                "Observer stream limit reached".to_string(),
            ));
        }
        self.forbidden_streams.insert(stream);
        Ok(())
    }
}

fn index_value(elements: &[String], index: usize) -> Option<u64> {
    elements.get(index)?.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_state() -> Arc<ProxyState> {
        Arc::new(ProxyState::new(&crate::config::ProxyConfig::default()).expect("proxy state"))
    }

    fn test_entry() -> ProxyConnection {
        ProxyConnection::new(
            json!({ "connection_type": "rdp" }),
            Arc::new(RwLock::new(ConnectionMetricsState::default())),
        )
    }

    fn test_session(
        connection_id: Uuid,
        generation: Uuid,
        protocol: DesktopProtocol,
        state: DesktopState,
    ) -> Arc<DesktopSession> {
        Arc::new(DesktopSession {
            connection_id,
            generation,
            protocol,
            guacd_connection_id: "test-ready-id".to_string(),
            owner_geometry: DesktopGeometry::default_desktop(),
            enable_drive: false,
            grace_seconds: 60,
            runtime: Mutex::new(DesktopRuntime {
                state,
                operator_attached: state == DesktopState::Live,
                last_operator_activity_at: None,
                detached_at: Some(Utc::now()),
                ended_at: if state == DesktopState::Ended {
                    Some(Utc::now())
                } else {
                    None
                },
            }),
            operator_epoch: AtomicU64::new(0),
            ended_tx: watch::channel(false).0,
            owner_handle: StdMutex::new(None),
        })
    }

    #[tokio::test]
    async fn observer_lease_expiry_cancels_stalled_join_without_ending_desktop() {
        use super::super::super::observation::tests::{WebSocketPair, expect_lease_close};
        let state = test_state();
        let connection_id = Uuid::now_v7();
        let entry = test_entry();
        let generation = entry.generation;
        let metrics = entry.metrics.clone();
        let session = test_session(
            connection_id,
            generation,
            DesktopProtocol::Rdp,
            DesktopState::Live,
        );
        state.connections.write().await.insert(connection_id, entry);
        state
            .desktop_sessions
            .write()
            .await
            .insert(connection_id, session.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let guacd_target = listener.local_addr().unwrap().to_string();
        let WebSocketPair {
            server_socket,
            mut client_socket,
        } = WebSocketPair::open().await;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);

        // The real JOIN connects to this peer and waits for its unanswered args.
        // Hold the peer open until the observer receives the lease close.
        let payload = Value::Null;
        let observe = session.run_leg(
            server_socket,
            &payload,
            LegMode::Observer {
                size: DesktopGeometry::default_desktop(),
            },
            &guacd_target,
            metrics,
            Some(deadline),
        );
        let peer = async {
            let (stream, _) = listener.accept().await.unwrap();
            expect_lease_close(&mut client_socket).await;
            drop(stream);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(observe, peer)
        })
        .await
        .expect("bounded stalled JOIN");
        result.expect("lease expiry is not a target failure");
        assert_eq!(session.generation(), generation);
        let presence = session.runtime_presence().await;
        assert_eq!(presence.state, "live");
        assert_eq!(presence.operator_attached, Some(true));
        assert_eq!(presence.ended_at, None);
        assert!(!*session.ended_tx.borrow());
        assert!(
            state
                .desktop_sessions
                .read()
                .await
                .contains_key(&connection_id)
        );
    }

    #[test]
    fn observer_outbound_suppresses_complete_file_and_clipboard_streams() {
        let mut policy = ObserverPolicy::default();
        assert!(
            !policy
                .outbound(&instruction(&["clipboard", "text/plain", "secret"]))
                .expect("clipboard")
        );
        assert!(
            !policy
                .outbound(&instruction(&["filesystem", "3", "Piping"]))
                .expect("filesystem")
        );
        assert!(
            !policy
                .outbound(&instruction(&["file", "7", "text/plain", "a.txt"]))
                .expect("file")
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "7", "data"]))
                .expect("file blob")
        );
        assert!(
            !policy
                .outbound(&instruction(&["end", "7"]))
                .expect("file end")
        );
        assert!(
            !policy
                .outbound(&instruction(&["body", "3", "9", "text/plain", "b.txt"]))
                .expect("body")
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "9", "data"]))
                .expect("body blob")
        );
        assert!(
            !policy
                .outbound(&instruction(&["end", "9"]))
                .expect("body end")
        );
        assert!(
            !policy
                .outbound(&instruction(&["audio", "11", "audio/L16", "a"]))
                .expect("audio")
        );
        assert!(
            !policy
                .outbound(&instruction(&["video", "12", "0", "video/mp4", "v"]))
                .expect("video")
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "11", "data"]))
                .expect("audio blob")
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "99", "unknown"]))
                .expect("unknown blob")
        );
        assert!(
            !policy
                .outbound(&instruction(&["end", "99"]))
                .expect("unknown end")
        );
    }

    #[test]
    fn observer_outbound_forwards_display_streams() {
        let mut policy = ObserverPolicy::default();
        assert!(
            policy
                .outbound(&instruction(&["img", "5", "0", "0", "0", "0", "10", "10"]))
                .expect("img")
        );
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .expect("display blob")
        );
        assert!(
            policy
                .outbound(&instruction(&["end", "5"]))
                .expect("display end")
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "5", "after end"]))
                .unwrap()
        );
    }

    #[test]
    fn observer_sync_must_match_a_timestamp_emitted_to_that_observer() {
        let mut policy = ObserverPolicy::default();
        assert!(
            policy
                .outbound(&instruction(&["sync", "1000"]))
                .expect("sync out")
        );
        assert!(
            policy
                .outbound(&instruction(&["sync", "2000"]))
                .expect("sync out")
        );
        assert_eq!(
            policy.validate_input("4.sync,4.1000;").unwrap(),
            ObserverInput::Forward
        );
        assert!(policy.validate_input("4.sync,4.1000;").is_err());
        assert!(policy.validate_input("4.sync,4.3000;").is_err());
        assert!(policy.validate_input("4.sync,4.2000;").is_ok());
        assert!(policy.validate_input("4.sync,4.2000;").is_err());
    }

    #[test]
    fn observer_ack_is_limited_to_outstanding_display_blobs() {
        let mut policy = ObserverPolicy::default();
        assert!(
            policy
                .outbound(&instruction(&["img", "5", "0", "0", "0", "0", "10", "10"]))
                .expect("img")
        );
        assert!(policy.validate_input("3.ack,1.5,0.,1.0;").is_err());
        assert!(policy.outbound(&instruction(&["img", "5"])).is_err());
        assert!(policy.outbound(&instruction(&["file", "5"])).is_err());
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .unwrap()
        );
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .unwrap()
        );
        assert!(policy.outbound(&instruction(&["end", "5"])).unwrap());
        assert!(policy.validate_input("3.ack,1.5,3.bad,1.0;").is_err());
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.1;").is_err());
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.x;").is_err());
        assert!(policy.validate_input("3.ack,2.99,0.,1.0;").is_err());
        assert_eq!(
            policy.validate_input("3.ack,1.5,2.OK,1.0;").unwrap(),
            ObserverInput::Consume
        );
        assert!(
            !policy
                .outbound(&instruction(&["file", "7", "text/plain", "a"]))
                .expect("file")
        );
        assert!(policy.validate_input("3.ack,1.7,0.,1.0;").is_err());
        assert!(policy.validate_input("3.ack,1.5,0.,1.0;").is_ok());
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.0;").is_err());
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.0;").is_err());
    }

    #[test]
    fn observer_reused_image_id_accepts_late_acks_locally() {
        let mut policy = ObserverPolicy::default();
        for _ in 0..2 {
            assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
            assert!(
                policy
                    .outbound(&instruction(&["blob", "5", "pixels"]))
                    .unwrap()
            );
            assert!(policy.outbound(&instruction(&["end", "5"])).unwrap());
        }
        for _ in 0..2 {
            assert_eq!(
                policy.validate_input("3.ack,1.5,2.OK,1.0;").unwrap(),
                ObserverInput::Consume
            );
        }
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.0;").is_err());
    }

    #[test]
    fn observer_late_image_ack_does_not_expose_reused_file_stream() {
        let mut policy = ObserverPolicy::default();
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .unwrap()
        );
        assert!(policy.outbound(&instruction(&["end", "5"])).unwrap());
        assert!(
            !policy
                .outbound(&instruction(&["file", "5", "text/plain", "secret"]))
                .unwrap()
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "5", "secret"]))
                .unwrap()
        );
        assert_eq!(
            policy.validate_input("3.ack,1.5,2.OK,1.0;").unwrap(),
            ObserverInput::Consume
        );
        assert!(
            !policy
                .outbound(&instruction(&["blob", "5", "secret"]))
                .unwrap()
        );
        assert!(!policy.outbound(&instruction(&["end", "5"])).unwrap());
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.0;").is_err());
    }

    #[test]
    fn observer_display_ack_state_is_bounded_and_independent_per_stream() {
        let mut policy = ObserverPolicy::default();
        for id in 0..2 {
            assert!(
                policy
                    .outbound(&instruction(&["img", &id.to_string()]))
                    .unwrap()
            );
        }
        for _ in 0..MAX_OBSERVER_STREAMS {
            assert!(
                policy
                    .outbound(&instruction(&["blob", "0", "pixels"]))
                    .unwrap()
            );
        }
        assert!(
            policy
                .outbound(&instruction(&["blob", "1", "pixels"]))
                .is_err()
        );
        assert!(policy.validate_input("3.ack,1.1,2.OK,1.0;").is_err());
        assert!(policy.validate_input("3.ack,1.0,2.OK,1.0;").is_ok());
        assert!(
            policy
                .outbound(&instruction(&["blob", "1", "pixels"]))
                .unwrap()
        );
        assert!(policy.outbound(&instruction(&["end", "1"])).unwrap());
        assert!(policy.validate_input("3.ack,1.1,2.OK,1.0;").is_ok());
        assert!(policy.validate_input("3.ack,1.1,2.OK,1.0;").is_err());
    }

    #[test]
    fn observer_sync_retires_ack_free_display_frames_with_bounded_state() {
        let mut policy = ObserverPolicy::default();
        for frame in 1..=MAX_OBSERVER_STREAMS + 64 {
            let timestamp = frame as u64;
            let timestamp_text = timestamp.to_string();
            assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
            assert!(
                policy
                    .outbound(&instruction(&["blob", "5", "pixels"]))
                    .unwrap()
            );
            assert!(policy.outbound(&instruction(&["end", "5"])).unwrap());
            assert!(
                policy
                    .outbound(&instruction(&["sync", &timestamp_text]))
                    .unwrap()
            );
            assert_eq!(
                policy.validate_input(&sync_input(timestamp)).unwrap(),
                ObserverInput::Forward
            );
            assert_eq!(policy.pending_display_acks, 0);
            assert!(policy.display_ack_credits.is_empty());
        }
        assert!(policy.pending_sync.is_empty());
    }

    #[test]
    fn observer_late_sync_keeps_credits_for_frames_delivered_after_it() {
        let mut policy = ObserverPolicy::default();
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "first"]))
                .unwrap()
        );
        assert!(policy.outbound(&instruction(&["sync", "1000"])).unwrap());
        assert!(policy.outbound(&instruction(&["img", "6"])).unwrap());
        assert!(
            policy
                .outbound(&instruction(&["blob", "6", "second"]))
                .unwrap()
        );
        assert_eq!(policy.pending_display_acks, 2);

        assert_eq!(
            policy.validate_input(&sync_input(1000)).unwrap(),
            ObserverInput::Forward
        );
        assert_eq!(policy.pending_display_acks, 1);
        assert_eq!(
            policy.display_ack_credits.get(&6).map(VecDeque::len),
            Some(1)
        );
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.0;").is_err());
        assert_eq!(
            policy.validate_input("3.ack,1.6,2.OK,1.0;").unwrap(),
            ObserverInput::Consume
        );
        assert_eq!(policy.pending_display_acks, 0);
    }

    #[test]
    fn observer_blob_ack_and_sync_consume_each_display_credit_once() {
        let mut policy = ObserverPolicy::default();
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(policy.outbound(&instruction(&["img", "6"])).unwrap());
        assert!(policy.outbound(&instruction(&["blob", "5", "a"])).unwrap());
        assert!(policy.outbound(&instruction(&["blob", "6", "b"])).unwrap());
        assert!(policy.outbound(&instruction(&["sync", "1000"])).unwrap());
        assert!(policy.outbound(&instruction(&["blob", "6", "c"])).unwrap());
        assert_eq!(policy.pending_display_acks, 3);

        assert_eq!(
            policy.validate_input("3.ack,1.5,0.,1.0;").unwrap(),
            ObserverInput::Consume
        );
        assert_eq!(policy.pending_display_acks, 2);
        assert_eq!(
            policy.validate_input(&sync_input(1000)).unwrap(),
            ObserverInput::Forward
        );
        assert_eq!(policy.pending_display_acks, 1);
        assert!(policy.validate_input("3.ack,1.5,2.OK,1.0;").is_err());
        assert_eq!(
            policy.validate_input("3.ack,1.6,2.OK,1.0;").unwrap(),
            ObserverInput::Consume
        );
        assert!(policy.validate_input("3.ack,1.6,2.OK,1.0;").is_err());
        assert_eq!(policy.pending_display_acks, 0);
    }

    #[test]
    fn observer_sync_retirement_keeps_reused_and_forbidden_streams_separate() {
        let mut policy = ObserverPolicy::default();
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .unwrap()
        );
        assert!(policy.outbound(&instruction(&["end", "5"])).unwrap());
        assert!(policy.outbound(&instruction(&["sync", "1000"])).unwrap());
        assert_eq!(
            policy.validate_input(&sync_input(1000)).unwrap(),
            ObserverInput::Forward
        );

        // The ended display ID is reusable once its credit was retired.
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .unwrap()
        );
        assert_eq!(
            policy.validate_input("3.ack,1.5,2.OK,1.0;").unwrap(),
            ObserverInput::Consume
        );
        assert!(policy.outbound(&instruction(&["end", "5"])).unwrap());

        // A forbidden file stream keeps the ID suppressed until its own end.
        assert!(
            !policy
                .outbound(&instruction(&["file", "5", "text/plain", "secret"]))
                .unwrap()
        );
        assert!(policy.outbound(&instruction(&["img", "5"])).is_err());
        assert!(
            !policy
                .outbound(&instruction(&["blob", "5", "secret"]))
                .unwrap()
        );
        assert!(!policy.outbound(&instruction(&["end", "5"])).unwrap());
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
    }

    #[test]
    fn observer_forged_sync_cannot_retire_display_credits() {
        let mut policy = ObserverPolicy::default();
        assert!(policy.outbound(&instruction(&["img", "5"])).unwrap());
        assert!(
            policy
                .outbound(&instruction(&["blob", "5", "pixels"]))
                .unwrap()
        );
        for forged in [
            "4.sync,4.9999;",
            "4.sync,1.0;",
            "4.sync,2.10,1.1;",
            "4.sync,4.1000,1.1;",
        ] {
            assert!(policy.validate_input(forged).is_err(), "{forged}");
        }
        assert_eq!(policy.pending_display_acks, 1);

        assert!(policy.outbound(&instruction(&["sync", "1000"])).unwrap());
        assert_eq!(
            policy.validate_input(&sync_input(1000)).unwrap(),
            ObserverInput::Forward
        );
        assert_eq!(policy.pending_display_acks, 0);
        assert!(policy.display_ack_credits.is_empty());
    }

    #[test]
    fn observer_unanswered_syncs_remain_bounded() {
        let mut policy = ObserverPolicy::default();
        for timestamp in 0..MAX_OBSERVER_PENDING_SYNCS as u64 {
            let timestamp = timestamp.to_string();
            assert!(policy.outbound(&instruction(&["sync", &timestamp])).is_ok());
        }
        assert!(policy.outbound(&instruction(&["sync", "999999"])).is_err());
    }

    #[test]
    fn observer_input_rejects_every_non_sync_or_ack_instruction() {
        let mut policy = ObserverPolicy::default();
        for input in [
            "3.key,1.1,1.65;",
            "4.mouse,1.1,1.2,1.4;",
            "4.size,4.1024,3.768,2.96;",
            "7.clipboard,1.a;",
            "4.file,1.1,4.text,1.a;",
            "3.get,1.0,1./;",
            "3.put,1.0,1.1,10.text/plain,5.a.txt;",
            "6.select,3.rdp;",
            "7.connect,5.1_6_0;",
            "6.resize,4.1024,3.768;",
            "3.ack,1.5,0.,1.0;3.ack,1.5,0.,1.0;",
            "2.5.1000;",
        ] {
            assert!(
                policy.validate_input(input).is_err(),
                "{input} must be rejected"
            );
        }
        assert!(
            policy
                .validate_input(&format!("2.5.{};", "9".repeat(MAX_OBSERVER_INPUT_BYTES)))
                .is_err()
        );
    }

    #[test]
    fn rdp_join_connect_values_never_carry_target_credentials() {
        let args = vec![
            "VERSION_1_6_0".to_string(),
            "hostname".to_string(),
            "port".to_string(),
            "username".to_string(),
            "password".to_string(),
            "domain".to_string(),
            "read-only".to_string(),
            "width".to_string(),
            "height".to_string(),
            "dpi".to_string(),
            "enable-drive".to_string(),
            "enable-sftp".to_string(),
        ];
        let (version, args) = extract_guacd_version(args).expect("native handshake version");
        let observer_size = DesktopGeometry::new(1280, 720, 96).expect("observer geometry");
        let observer = build_join_connect_elements(
            &args,
            &version,
            DesktopProtocol::Rdp,
            LegMode::Observer {
                size: observer_size,
            },
        );
        assert_eq!(observer[0], "connect");
        assert_eq!(observer[1], "VERSION_1_6_0");
        assert_eq!(observer[2], "");
        assert_eq!(observer[3], "");
        assert_eq!(observer[4], "");
        assert_eq!(observer[5], "");
        assert_eq!(observer[6], "");
        assert_eq!(observer[7], "true");
        assert_eq!(observer[8], "1280");
        assert_eq!(observer[9], "720");
        assert_eq!(observer[10], "96");
        assert_eq!(observer[11], "false");
        assert_eq!(observer[12], "false");

        let operator = build_join_connect_elements(
            &args,
            &version,
            DesktopProtocol::Rdp,
            LegMode::Operator {
                epoch: 1,
                size: DesktopGeometry::new(1280, 720, 96).expect("operator geometry"),
            },
        );
        assert_eq!(operator[7], "false");
        assert_eq!(operator[8], "1280");
        assert_eq!(operator[9], "720");
        assert_eq!(operator[10], "96");
    }

    #[test]
    fn vnc_observer_join_is_read_only_and_disables_side_channels() {
        let args = vec![
            "VERSION_1_6_0".to_string(),
            "hostname".to_string(),
            "port".to_string(),
            "password".to_string(),
            "read-only".to_string(),
            "disable-copy".to_string(),
            "disable-paste".to_string(),
            "disable-display-resize".to_string(),
            "enable-sftp".to_string(),
        ];
        let (version, args) = extract_guacd_version(args).expect("native handshake version");
        let values = build_join_connect_elements(
            &args,
            &version,
            DesktopProtocol::Vnc,
            LegMode::Observer {
                size: DesktopGeometry::default_desktop(),
            },
        );
        assert_eq!(values[2], "");
        assert_eq!(values[3], "");
        assert_eq!(values[4], "");
        assert_eq!(values[5], "true");
        assert_eq!(values[6], "true");
        assert_eq!(values[7], "true");
        assert_eq!(values[8], "true");
        assert_eq!(values[9], "false");
    }

    #[tokio::test]
    async fn capability_handshake_serializes_nonzero_size_for_every_role_and_protocol() {
        let metrics = test_metrics();
        let owner_geometry = DesktopGeometry::new(1920, 1080, 120).expect("owner geometry");
        let operator_geometry = DesktopGeometry::new(1280, 720, 96).expect("operator geometry");
        let cases = [
            (DesktopProtocol::Rdp, DesktopRole::Owner, owner_geometry),
            (
                DesktopProtocol::Vnc,
                DesktopRole::Owner,
                DesktopGeometry::default_desktop(),
            ),
            (
                DesktopProtocol::Rdp,
                DesktopRole::Operator,
                operator_geometry,
            ),
            (DesktopProtocol::Vnc, DesktopRole::Operator, owner_geometry),
            (DesktopProtocol::Rdp, DesktopRole::Observer, owner_geometry),
            (DesktopProtocol::Vnc, DesktopRole::Observer, owner_geometry),
        ];

        for (protocol, role, size) in cases {
            let capabilities = desktop_client_capabilities(protocol, role, size);
            let connect = vec!["connect".to_string(), "VERSION_1_6_0".to_string()];
            let mut out = Vec::new();
            send_client_handshake(&mut out, &metrics, &capabilities, &connect)
                .await
                .expect("serialized handshake");
            let instructions = parse_instructions(&out);

            assert_eq!(
                instructions[0],
                vec![
                    "size".to_string(),
                    size.width().to_string(),
                    size.height().to_string(),
                    size.dpi().to_string(),
                ],
                "{protocol:?}/{role:?} must advertise a non-zero size"
            );
            assert!(size.width() > 0 && size.height() > 0 && size.dpi() > 0);
            assert_eq!(instructions[1], vec!["audio".to_string()]);
            assert_eq!(instructions[2], vec!["video".to_string()]);
            assert_eq!(
                instructions[3],
                vec![
                    "image".to_string(),
                    "image/png".to_string(),
                    "image/jpeg".to_string()
                ]
            );
            assert_eq!(
                instructions.last().map(|elements| elements[0].as_str()),
                Some("connect"),
                "{protocol:?}/{role:?} must connect after its capabilities"
            );
        }
    }

    #[tokio::test]
    async fn observer_join_handshake_is_read_only_and_uses_retained_geometry() {
        let metrics = test_metrics();
        let owner_geometry = DesktopGeometry::new(1600, 900, 120).expect("owner geometry");
        let cases = [
            (
                DesktopProtocol::Rdp,
                vec![
                    "VERSION_1_6_0",
                    "hostname",
                    "port",
                    "username",
                    "password",
                    "read-only",
                    "width",
                    "height",
                    "dpi",
                    "enable-sftp",
                ],
                None,
            ),
            (
                DesktopProtocol::Vnc,
                vec![
                    "VERSION_1_6_0",
                    "hostname",
                    "port",
                    "password",
                    "read-only",
                    "disable-display-resize",
                    "enable-sftp",
                ],
                Some("disable-display-resize"),
            ),
        ];

        for (protocol, raw_args, observer_only_key) in cases {
            let (version, args) =
                extract_guacd_version(raw_args.into_iter().map(str::to_string).collect())
                    .expect("native handshake version");
            let mode = LegMode::Observer {
                size: owner_geometry,
            };
            let capabilities = mode.capabilities(protocol);
            let elements = build_join_connect_elements(&args, &version, protocol, mode);
            let mut out = Vec::new();
            send_client_handshake(&mut out, &metrics, &capabilities, &elements)
                .await
                .expect("observer handshake");
            let instructions = parse_instructions(&out);

            assert_eq!(
                instructions[0],
                vec![
                    "size".to_string(),
                    owner_geometry.width().to_string(),
                    owner_geometry.height().to_string(),
                    owner_geometry.dpi().to_string(),
                ]
            );
            let connect = instructions
                .iter()
                .find(|elements| elements[0] == "connect")
                .expect("connect after capabilities");
            let read_only_index = args
                .iter()
                .position(|key| key == "read-only")
                .map(|index| index + 2)
                .expect("read-only parameter");
            assert_eq!(connect[read_only_index], "true");
            if let Some(key) = observer_only_key {
                let index = args
                    .iter()
                    .position(|argument| argument == key)
                    .map(|index| index + 2)
                    .expect("observer-only parameter");
                assert_eq!(connect[index], "true");
            }
            assert_eq!(connect[connect.len() - 1], "false");
        }
    }

    #[test]
    fn vnc_owner_connect_applies_quality_options() {
        let (version, args) = extract_guacd_version(
            vec![
                "VERSION_1_6_0",
                "hostname",
                "port",
                "password",
                "color-depth",
                "force-lossless",
                "read-only",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        )
        .expect("native handshake version");
        let elements = build_vnc_owner_connect_elements(
            &args,
            &version,
            &VncOwnerConfig {
                host: "vnc.local".to_string(),
                port: 5900,
                password: Some("secret".to_string()),
                color_depth: Some(16),
                force_lossless: true,
            },
        );
        assert_eq!(elements[0], "connect");
        assert_eq!(elements[1], "VERSION_1_6_0");
        let value = |key: &str| {
            let index = args
                .iter()
                .position(|argument| argument == key)
                .map(|index| index + 2)
                .expect("parameter");
            elements[index].clone()
        };
        assert_eq!(value("hostname"), "vnc.local");
        assert_eq!(value("port"), "5900");
        assert_eq!(value("password"), "secret");
        assert_eq!(value("color-depth"), "16");
        assert_eq!(value("force-lossless"), "true");
        assert_eq!(value("read-only"), "false");
    }

    #[test]
    fn observer_input_policy_rejects_runtime_size_after_the_handshake_capability() {
        let mut policy = ObserverPolicy::default();
        assert!(policy.validate_input("4.size,4.1600,3.900,3.120;").is_err());
        assert!(policy.validate_input("6.resize,4.1600,3.900;").is_err());
    }

    #[tokio::test]
    async fn observer_resolution_accepts_detached_session_with_matching_generation() {
        let state = test_state();
        let connection_id = Uuid::now_v7();
        let entry = test_entry();
        let generation = entry.generation;
        state.connections.write().await.insert(connection_id, entry);
        state.desktop_sessions.write().await.insert(
            connection_id,
            test_session(
                connection_id,
                generation,
                DesktopProtocol::Rdp,
                DesktopState::Detached,
            ),
        );

        assert!(
            resolve_observer_session(&state, connection_id, generation)
                .await
                .is_ok()
        );
        assert!(
            resolve_observer_session(&state, connection_id, Uuid::now_v7())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn observer_resolution_rejects_a_session_recreated_under_a_new_generation() {
        let state = test_state();
        let connection_id = Uuid::now_v7();
        let old_entry = test_entry();
        let old_generation = old_entry.generation;
        state
            .connections
            .write()
            .await
            .insert(connection_id, old_entry);
        state.desktop_sessions.write().await.insert(
            connection_id,
            test_session(
                connection_id,
                old_generation,
                DesktopProtocol::Rdp,
                DesktopState::Live,
            ),
        );

        // Delete/recreate before the observer loads the runtime.
        state.connections.write().await.remove(&connection_id);
        let new_entry = test_entry();
        let new_generation = new_entry.generation;
        state
            .connections
            .write()
            .await
            .insert(connection_id, new_entry);
        state.desktop_sessions.write().await.insert(
            connection_id,
            test_session(
                connection_id,
                new_generation,
                DesktopProtocol::Rdp,
                DesktopState::Live,
            ),
        );

        assert!(
            resolve_observer_session(&state, connection_id, old_generation)
                .await
                .is_err()
        );
        assert!(
            resolve_observer_session(&state, connection_id, new_generation)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn ended_session_is_not_restarted_under_the_same_connection_generation() {
        let state = test_state();
        let connection_id = Uuid::now_v7();
        let entry = test_entry();
        let generation = entry.generation;
        state
            .connections
            .write()
            .await
            .insert(connection_id, entry.clone());
        state.desktop_sessions.write().await.insert(
            connection_id,
            test_session(
                connection_id,
                generation,
                DesktopProtocol::Rdp,
                DesktopState::Ended,
            ),
        );

        let result =
            existing_session_for_attach(&state, connection_id, &entry, DesktopProtocol::Rdp).await;
        assert!(matches!(result, Err(AppError::NotFound(_))));
    }

    #[tokio::test]
    async fn concurrent_operator_attach_is_rejected_and_stale_detach_is_ignored() {
        let connection_id = Uuid::now_v7();
        let generation = Uuid::now_v7();
        let session = test_session(
            connection_id,
            generation,
            DesktopProtocol::Rdp,
            DesktopState::Detached,
        );
        let first = session
            .try_begin_operator_attach()
            .await
            .expect("first attach");
        assert!(session.try_begin_operator_attach().await.is_none());

        session.mark_operator_detached(first).await;
        assert!(!session.runtime.lock().await.operator_attached);

        let second = session
            .try_begin_operator_attach()
            .await
            .expect("second attach");
        assert_ne!(first, second);
        // A late drop of the previous leg must not clear the new attachment.
        session.mark_operator_detached(first).await;
        assert!(session.runtime.lock().await.operator_attached);
        assert_eq!(session.runtime.lock().await.state, DesktopState::Live);
    }

    #[tokio::test]
    async fn runtime_presence_reports_detached_deadline_and_terminal_end() {
        let connection_id = Uuid::now_v7();
        let generation = Uuid::now_v7();
        let session = test_session(
            connection_id,
            generation,
            DesktopProtocol::Rdp,
            DesktopState::Detached,
        );
        let presence = session.runtime_presence().await;
        assert_eq!(presence.state, "detached");
        assert_eq!(presence.operator_attached, Some(false));
        assert!(presence.reconnect_deadline.is_some());
        assert!(presence.ended_at.is_none());

        session.mark_ended().await;
        let presence = session.runtime_presence().await;
        assert_eq!(presence.state, "ended");
        assert!(presence.reconnect_deadline.is_none());
        assert!(presence.ended_at.is_some());
    }

    fn instruction(elements: &[&str]) -> Vec<String> {
        elements.iter().map(|value| (*value).to_string()).collect()
    }

    fn test_metrics() -> Metrics {
        Arc::new(RwLock::new(ConnectionMetricsState::default()))
    }

    fn parse_instructions(bytes: &[u8]) -> Vec<Vec<String>> {
        let mut instructions = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let (consumed, elements) = parse_guac_instruction(&bytes[offset..])
                .expect("valid instruction")
                .expect("complete instruction");
            offset += consumed;
            instructions.push(elements);
        }
        instructions
    }

    fn sync_input(timestamp: u64) -> String {
        let timestamp = timestamp.to_string();
        format!("4.sync,{}.{timestamp};", timestamp.len())
    }
}
