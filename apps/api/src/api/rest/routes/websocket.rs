use std::future::{Future, pending};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use axum::{
    Extension,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message as ProxyMessage;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use uuid::Uuid;

use crate::api::connections_helpers::{ConnectionServiceType, build_connection_service};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{Path, Query};
use crate::api::rest::request_context::{RequestContextExtractor, build_request_context};
use crate::app::AppState;
use crate::config::effective_proxy_base_url;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::connections::domain::{
    ConnectionProtocol, ConnectionSession, ConnectionStatus,
};
use crate::contexts::playbooks::application::RunStreamFrame;
use crate::contexts::playbooks::domain::PlaybookRun;
use crate::errors::AppError;
use crate::integrations::proxy::{build_proxy_ws_url, create_proxy_ws_token};
use crate::observability::{
    EventFields, instrument_span, log_connection_event, log_error_event, operation_span,
};
use crate::proxy::RdpDisplayParams;

const SESSION_TOUCH_THROTTLE_MS: i64 = 1_000;
const RDP_WS_MIN_WIDTH: u32 = 1;
const RDP_WS_MAX_WIDTH: u32 = 7_680;
const RDP_WS_MIN_HEIGHT: u32 = 1;
const RDP_WS_MAX_HEIGHT: u32 = 4_320;
const RDP_WS_MIN_DPI: u32 = 1;
const RDP_WS_MAX_DPI: u32 = 600;

#[derive(Debug, Default, Deserialize)]
pub struct RdpWsDisplayQuery {
    width: Option<u32>,
    height: Option<u32>,
    dpi: Option<u32>,
}

pub async fn ws_ssh(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    ws: WebSocketUpgrade,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_ws_upgrade(
        state,
        WsUpgradeRequest {
            session_id,
            expected_protocol: ConnectionProtocol::Ssh,
            protocol_label: "ssh",
            proxy_path: "terminal",
            rdp_display_params: None,
        },
        ws,
        build_request_context(&request_id, auth_context, &headers),
    )
    .await
}

pub async fn ws_rdp(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Query(display_query): Query<RdpWsDisplayQuery>,
    ws: WebSocketUpgrade,
    RequestContextExtractor(request_context): RequestContextExtractor,
) -> Response {
    let display_params = match normalize_rdp_ws_display_query(display_query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    handle_ws_upgrade(
        state,
        WsUpgradeRequest {
            session_id,
            expected_protocol: ConnectionProtocol::Rdp,
            protocol_label: "rdp",
            proxy_path: "rdp",
            rdp_display_params: display_params,
        },
        ws,
        request_context,
    )
    .await
    .into_response()
}

pub async fn ws_vnc(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    ws: WebSocketUpgrade,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    handle_ws_upgrade(
        state,
        WsUpgradeRequest {
            session_id,
            expected_protocol: ConnectionProtocol::Vnc,
            protocol_label: "vnc",
            proxy_path: "vnc",
            rdp_display_params: None,
        },
        ws,
        build_request_context(&request_id, auth_context, &headers),
    )
    .await
}

/// Page size for replaying persisted run events on connect. Matches the
/// service's max page size (`PlaybookService::list_run_events` clamps `limit` to
/// 1000), so a full page means "there may be more" and a short page means we
/// reached the tail. Replay pages through the whole history at increasing offset
/// — a run can emit far more than one page of ansible events, so a single capped
/// query would silently drop events `1001..N`.
const PLAYBOOK_RUN_REPLAY_PAGE_SIZE: u32 = 1_000;

/// Safety cap on replay pages to bound the loop against a pathological run (1M
/// events). At [`PLAYBOOK_RUN_REPLAY_PAGE_SIZE`] per page this replays up to one
/// million events before falling through to live forwarding; live frames still
/// arrive via the broadcaster and any gap is recoverable through REST history.
const PLAYBOOK_RUN_REPLAY_MAX_PAGES: usize = 1_000;

/// Live + replay stream for a single playbook run.
///
/// Auth mirrors `ws_ssh` exactly: the same cookie/session-derived
/// `RequestContext` (built by `build_request_context` from the request id,
/// `AuthContext` extension, and headers) authenticates the user. Authorization
/// is IDENTICAL to the REST run-read path: `Read` on `ResourceType::PlaybookRun`
/// keyed by the bare `run_id`, via the shared policy-evaluation service, so the
/// live stream and the REST polling fallback stay strictly coupled. On success
/// the socket replays the persisted `playbook_run_event` rows through the
/// repository (handlers/tasks don't run raw SQL) and forwards live frames from
/// the broadcaster.
pub async fn ws_playbook_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    ws: WebSocketUpgrade,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let correlation_id = request_context.correlation_id.clone();
    let run = match authorize_playbook_run_stream(&state, run_id, &request_context).await {
        Ok(run) => run,
        Err(error) => return handle_app_error(error),
    };
    ws.on_upgrade(move |socket| handle_ws_playbook_run(socket, state, run, correlation_id))
}

/// Authenticate (session/cookie, same path as `ws_ssh`) and authorize the live
/// run stream IDENTICALLY to the REST run-read path so the WS stream and the
/// REST polling fallback are strictly coupled: a user allowed/denied on one is
/// allowed/denied on the other. The REST `get_run`/`list_run_events`/
/// `list_run_artifacts` handlers gate on `ensure_run_access` → `Read` on
/// `ResourceType::PlaybookRun` with the bare `run_id` string (see
/// `routes::playbooks::ensure_run_access`); this mirrors that exact
/// `(action, resource_type, resource_id)` triple. Returns the run when access is
/// granted.
async fn authorize_playbook_run_stream(
    state: &AppState,
    run_id: Uuid,
    request_context: &crate::contexts::shared::request_context::RequestContext,
) -> Result<PlaybookRun, AppError> {
    // Same auth surface as `authorize_websocket`: a missing user id means the
    // session cookie did not resolve to an authenticated user.
    if request_context.user_id.is_none() {
        return Err(AppError::Auth("Authentication required".to_string()));
    }
    let Some(run) = state.services.playbooks.get_run(run_id).await? else {
        return Err(AppError::NotFound("Playbook run not found".to_string()));
    };
    // RBAC: `Read` on `PlaybookRun` keyed by the bare `run_id` — byte-identical
    // to the REST run-read handlers so WS and REST cannot diverge.
    let run_resource_id = run.id.to_string();
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            Action::Read,
            ResourceType::PlaybookRun,
            Some(run_resource_id.as_str()),
        )
        .await?;
    Ok(run)
}

async fn handle_ws_playbook_run(
    socket: WebSocket,
    state: AppState,
    run: PlaybookRun,
    correlation_id: String,
) {
    let span = operation_span(
        "websocket_playbook_run_stream",
        &[
            ("run_id", run.id.to_string()),
            ("playbook_id", run.playbook_id.to_string()),
            ("correlation_id", correlation_id.clone()),
        ],
    );
    instrument_span(span, async move {
        let (mut sender, mut receiver) = socket.split();

        // Subscribe BEFORE replaying so a frame published in the replay window is
        // not lost; live frames are de-duped against the replayed sequence.
        let mut live = state.run_event_broadcaster.subscribe(run.id);

        // Re-read the run AFTER subscribing so the terminal-branch decision uses
        // a fresh status: a run that completed in the `[authorize get_run …
        // subscribe]` window published its Completion before this receiver
        // existed (so that frame is lost), but the fresh status now reads
        // terminal and we emit the synthetic completion below. Any completion
        // AFTER this re-read arrives live on the subscribed channel. Either way
        // the window is closed.
        let current = match state.services.playbooks.get_run(run.id).await {
            Ok(Some(current)) => current,
            // The run vanished between authorize and here: nothing to stream.
            Ok(None) => {
                let _ = sender.send(Message::Close(None)).await;
                return;
            }
            Err(error) => {
                log_playbook_stream_failure(&run, &correlation_id, "refetch_run", &error);
                let _ = sender.send(Message::Close(None)).await;
                return;
            }
        };

        // Replay persisted events in order, paging through the whole history.
        // `list_run_events` clamps `limit` to 1000, so a single call would
        // silently drop events 1001..N; we page at `PLAYBOOK_RUN_REPLAY_PAGE_SIZE`
        // and stop on a short page (the tail) or the safety cap. The repository
        // is the only SQL surface (this handler runs no inline SQL).
        let mut last_replayed_sequence: i64 = -1;
        let mut offset: u32 = 0;
        for _page in 0..PLAYBOOK_RUN_REPLAY_MAX_PAGES {
            let page = match state
                .services
                .playbooks
                .list_run_events(current.id, PLAYBOOK_RUN_REPLAY_PAGE_SIZE, offset)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    log_playbook_stream_failure(
                        &current,
                        &correlation_id,
                        "replay_run_events",
                        &error,
                    );
                    let _ = sender.send(Message::Close(None)).await;
                    return;
                }
            };
            let page_len = u32::try_from(page.items.len()).unwrap_or(u32::MAX);
            for event in &page.items {
                last_replayed_sequence = last_replayed_sequence.max(event.event_sequence);
                if !send_run_stream_frame(
                    &mut sender,
                    &RunStreamFrame::event_for_run(event, current.id),
                )
                .await
                {
                    return;
                }
            }
            // A short page (fewer than the page size) is the tail: stop paging.
            if page_len < PLAYBOOK_RUN_REPLAY_PAGE_SIZE {
                break;
            }
            offset += PLAYBOOK_RUN_REPLAY_PAGE_SIZE;
        }

        // A run that is already terminal needs no live stream: emit a synthetic
        // completion so the client closes cleanly (history / replay mode). Uses
        // the fresh status to close the completion race above.
        if current.status.is_terminal() {
            let frame = RunStreamFrame::Completion {
                run_id: current.id,
                status: current.status.as_str().to_string(),
                exit_code: current.exit_code,
                error_message: current.error_message.clone(),
            };
            let _ = send_run_stream_frame(&mut sender, &frame).await;
            let _ = sender.send(Message::Close(None)).await;
            return;
        }

        // Live forwarding: forward broadcaster frames until a terminal completion
        // or the client disconnects. A client close is observed on the read half.
        loop {
            tokio::select! {
                live_frame = live.recv() => {
                    match live_frame {
                        Ok(frame) => {
                            // Skip any event already delivered during replay.
                            if let RunStreamFrame::Event { event_sequence, .. } = &frame
                                && *event_sequence <= last_replayed_sequence {
                                    continue;
                                }
                            let is_completion =
                                matches!(frame, RunStreamFrame::Completion { .. });
                            if !send_run_stream_frame(&mut sender, &frame).await {
                                return;
                            }
                            if is_completion {
                                let _ = sender.send(Message::Close(None)).await;
                                return;
                            }
                        }
                        // A slow consumer overflowed the channel and dropped
                        // frames. If the dropped frame was the terminal
                        // Completion, the socket would otherwise wait forever (the
                        // FE disables REST polling while live). Re-read the run:
                        // if it is now terminal, emit a synthetic Completion from
                        // the fresh status and close; otherwise resync and keep
                        // forwarding. A fetch error / vanished run also closes so a
                        // dropped completion can never strand the client.
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            match state.services.playbooks.get_run(run.id).await {
                                Ok(Some(fresh)) if fresh.status.is_terminal() => {
                                    let frame = RunStreamFrame::Completion {
                                        run_id: fresh.id,
                                        status: fresh.status.as_str().to_string(),
                                        exit_code: fresh.exit_code,
                                        error_message: fresh.error_message.clone(),
                                    };
                                    let _ = send_run_stream_frame(&mut sender, &frame).await;
                                    let _ = sender.send(Message::Close(None)).await;
                                    return;
                                }
                                // Still running: skip the dropped frames and keep
                                // forwarding live (the client recovers the gap via
                                // REST history).
                                Ok(Some(_)) => continue,
                                // The run vanished or the fetch failed: don't risk
                                // stranding the client on a lost completion.
                                Ok(None) => {
                                    let _ = sender.send(Message::Close(None)).await;
                                    return;
                                }
                                Err(error) => {
                                    log_playbook_stream_failure(
                                        &run,
                                        &correlation_id,
                                        "lagged_refetch_run",
                                        &error,
                                    );
                                    let _ = sender.send(Message::Close(None)).await;
                                    return;
                                }
                            }
                        }
                        // The run's channel closed (process shutdown): end cleanly.
                        Err(broadcast::error::RecvError::Closed) => {
                            let _ = sender.send(Message::Close(None)).await;
                            return;
                        }
                    }
                }
                client_message = receiver.next() => {
                    match client_message {
                        // The client closed or its read half errored: stop forwarding.
                        Some(Ok(Message::Close(_))) | None => return,
                        Some(Err(_)) => return,
                        // Inbound frames are ignored: this is a server-push stream.
                        Some(Ok(_)) => {}
                    }
                }
            }
        }
    })
    .await;
}

/// Serialize and send a run-stream frame as a JSON text message. Returns `false`
/// when the client send fails (the socket is gone) so the caller can stop.
async fn send_run_stream_frame(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    frame: &RunStreamFrame,
) -> bool {
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    sender.send(Message::Text(text.into())).await.is_ok()
}

fn log_playbook_stream_failure<E: std::fmt::Display>(
    run: &PlaybookRun,
    correlation_id: &str,
    operation: &str,
    error: E,
) {
    log_error_event(
        "websocket.playbook_run_stream.failed",
        "websocket_route_error",
        error.to_string(),
        EventFields::default()
            .with_correlation_id(correlation_id)
            .with_operation(operation)
            .with_resource("playbook_run", run.id.to_string())
            .with_context("playbook_id", run.playbook_id.to_string()),
    )
}

struct WsUpgradeRequest {
    session_id: Uuid,
    expected_protocol: ConnectionProtocol,
    protocol_label: &'static str,
    proxy_path: &'static str,
    rdp_display_params: Option<RdpDisplayParams>,
}

async fn handle_ws_upgrade(
    state: AppState,
    request: WsUpgradeRequest,
    ws: WebSocketUpgrade,
    request_context: crate::contexts::shared::request_context::RequestContext,
) -> impl IntoResponse {
    let WsUpgradeRequest {
        session_id,
        expected_protocol,
        protocol_label,
        proxy_path,
        rdp_display_params,
    } = request;
    let (session, user_id) =
        match authorize_websocket(&state, session_id, expected_protocol, &request_context).await {
            Ok(values) => values,
            Err(error) => return handle_app_error(error),
        };

    with_subprotocols(ws, &session.protocol).on_upgrade(move |socket| {
        handle_ws(
            socket,
            WsConnectionContext {
                state,
                session,
                user_id,
                protocol_label,
                proxy_path,
                correlation_id: request_context.correlation_id,
                rdp_display_params,
            },
        )
    })
}

struct WsConnectionContext {
    state: AppState,
    session: ConnectionSession,
    user_id: Uuid,
    protocol_label: &'static str,
    proxy_path: &'static str,
    correlation_id: String,
    rdp_display_params: Option<RdpDisplayParams>,
}

fn with_subprotocols(ws: WebSocketUpgrade, protocol: &ConnectionProtocol) -> WebSocketUpgrade {
    match protocol {
        ConnectionProtocol::Ssh | ConnectionProtocol::Rdp => ws.protocols(["guacamole"]),
        ConnectionProtocol::Vnc => ws.protocols(["binary", "base64"]),
        ConnectionProtocol::App | ConnectionProtocol::Cctv => ws,
    }
}

/// Proxy close reason emitted when the *client* leg dropped but the SSH session
/// is still alive and re-attachable. Must match `WS_REASON_DETACHED_ALIVE` in
/// `proxy/ssh.rs`. When the proxy closes with this reason the API keeps the DB
/// session non-terminal so a re-attach within the grace window succeeds.
const PROXY_REASON_DETACHED_ALIVE: &str = "detached_alive";
const WS_CONTROL_DETACH_MESSAGE: &str = "__piping_ws__:{\"type\":\"detach\"}";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WsDisconnectReason {
    ClientClosed,
    /// Proxy reported the SSH session is still alive, only the client leg
    /// detached. Non-terminal: the DB session must stay re-attachable.
    DetachedAlive,
    ProxyClosed,
    ProxyInternalError,
    ProxyPolicyViolation,
    ProxyTryAgainLater,
    ClientBridgeError,
    ProxyBridgeError,
}

impl WsDisconnectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClientClosed => "client_closed",
            Self::DetachedAlive => "detached_alive",
            Self::ProxyClosed => "proxy_closed",
            Self::ProxyInternalError => "proxy_internal_error",
            Self::ProxyPolicyViolation => "proxy_policy_violation",
            Self::ProxyTryAgainLater => "proxy_try_again_later",
            Self::ClientBridgeError => "client_bridge_error",
            Self::ProxyBridgeError => "proxy_bridge_error",
        }
    }

    /// Reasons that leave the SSH session alive and the DB session
    /// re-attachable. These must NOT mark the session `Disconnected`.
    fn preserves_session(self) -> bool {
        matches!(
            self,
            // A clean client close, the proxy's explicit "still alive" signal, or
            // a client-leg transport failure (reload / network blip).
            Self::ClientClosed | Self::DetachedAlive | Self::ClientBridgeError
            // A raw proxy-leg transport failure (receiver errored without a clean
            // Close frame). When the client link drops, the whole bridge can RST
            // and the select! may surface the proxy leg's reset before the client
            // leg's — but the SSH PTY is owned by a detached task in the proxy
            // registry and is still alive, so the session must stay re-attachable.
            // A *real* SSH end always arrives as an explicit Close frame ("SSH
            // session ended" → ProxyClosed), which is NOT in this set and still
            // marks the session terminal. The detach reaper GCs a truly abandoned
            // PTY after the grace window.
            | Self::ProxyBridgeError
        )
    }
}

fn map_proxy_close_reason(close_code: Option<CloseCode>, reason: &str) -> WsDisconnectReason {
    if reason == PROXY_REASON_DETACHED_ALIVE {
        return WsDisconnectReason::DetachedAlive;
    }
    match close_code {
        Some(CloseCode::Error) => WsDisconnectReason::ProxyInternalError,
        Some(CloseCode::Policy) => WsDisconnectReason::ProxyPolicyViolation,
        Some(CloseCode::Again) => WsDisconnectReason::ProxyTryAgainLater,
        _ => WsDisconnectReason::ProxyClosed,
    }
}

async fn maybe_touch_session<F, Fut>(touch_gate: &AtomicI64, touch: F) -> Result<(), AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    let now_ms = Utc::now().timestamp_millis();
    let last_ms = touch_gate.load(Ordering::Relaxed);
    if now_ms - last_ms < SESSION_TOUCH_THROTTLE_MS {
        return Ok(());
    }
    if touch_gate
        .compare_exchange(last_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return Ok(());
    }
    touch().await
}

async fn authorize_websocket(
    state: &AppState,
    session_id: Uuid,
    expected_protocol: ConnectionProtocol,
    request_context: &crate::contexts::shared::request_context::RequestContext,
) -> Result<(ConnectionSession, Uuid), AppError> {
    let user_id = request_context
        .user_id
        .ok_or_else(|| AppError::Auth("Authentication required".to_string()))?;

    let connection_service = build_connection_service(state)?;
    let session = connection_service.require_session(session_id).await?;
    if session.protocol != expected_protocol {
        return Err(AppError::Forbidden(
            "Invalid connection protocol".to_string(),
        ));
    }
    if session.status.is_terminal() {
        return Err(AppError::Forbidden(
            "Connection session is no longer active".to_string(),
        ));
    }

    // SSH sessions keep the PTY alive across client drops so a non-owner
    // re-attach would inherit the owner's live authenticated shell and replayed
    // scrollback. Shared-session access is a separate explicit capability that
    // does not exist yet; hard-reject any cross-user attach unconditionally.
    if session.user_id != user_id {
        return Err(AppError::Forbidden(
            "Connection session belongs to another user".to_string(),
        ));
    }

    Ok((session, user_id))
}

async fn handle_ws(socket: WebSocket, context: WsConnectionContext) {
    let WsConnectionContext {
        state,
        session,
        user_id,
        protocol_label,
        proxy_path,
        correlation_id,
        rdp_display_params,
    } = context;
    let span = operation_span(
        "websocket_bridge",
        &[
            ("session_id", session.id.to_string()),
            ("protocol", protocol_label.to_string()),
            ("correlation_id", correlation_id.clone()),
            ("user_id", user_id.to_string()),
        ],
    );
    instrument_span(span, async move {
        log_ws_lifecycle_event(
            "websocket_connect",
            &session,
            user_id,
            protocol_label,
            &session.status,
            &correlation_id,
        );
        state.session_relay_health.mark_connecting(session.id).await;

        let connection_service = state.services.connection.clone();
        let setup_failure = WsSetupFailure {
            state: &state,
            session: &session,
            user_id,
            correlation_id: &correlation_id,
            connection_service: connection_service.as_ref(),
        };
        let Some(proxy_socket) =
            connect_proxy_ws(&setup_failure, proxy_path, rdp_display_params).await
        else {
            return;
        };

        if let Err(error) = connection_service
            .mark_session_active(session.id, user_id)
            .await
        {
            log_ws_failure(
                &session,
                user_id,
                &correlation_id,
                "session_mark_active",
                &error,
            );
        }

        if let Err(error) = record_ws_audit_event(
            &state,
            &session,
            user_id,
            "connection.session.websocket.connected",
            ConnectionStatus::Active,
            None,
        )
        .await
        {
            log_ws_failure(
                &session,
                user_id,
                &correlation_id,
                "audit_record_connected",
                &error,
            );
        }
        state.session_relay_health.mark_open(session.id).await;

        let (mut client_sender, mut client_receiver) = socket.split();
        let (mut proxy_sender, mut proxy_receiver) = proxy_socket.split();
        let touch_gate = Arc::new(AtomicI64::new(0));
        let relay = WsRelay {
            state: &state,
            session: &session,
            protocol_label,
            connection_service: connection_service.as_ref(),
            touch_gate: &touch_gate,
        };

        let disconnect_reason = tokio::select! {
            result = relay_client_to_proxy(&relay, &mut client_receiver, &mut proxy_sender) => {
                match result {
                    Ok(reason) => reason,
                    Err(error) => {
                        log_ws_failure(
                            &session,
                            user_id,
                            &correlation_id,
                            "client_to_proxy_bridge",
                            &error,
                        );
                        WsDisconnectReason::ClientBridgeError
                    }
                }
            }
            result = relay_proxy_to_client(&relay, &mut proxy_receiver, &mut client_sender) => {
                match result {
                    Ok(reason) => reason,
                    Err(error) => {
                        log_ws_failure(
                            &session,
                            user_id,
                            &correlation_id,
                            "proxy_to_client_bridge",
                            &error,
                        );
                        WsDisconnectReason::ProxyBridgeError
                    }
                }
            }
        };

        finalize_ws_disconnect(&setup_failure, disconnect_reason, protocol_label).await;
    })
    .await;
}

/// The split websocket halves and shared session/audit context used by the
/// relay directions. Bundling them keeps every relay helper's signature small.
struct WsRelay<'a> {
    state: &'a AppState,
    session: &'a ConnectionSession,
    protocol_label: &'static str,
    connection_service: &'a ConnectionServiceType,
    touch_gate: &'a AtomicI64,
}

/// Failure reporter for the websocket proxy-connection setup: logs the stage,
/// optionally records the failed-handshake audit event, then marks the session
/// and relay-health entry failed — the exact sequence every setup arm needs.
struct WsSetupFailure<'a> {
    state: &'a AppState,
    session: &'a ConnectionSession,
    user_id: Uuid,
    correlation_id: &'a str,
    connection_service: &'a ConnectionServiceType,
}

impl WsSetupFailure<'_> {
    async fn fail<E: std::fmt::Display>(
        &self,
        stage: &str,
        reason: &str,
        with_audit: bool,
        error: &E,
    ) {
        log_ws_failure(
            self.session,
            self.user_id,
            self.correlation_id,
            stage,
            error,
        );
        if with_audit
            && let Err(error) = record_ws_audit_event(
                self.state,
                self.session,
                self.user_id,
                "connection.session.websocket.failed",
                ConnectionStatus::Failed,
                Some(reason),
            )
            .await
        {
            log_ws_failure(
                self.session,
                self.user_id,
                self.correlation_id,
                "audit_record_failed",
                &error,
            );
        }
        let _ = self
            .connection_service
            .mark_session_failed(self.session.id, self.user_id)
            .await;
        self.state
            .session_relay_health
            .mark_failed(self.session.id, reason)
            .await;
    }
}

type WsClientSender = futures_util::stream::SplitSink<WebSocket, Message>;
type WsClientReceiver = futures_util::stream::SplitStream<WebSocket>;
type WsProxySocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsProxySender = futures_util::stream::SplitSink<WsProxySocket, ProxyMessage>;
type WsProxyReceiver = futures_util::stream::SplitStream<WsProxySocket>;

/// Run the six proxied-websocket setup steps. On the first failure the matching
/// `WsSetupFailure::fail` side effects run and `None` is returned (the caller
/// then returns without relaying).
async fn connect_proxy_ws(
    failure: &WsSetupFailure<'_>,
    proxy_path: &str,
    rdp_display_params: Option<RdpDisplayParams>,
) -> Option<WsProxySocket> {
    let proxy_route = match failure
        .connection_service
        .resolve_session_proxy_route(failure.session)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            failure
                .fail(
                    "resolve_session_proxy_route",
                    "route_resolution_failed",
                    false,
                    &error,
                )
                .await;
            return None;
        }
    };
    let proxy_base_url = proxy_route
        .proxy_base_url
        .unwrap_or_else(|| effective_proxy_base_url(failure.state.settings.as_ref()));
    let proxy_credential = match proxy_route.credential {
        Some(value) => value,
        None => match failure.state.services.proxy_shared_secret_provider.get() {
            Ok(value) => value,
            Err(error) => {
                failure
                    .fail(
                        "proxy_credential_get",
                        "proxy_credential_error",
                        false,
                        &error,
                    )
                    .await;
                return None;
            }
        },
    };
    let token = match create_proxy_ws_token(
        &proxy_credential.secret,
        failure.session.id,
        failure.user_id,
    ) {
        Ok(value) => value,
        Err(error) => {
            failure
                .fail("proxy_token_create", "proxy_token_error", true, &error)
                .await;
            return None;
        }
    };
    let proxy_ws_path = format!("/proxy/v1/ws/{}/{}", proxy_path, failure.session.id);
    let proxy_url = match build_proxy_ws_url(&proxy_base_url, &proxy_ws_path, &token)
        .and_then(|url| append_rdp_proxy_display_query(url, rdp_display_params))
    {
        Ok(url) => url,
        Err(error) => {
            failure
                .fail("proxy_url_build", "proxy_url_error", true, &error)
                .await;
            return None;
        }
    };

    open_proxy_connection(failure, &proxy_url, &failure.session.protocol).await
}

/// Build the client request for the resolved proxy URL and open the proxied
/// websocket, reporting the request-build and connect stages on failure.
async fn open_proxy_connection(
    failure: &WsSetupFailure<'_>,
    proxy_url: &str,
    protocol: &ConnectionProtocol,
) -> Option<WsProxySocket> {
    let mut request = match proxy_url.into_client_request() {
        Ok(request) => request,
        Err(error) => {
            failure
                .fail("proxy_request_build", "proxy_request_error", true, &error)
                .await;
            return None;
        }
    };

    if matches!(protocol, ConnectionProtocol::Ssh | ConnectionProtocol::Rdp) {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("guacamole"),
        );
    }

    match connect_async(request).await {
        Ok((socket, _response)) => Some(socket),
        Err(error) => {
            failure
                .fail("proxy_connect", "proxy_connect_failed", true, &error)
                .await;
            None
        }
    }
}

/// Client -> proxy leg. Returns the reason the leg ended.
async fn relay_client_to_proxy(
    relay: &WsRelay<'_>,
    client_receiver: &mut WsClientReceiver,
    proxy_sender: &mut WsProxySender,
) -> Result<WsDisconnectReason, AppError> {
    while let Some(message) = client_receiver.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                if relay.protocol_label == "ssh" {
                    proxy_sender
                        .send(ProxyMessage::Text(WS_CONTROL_DETACH_MESSAGE.into()))
                        .await
                        .map_err(|send_error| {
                            AppError::External(format!(
                                "Proxy websocket detach failed after client receive error: {}; {}",
                                send_error, error
                            ))
                        })?;
                    return pending().await;
                }
                return Err(AppError::External(format!(
                    "Client websocket receive failed: {}",
                    error
                )));
            }
        };
        if let Some(reason) = handle_client_to_proxy_message(relay, proxy_sender, message).await? {
            return Ok(reason);
        }
    }

    if relay.protocol_label == "ssh" {
        proxy_sender
            .send(ProxyMessage::Text(WS_CONTROL_DETACH_MESSAGE.into()))
            .await
            .map_err(|error| {
                AppError::External(format!("Proxy websocket detach failed: {}", error))
            })?;
        return pending().await;
    }
    Ok(WsDisconnectReason::ClientClosed)
}

/// One decoded client frame forwarded to the proxy. `Some(reason)` ends the leg.
async fn handle_client_to_proxy_message(
    relay: &WsRelay<'_>,
    proxy_sender: &mut WsProxySender,
    message: Message,
) -> Result<Option<WsDisconnectReason>, AppError> {
    match message {
        Message::Text(text) => {
            let bytes_len = text.len();
            proxy_sender
                .send(ProxyMessage::Text(text.to_string().into()))
                .await
                .map_err(|error| {
                    AppError::External(format!("Proxy websocket send failed: {}", error))
                })?;
            touch_relay_session(relay).await?;
            relay
                .state
                .session_relay_health
                .record_client_to_backend(relay.session.id, bytes_len)
                .await;
        }
        Message::Binary(bytes) => {
            let bytes_len = bytes.len();
            proxy_sender
                .send(ProxyMessage::Binary(bytes))
                .await
                .map_err(|error| {
                    AppError::External(format!("Proxy websocket send failed: {}", error))
                })?;
            touch_relay_session(relay).await?;
            relay
                .state
                .session_relay_health
                .record_client_to_backend(relay.session.id, bytes_len)
                .await;
        }
        Message::Close(_) => {
            if relay.protocol_label == "ssh" {
                proxy_sender
                    .send(ProxyMessage::Text(WS_CONTROL_DETACH_MESSAGE.into()))
                    .await
                    .map_err(|error| {
                        AppError::External(format!("Proxy websocket detach failed: {}", error))
                    })?;
                return pending().await;
            }
            let _ = proxy_sender.send(ProxyMessage::Close(None)).await;
            return Ok(Some(WsDisconnectReason::ClientClosed));
        }
        Message::Ping(payload) => {
            let _ = proxy_sender.send(ProxyMessage::Ping(payload)).await;
            touch_relay_session(relay).await?;
        }
        Message::Pong(payload) => {
            let _ = proxy_sender.send(ProxyMessage::Pong(payload)).await;
            touch_relay_session(relay).await?;
        }
    }
    Ok(None)
}

/// Proxy -> client leg. Returns the reason the leg ended.
async fn relay_proxy_to_client(
    relay: &WsRelay<'_>,
    proxy_receiver: &mut WsProxyReceiver,
    client_sender: &mut WsClientSender,
) -> Result<WsDisconnectReason, AppError> {
    while let Some(message) = proxy_receiver.next().await {
        let message = message.map_err(|error| {
            AppError::External(format!("Proxy websocket receive failed: {}", error))
        })?;
        if let Some(reason) = handle_proxy_to_client_message(relay, client_sender, message).await? {
            return Ok(reason);
        }
    }
    Ok(WsDisconnectReason::ProxyClosed)
}

/// One decoded proxy frame forwarded to the client. `Some(reason)` ends the leg.
async fn handle_proxy_to_client_message(
    relay: &WsRelay<'_>,
    client_sender: &mut WsClientSender,
    message: ProxyMessage,
) -> Result<Option<WsDisconnectReason>, AppError> {
    match message {
        ProxyMessage::Text(text) => {
            let bytes_len = text.len();
            client_sender
                .send(Message::Text(text.to_string().into()))
                .await
                .map_err(|error| {
                    AppError::External(format!("Client websocket send failed: {}", error))
                })?;
            touch_relay_session(relay).await?;
            relay
                .state
                .session_relay_health
                .record_backend_to_client(relay.session.id, bytes_len)
                .await;
        }
        ProxyMessage::Binary(bytes) => {
            let bytes_len = bytes.len();
            client_sender
                .send(Message::Binary(bytes))
                .await
                .map_err(|error| {
                    AppError::External(format!("Client websocket send failed: {}", error))
                })?;
            touch_relay_session(relay).await?;
            relay
                .state
                .session_relay_health
                .record_backend_to_client(relay.session.id, bytes_len)
                .await;
        }
        ProxyMessage::Close(frame) => {
            let disconnect_reason = map_proxy_close_reason(
                frame.as_ref().map(|close_frame| close_frame.code),
                frame
                    .as_ref()
                    .map(|close_frame| close_frame.reason.as_ref())
                    .unwrap_or_default(),
            );
            let client_frame = frame.map(|close_frame| {
                Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: close_frame.code.into(),
                    reason: close_frame.reason.to_string().into(),
                }))
            });
            match client_frame {
                Some(message) => {
                    let _ = client_sender.send(message).await;
                }
                None => {
                    let _ = client_sender.send(Message::Close(None)).await;
                }
            }
            return Ok(Some(disconnect_reason));
        }
        ProxyMessage::Ping(payload) => {
            let _ = client_sender.send(Message::Ping(payload)).await;
            touch_relay_session(relay).await?;
        }
        ProxyMessage::Pong(payload) => {
            let _ = client_sender.send(Message::Pong(payload)).await;
            touch_relay_session(relay).await?;
        }
        _ => {}
    }
    Ok(None)
}

/// Throttled session touch shared by both relay directions.
async fn touch_relay_session(relay: &WsRelay<'_>) -> Result<(), AppError> {
    maybe_touch_session(relay.touch_gate, || {
        relay.connection_service.touch_session(relay.session.id)
    })
    .await
}

/// Terminal handling once the relay ends: a preserving reason keeps the DB
/// session re-attachable, otherwise the session is audited and marked
/// disconnected.
async fn finalize_ws_disconnect(
    failure: &WsSetupFailure<'_>,
    disconnect_reason: WsDisconnectReason,
    protocol_label: &'static str,
) {
    if disconnect_reason.preserves_session() {
        failure
            .state
            .session_relay_health
            .mark_disconnected(failure.session.id, disconnect_reason.as_str())
            .await;
        log_ws_lifecycle_event(
            "websocket_detached",
            failure.session,
            failure.user_id,
            protocol_label,
            &ConnectionStatus::Active,
            failure.correlation_id,
        );
        return;
    }

    if let Err(error) = record_ws_audit_event(
        failure.state,
        failure.session,
        failure.user_id,
        "connection.session.websocket.disconnected",
        ConnectionStatus::Disconnected,
        Some(disconnect_reason.as_str()),
    )
    .await
    {
        log_ws_failure(
            failure.session,
            failure.user_id,
            failure.correlation_id,
            "audit_record_disconnected",
            &error,
        );
    }
    if let Err(error) = failure
        .connection_service
        .mark_session_disconnected(failure.session.id, failure.user_id)
        .await
    {
        log_ws_failure(
            failure.session,
            failure.user_id,
            failure.correlation_id,
            "session_mark_disconnected",
            &error,
        );
    }
    failure
        .state
        .session_relay_health
        .mark_disconnected(failure.session.id, disconnect_reason.as_str())
        .await;

    log_ws_lifecycle_event(
        "websocket_disconnected",
        failure.session,
        failure.user_id,
        protocol_label,
        &ConnectionStatus::Disconnected,
        failure.correlation_id,
    );
}

fn normalize_rdp_ws_display_query(
    query: RdpWsDisplayQuery,
) -> Result<Option<RdpDisplayParams>, AppError> {
    let width =
        validate_rdp_ws_dimension("width", query.width, RDP_WS_MIN_WIDTH, RDP_WS_MAX_WIDTH)?;
    let height =
        validate_rdp_ws_dimension("height", query.height, RDP_WS_MIN_HEIGHT, RDP_WS_MAX_HEIGHT)?;
    let dpi = validate_rdp_ws_dimension("dpi", query.dpi, RDP_WS_MIN_DPI, RDP_WS_MAX_DPI)?;
    let params = RdpDisplayParams { width, height, dpi };
    Ok(params.has_any().then_some(params))
}

fn validate_rdp_ws_dimension(
    key: &str,
    value: Option<u32>,
    min: u32,
    max: u32,
) -> Result<Option<u32>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if (min..=max).contains(&value) {
        return Ok(Some(value));
    }
    Err(AppError::Validation(format!(
        "Invalid RDP {key}; expected {min}..={max}"
    )))
}

fn append_rdp_proxy_display_query(
    proxy_url: String,
    params: Option<RdpDisplayParams>,
) -> Result<String, AppError> {
    let Some(params) = params.filter(|value| value.has_any()) else {
        return Ok(proxy_url);
    };
    let mut url = url::Url::parse(&proxy_url)
        .map_err(|error| AppError::Internal(format!("Invalid proxy websocket URL: {error}")))?;
    {
        let mut query = url.query_pairs_mut();
        if let Some(width) = params.width {
            query.append_pair("width", &width.to_string());
        }
        if let Some(height) = params.height {
            query.append_pair("height", &height.to_string());
        }
        if let Some(dpi) = params.dpi {
            query.append_pair("dpi", &dpi.to_string());
        }
    }
    Ok(url.to_string())
}

async fn record_ws_audit_event(
    state: &AppState,
    session: &ConnectionSession,
    user_id: Uuid,
    event_type: &str,
    status: ConnectionStatus,
    reason: Option<&str>,
) -> Result<(), AppError> {
    let audit_service = state.services.audit.clone();
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: event_type.to_string(),
        actor_id: Some(user_id),
        actor_kind: "user".to_string(),
        target_type: Some("connection_session".to_string()),
        target_id: Some(session.id),
        metadata: json!({
            "target_id": session.target_id.to_string(),
            "protocol": format!("{:?}", session.protocol),
            "status": format!("{:?}", status),
            "proxy_id": session.proxy_id.map(|value| value.to_string()),
            "proxy_node_id": session.proxy_node_id.map(|value| value.to_string()),
            "routing_source": session.routing_source.as_str(),
            "reason": reason,
        }),
        created_at: Utc::now(),
    };
    audit_service.record_event(event).await
}

fn log_ws_failure<E: std::fmt::Display>(
    session: &ConnectionSession,
    user_id: Uuid,
    correlation_id: &str,
    operation: &str,
    error: E,
) {
    log_error_event(
        "websocket.bridge.failed",
        "websocket_route_error",
        error.to_string(),
        EventFields::default()
            .with_correlation_id(correlation_id)
            .with_operation(operation)
            .with_resource("connection", session.id.to_string())
            .with_user_id(user_id.to_string())
            .with_context("target_id", session.target_id.to_string())
            .with_context("protocol", session.protocol.as_str().to_string()),
    )
}

fn log_ws_lifecycle_event(
    event: &str,
    session: &ConnectionSession,
    user_id: Uuid,
    protocol_label: &str,
    status: &ConnectionStatus,
    correlation_id: &str,
) {
    let canonical_event = match event {
        "websocket_connect" => "websocket.bridge.connected",
        "websocket_disconnected" => "websocket.bridge.disconnected",
        other => other,
    };
    log_connection_event(log_connection_event::Input {
        event: canonical_event,
        session_id: Some(session.id.to_string()),
        host_id: Some(session.target_id.to_string()),
        user_id: Some(user_id.to_string()),
        protocol: Some(protocol_label.to_string()),
        status: Some(format!("{:?}", status)),
        correlation_id: Some(correlation_id.to_string()),
    });
}

#[cfg(test)]
mod tests {
    use super::{
        RdpWsDisplayQuery, WsDisconnectReason, append_rdp_proxy_display_query,
        map_proxy_close_reason, normalize_rdp_ws_display_query,
    };
    use crate::proxy::RdpDisplayParams;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    #[test]
    fn maps_policy_close_to_policy_reason() {
        assert_eq!(
            map_proxy_close_reason(Some(CloseCode::Policy), ""),
            WsDisconnectReason::ProxyPolicyViolation
        );
    }

    #[test]
    fn maps_error_close_to_internal_reason() {
        assert_eq!(
            map_proxy_close_reason(Some(CloseCode::Error), ""),
            WsDisconnectReason::ProxyInternalError
        );
    }

    #[test]
    fn maps_again_close_to_retry_later_reason() {
        assert_eq!(
            map_proxy_close_reason(Some(CloseCode::Again), ""),
            WsDisconnectReason::ProxyTryAgainLater
        );
    }

    #[test]
    fn defaults_unknown_close_to_proxy_closed() {
        assert_eq!(
            map_proxy_close_reason(Some(CloseCode::Away), ""),
            WsDisconnectReason::ProxyClosed
        );
        assert_eq!(
            map_proxy_close_reason(None, ""),
            WsDisconnectReason::ProxyClosed
        );
    }

    #[test]
    fn maps_detached_alive_reason_to_non_terminal() {
        // A normal-code close carrying the detached_alive reason means the SSH
        // session is still alive and must NOT poison the DB session.
        let reason = map_proxy_close_reason(Some(CloseCode::Normal), "detached_alive");
        assert_eq!(reason, WsDisconnectReason::DetachedAlive);
        assert!(reason.preserves_session());
    }

    #[test]
    fn detached_alive_reason_overrides_close_code() {
        // The reason string is authoritative even if the code would otherwise
        // classify as an error.
        assert_eq!(
            map_proxy_close_reason(Some(CloseCode::Error), "detached_alive"),
            WsDisconnectReason::DetachedAlive
        );
    }

    #[test]
    fn transport_failures_preserve_session() {
        // Client close / client-leg transport failure (reload / network blip).
        assert!(WsDisconnectReason::ClientClosed.preserves_session());
        assert!(WsDisconnectReason::ClientBridgeError.preserves_session());
        assert!(WsDisconnectReason::DetachedAlive.preserves_session());
        // A raw proxy-leg transport failure (no clean Close frame) is the
        // RST-cascade case when the client link drops: the SSH PTY is still
        // alive in the proxy registry, so the session must stay re-attachable.
        assert!(WsDisconnectReason::ProxyBridgeError.preserves_session());
    }

    #[test]
    fn proxy_end_reasons_do_not_preserve_session() {
        // Explicit proxy Close frames signalling a real SSH end / policy stop.
        assert!(!WsDisconnectReason::ProxyClosed.preserves_session());
        assert!(!WsDisconnectReason::ProxyInternalError.preserves_session());
        assert!(!WsDisconnectReason::ProxyPolicyViolation.preserves_session());
        assert!(!WsDisconnectReason::ProxyTryAgainLater.preserves_session());
    }

    #[test]
    fn rdp_ws_display_query_accepts_viewer_size() {
        let params = normalize_rdp_ws_display_query(RdpWsDisplayQuery {
            width: Some(1216),
            height: Some(794),
            dpi: Some(96),
        });

        assert_eq!(
            params.expect("valid params"),
            Some(RdpDisplayParams {
                width: Some(1216),
                height: Some(794),
                dpi: Some(96),
            })
        );
    }

    #[test]
    fn rdp_ws_display_query_rejects_out_of_range_values() {
        let params = normalize_rdp_ws_display_query(RdpWsDisplayQuery {
            width: Some(0),
            height: Some(794),
            dpi: None,
        });

        assert!(params.is_err(), "zero width must be rejected");
    }

    #[test]
    fn rdp_proxy_display_query_preserves_token() {
        let url = append_rdp_proxy_display_query(
            "ws://proxy.example/proxy/v1/ws/rdp/session?token=abc".to_string(),
            Some(RdpDisplayParams {
                width: Some(1216),
                height: Some(794),
                dpi: None,
            }),
        )
        .expect("url");

        assert_eq!(
            url,
            "ws://proxy.example/proxy/v1/ws/rdp/session?token=abc&width=1216&height=794"
        );
    }
}
