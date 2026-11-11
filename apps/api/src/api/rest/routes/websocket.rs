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
use crate::api::middleware::auth::{AuthContext, reauthenticate_observer};
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{Path, Query};
use crate::api::rest::request_context::{RequestContextExtractor, build_request_context};
use crate::app::AppState;
use crate::config::effective_proxy_base_url;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::{
    AuditContext, AuditCorrelation, AuditDecision, AuditEmitter, AuditEmitterKind, AuditEvent,
    AuditLifecycleState, AuditOutcome, AuditTransportState,
};
use crate::contexts::connections::domain::{
    ConnectionProtocol, ConnectionSession, ConnectionStatus,
};
use crate::contexts::playbooks::application::RunStreamFrame;
use crate::contexts::playbooks::domain::PlaybookRun;
use crate::errors::AppError;
use crate::integrations::proxy::{
    build_proxy_ws_url, create_proxy_observer_ws_token, create_proxy_ws_token,
};
use crate::observability::{
    EventFields, instrument_span, log_connection_event, log_error_event, operation_span,
};
use crate::proxy::{
    OBSERVER_LEASE_EXPIRED_CLOSE_CODE, RdpDisplayParams, WS_REASON_CONCURRENT_ATTACH_REJECTED,
    WS_REASON_OBSERVER_LEASE_EXPIRED, is_concurrent_attach_refusal, is_observer_lease_expiry_close,
};

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

/// Observation revalidation starts every five seconds. A slow dependency gets at
/// most three seconds; no frames are forwarded while revalidation is in flight.
const OBSERVER_REVALIDATE_SECONDS: u64 = 5;
const OBSERVER_CHECK_TIMEOUT_SECONDS: u64 = 3;

/// Open an observation-only stream for an existing session.
#[utoipa::path(
    get, path = "/api/v1/ws/observe/{session_id}", tag = "connections",
    params(("session_id" = String, Path, description = "Connection session ID")),
    responses((status = 101, description = "Observation websocket"),
              (status = 403, description = "Observation denied"))
)]
pub async fn ws_observe(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    ws: WebSocketUpgrade,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let initial = build_request_context(&request_id, auth_context, &headers);
    if !observer_origin_allowed(
        &headers,
        &state.settings.server.cors_allowed_origins,
        initial.auth_method == "api_token",
    ) {
        // The request is denied even if persistence is unavailable; the audit
        // helper emits a structured failure without the original error text.
        let _ = record_observe_audit(
            &state,
            initial.user_id,
            session_id,
            None,
            None,
            &request_id.0,
            "connection.session.observe.denied",
            "origin_denied",
        )
        .await;
        return handle_app_error(AppError::Forbidden("Observation origin denied".to_string()));
    }
    let mut evidence = HeaderMap::new();
    for name in [
        axum::http::header::AUTHORIZATION,
        axum::http::header::COOKIE,
    ] {
        for value in headers.get_all(&name).iter() {
            evidence.append(name.clone(), value.clone());
        }
    }
    let checked = tokio::time::timeout(
        std::time::Duration::from_secs(OBSERVER_CHECK_TIMEOUT_SECONDS),
        authorize_observer(&state, session_id, &evidence, &request_id, &initial, None),
    )
    .await;
    let (session, actor, generation) = match checked {
        Ok(Ok(value)) => value,
        _ => {
            // Failure is already logged by the audit helper; never upgrade.
            let _ = record_observe_audit(
                &state,
                initial.user_id,
                session_id,
                None,
                None,
                &request_id.0,
                "connection.session.observe.denied",
                "authorization_failed",
            )
            .await;
            return handle_app_error(AppError::Forbidden("Observation denied".to_string()));
        }
    };
    if record_observe_audit(
        &state,
        Some(actor),
        session_id,
        Some(session.target_id),
        Some(actor),
        &request_id.0,
        "connection.session.observe.started",
        "authorized",
    )
    .await
    .is_err()
    {
        return handle_app_error(AppError::Forbidden(
            "Observation audit unavailable".to_string(),
        ));
    }
    with_subprotocols(
        ws.max_message_size(4096).max_frame_size(4096),
        &session.protocol,
    )
    .on_upgrade(move |socket| async move {
        let reason = relay_observer(
            socket,
            &state,
            &session,
            actor,
            generation,
            &evidence,
            &request_id,
            &initial,
        )
        .await;
        if matches!(
            reason,
            "authentication_invalid_or_unavailable"
                | "policy_denied_or_unavailable"
                | "identity_changed"
                | "input_denied"
        ) {
            // The socket is closing; persistence failure is logged by the helper.
            let _ = record_observe_audit(
                &state,
                if reason == "input_denied" {
                    Some(actor)
                } else {
                    None
                },
                session_id,
                Some(session.target_id),
                Some(actor),
                &request_id.0,
                "connection.session.observe.denied",
                reason,
            )
            .await;
        }
        // The socket has closed; persistence failure is logged by the helper.
        let _ = record_observe_audit(
            &state,
            None,
            session_id,
            Some(session.target_id),
            Some(actor),
            &request_id.0,
            "connection.session.observe.stopped",
            reason,
        )
        .await;
    })
    .into_response()
}

fn observer_origin_allowed(headers: &HeaderMap, allowed: &[String], bearer: bool) -> bool {
    use axum::http::header::ORIGIN;
    let origins = headers.get_all(ORIGIN);
    let mut values = origins.iter();
    let Some(value) = values.next() else {
        // Native PAT clients cannot cause a browser to send ambient cookies.
        return bearer && headers.contains_key(axum::http::header::AUTHORIZATION);
    };
    if values.next().is_some() {
        return false;
    }
    let Ok(origin) = value.to_str() else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.origin().ascii_serialization() != origin
    {
        return false;
    }
    if allowed
        .iter()
        .any(|entry| entry.trim().trim_end_matches('/') == origin)
    {
        return true;
    }
    crate::config::public_base_url_from_headers(headers)
        .and_then(|base| url::Url::parse(&base).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .is_some_and(|url| url.origin().ascii_serialization() == origin)
}

pub(crate) async fn authorize_observe_policy(
    state: &AppState,
    context: &crate::contexts::shared::request_context::RequestContext,
    session: &ConnectionSession,
) -> Result<(), AppError> {
    context.require_authenticated_user_id()?;
    if session.ended_at.is_some() || session.status.is_terminal() || session.proxy_id.is_none() {
        return Err(AppError::Forbidden("Session is not observable".to_string()));
    }
    if !matches!(
        session.protocol,
        ConnectionProtocol::Ssh | ConnectionProtocol::Rdp | ConnectionProtocol::Vnc
    ) {
        return Err(AppError::Forbidden("Session is not observable".to_string()));
    }
    let facets = crate::api::connection_authz_attributes::resolve_connection_target_facets(
        state,
        session.target_id,
    )
    .await?
    .ok_or_else(|| AppError::Forbidden("Target unavailable".to_string()))?;
    let attributes = crate::api::connection_authz_attributes::connection_target_authz_attributes(
        &facets,
        Some(session.protocol.as_str()),
    );
    let mut session_attributes = attributes.clone();
    session_attributes.insert("user_id".to_string(), json!(session.user_id.to_string()));
    session_attributes.insert(
        "target_id".to_string(),
        json!(session.target_id.to_string()),
    );
    session_attributes.insert("status".to_string(), json!(session.status.as_str()));
    let policy = crate::api::access_control_helpers::build_policy_evaluation_service(state);
    policy
        .ensure_access_with_attributes(
            context,
            Action::Observe,
            ResourceType::ConnectionSession,
            Some(&format!("connection_session:{}", session.id)),
            Some(session_attributes),
        )
        .await?;
    policy
        .ensure_access_with_attributes(
            context,
            Action::Observe,
            ResourceType::ConnectionTarget,
            Some(&format!("connection_target:{}", session.target_id)),
            Some(attributes),
        )
        .await
}

async fn authorize_observer(
    state: &AppState,
    session_id: Uuid,
    evidence: &HeaderMap,
    request_id: &RequestId,
    initial: &crate::contexts::shared::request_context::RequestContext,
    expected_generation: Option<Uuid>,
) -> Result<(ConnectionSession, Uuid, Uuid), &'static str> {
    let auth = reauthenticate_observer(state, evidence)
        .await
        .map_err(|_| "authentication_invalid_or_unavailable")?;
    let actor = auth
        .user_id
        .ok_or("authentication_invalid_or_unavailable")?;
    if initial.user_id != Some(actor)
        || initial.auth_method != auth.auth_method
        || initial.api_token_id != auth.api_token_id
    {
        return Err("identity_changed");
    }
    let mut context = build_request_context(request_id, Some(Extension(auth)), evidence);
    context.request_ip = initial.request_ip.clone();
    let session = state
        .services
        .connection
        .require_session(session_id)
        .await
        .map_err(|_| "session_unavailable")?;
    if session.ended_at.is_some() || session.status.is_terminal() {
        return Err("session_ended");
    }
    authorize_observe_policy(state, &context, &session)
        .await
        .map_err(|_| "policy_denied_or_unavailable")?;
    let sample = state
        .services
        .connection
        .get_connection_metrics(session.id)
        .await
        .map_err(|_| "runtime_unavailable")?
        .and_then(|value| value.metrics.runtime)
        .ok_or("runtime_unavailable")?;
    if !matches!(sample.state.as_str(), "live" | "detached") {
        return Err(if sample.state == "ended" {
            "runtime_ended"
        } else {
            "runtime_unavailable"
        });
    }
    let generation = sample
        .generation
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or("runtime_unavailable")?;
    if session.runtime_generation() != Some(generation) {
        return Err("runtime_generation_changed");
    }
    if expected_generation.is_some_and(|expected| expected != generation) {
        return Err("runtime_generation_changed");
    }
    Ok((session, actor, generation))
}

async fn record_observe_audit(
    state: &AppState,
    actor: Option<Uuid>,
    session_id: Uuid,
    target_id: Option<Uuid>,
    observer_id: Option<Uuid>,
    request_id: &str,
    event: &str,
    reason: &str,
) -> Result<(), AppError> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(OBSERVER_CHECK_TIMEOUT_SECONDS),
        async {
            let event = build_observe_audit_event(
                actor,
                session_id,
                target_id,
                observer_id,
                request_id,
                event,
                reason,
            )?;
            state.services.audit.record_event(event).await
        },
    )
    .await;
    if matches!(result, Ok(Ok(()))) {
        return Ok(());
    }
    let mut fields = EventFields::default()
        .with_operation("observation_audit")
        .with_resource("connection_session", session_id.to_string())
        .with_context("audit_event", event);
    if let Some(actor) = actor {
        fields = fields.with_user_id(actor.to_string());
    }
    log_error_event(
        "connection.session.observe.audit_failed",
        "audit_unavailable",
        "Observation audit could not be recorded",
        fields,
    );
    Err(AppError::Internal(
        "Observation audit unavailable".to_string(),
    ))
}

fn build_observe_audit_event(
    actor: Option<Uuid>,
    session_id: Uuid,
    target_id: Option<Uuid>,
    observer_id: Option<Uuid>,
    request_id: &str,
    event: &str,
    reason: &str,
) -> Result<AuditEvent, AppError> {
    let (outcome, decision, transport) = match event {
        // This audit gate precedes the proxy handshake. Authorization is known,
        // but observing a connected display has not yet been established.
        "connection.session.observe.started" => {
            (AuditOutcome::Unknown, Some(AuditDecision::Allowed), None)
        }
        "connection.session.observe.denied" => (
            AuditOutcome::Denied,
            matches!(
                reason,
                "origin_denied" | "input_denied" | "identity_changed"
            )
            .then_some(AuditDecision::Denied),
            None,
        ),
        "connection.session.observe.stopped" => (
            AuditOutcome::NotApplicable,
            None,
            Some(AuditTransportState::Disconnected),
        ),
        _ => (AuditOutcome::Unknown, None, None),
    };
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: event.to_string(),
        actor_id: actor,
        actor_kind: if actor.is_some() { "user" } else { "system" }.to_string(),
        target_type: Some("connection_session".to_string()),
        target_id: Some(session_id),
        metadata: json!({"reason": reason, "target_id": target_id.map(|id| id.to_string()),
            "observer_id": observer_id.map(|id| id.to_string())}),
        created_at: Utc::now(),
    }
    .with_context(AuditContext {
        outcome,
        decision,
        state: transport.map(AuditLifecycleState::Transport),
        emitter: Some(AuditEmitter {
            kind: AuditEmitterKind::Api,
            id: None,
        }),
        correlation: AuditCorrelation {
            request_id: Uuid::parse_str(request_id).ok(),
            session_id: Some(session_id),
            ..Default::default()
        },
        ..Default::default()
    })
}

async fn relay_observer(
    socket: WebSocket,
    state: &AppState,
    session: &ConnectionSession,
    actor: Uuid,
    generation: Uuid,
    evidence: &HeaderMap,
    request_id: &RequestId,
    initial: &crate::contexts::shared::request_context::RequestContext,
) -> &'static str {
    let Some(proxy_id) = session.proxy_id else {
        return "runtime_unavailable";
    };
    let route = match state
        .services
        .connection
        .resolve_session_proxy_route(session)
        .await
    {
        Ok(route) => route,
        Err(_) => return "proxy_unavailable",
    };
    let credential = match route.credential {
        Some(value) => value,
        None => match state.services.proxy_shared_secret_provider.get() {
            Ok(value) => value,
            Err(_) => return "proxy_unavailable",
        },
    };
    let token =
        match create_proxy_observer_ws_token(&credential.secret, proxy_id, actor, generation) {
            Ok(token) => token,
            Err(_) => return "proxy_unavailable",
        };
    let base = route
        .proxy_base_url
        .unwrap_or_else(|| effective_proxy_base_url(state.settings.as_ref()));
    let url = match build_proxy_ws_url(&base, &format!("/proxy/v1/ws/observe/{proxy_id}"), &token) {
        Ok(url) => url,
        Err(_) => return "proxy_unavailable",
    };
    let mut request = match url.as_str().into_client_request() {
        Ok(request) => request,
        Err(_) => return "proxy_unavailable",
    };
    if matches!(
        session.protocol,
        ConnectionProtocol::Rdp | ConnectionProtocol::Vnc
    ) {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("guacamole"),
        );
    }
    let proxy = match tokio::time::timeout(
        std::time::Duration::from_secs(OBSERVER_CHECK_TIMEOUT_SECONDS),
        connect_async(request),
    )
    .await
    {
        Ok(Ok((proxy, _))) => proxy,
        _ => return "proxy_unavailable",
    };
    let valid = || async {
        match tokio::time::timeout(
            std::time::Duration::from_secs(OBSERVER_CHECK_TIMEOUT_SECONDS),
            authorize_observer(
                state,
                session.id,
                evidence,
                request_id,
                initial,
                Some(generation),
            ),
        )
        .await
        {
            Ok(Ok((current, _, _))) => {
                if current.target_id == session.target_id
                    && current.proxy_id == session.proxy_id
                    && current.protocol.as_str() == session.protocol.as_str()
                {
                    None
                } else {
                    Some("session_identity_changed")
                }
            }
            Ok(Err(reason)) => Some(reason),
            Err(_) => Some("reauthorization_timeout"),
        }
    };
    // The owner can end or replace the session while the proxy handshake runs.
    if let Some(reason) = valid().await {
        return reason;
    }
    let (mut client_tx, mut client_rx) = socket.split();
    let (mut proxy_tx, mut proxy_rx) = proxy.split();
    let period = std::time::Duration::from_secs(OBSERVER_REVALIDATE_SECONDS);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            biased;
            _ = interval.tick() => {
                if let Some(reason) = valid().await { return reason; }
            }
            incoming = client_rx.next() => match incoming {
                Some(Ok(Message::Text(text))) if !matches!(session.protocol, ConnectionProtocol::Ssh) && observer_guacamole_ping(&text) => {
                    // Guacamole tunnel heartbeat is local to this observer leg.
                    if !observer_send(&mut client_tx, Message::Text(text)).await { return "client_transport_lost"; }
                }
                Some(Ok(Message::Text(text))) if !matches!(session.protocol, ConnectionProtocol::Ssh) && observer_guacamole_ack(&text) => {
                    if !observer_send(&mut proxy_tx, ProxyMessage::Text(text.to_string().into())).await { return "proxy_closed"; }
                }
                Some(Ok(Message::Ping(payload))) => { if !observer_send(&mut proxy_tx, ProxyMessage::Ping(payload)).await { return "proxy_closed"; } }
                Some(Ok(Message::Pong(payload))) => { if !observer_send(&mut proxy_tx, ProxyMessage::Pong(payload)).await { return "proxy_closed"; } }
                Some(Ok(Message::Close(_))) => return "client_close_frame",
                None | Some(Err(_)) => return "client_transport_lost",
                _ => return "input_denied",
            },
            incoming = proxy_rx.next() => match incoming {
                Some(Ok(ProxyMessage::Text(text))) => { if !observer_send(&mut client_tx, Message::Text(text.to_string().into())).await { return "client_transport_lost"; } }
                Some(Ok(ProxyMessage::Binary(bytes))) => { if !observer_send(&mut client_tx, Message::Binary(bytes)).await { return "client_transport_lost"; } }
                Some(Ok(ProxyMessage::Ping(bytes))) => { if !observer_send(&mut client_tx, Message::Ping(bytes)).await { return "client_transport_lost"; } }
                Some(Ok(ProxyMessage::Pong(bytes))) => { if !observer_send(&mut client_tx, Message::Pong(bytes)).await { return "client_transport_lost"; } }
                Some(Ok(ProxyMessage::Close(frame))) => {
                    if let Some(frame) = frame.as_ref()
                        && is_observer_lease_expiry_close(u16::from(frame.code), frame.reason.as_str())
                    {
                        // Forward the exact expiry close; a bounded send failure
                        // only ends this observer leg, never the session.
                        let _ = observer_send(
                            &mut client_tx,
                            Message::Close(Some(axum::extract::ws::CloseFrame {
                                code: OBSERVER_LEASE_EXPIRED_CLOSE_CODE,
                                reason: WS_REASON_OBSERVER_LEASE_EXPIRED.into(),
                            })),
                        )
                        .await;
                        return "lease_expired";
                    }
                    return "proxy_close_frame";
                }
                _ => return "proxy_transport_lost",
            }
        }
    }
}

/// Pure boundary: only the proxy's exact lease-expiry close (code 1000 and the
/// canonical reason) is a reconnect signal. Everything else — policy closes,
/// transport errors, revocation, generation loss — stays terminal.
fn observer_guacamole_ping(text: &str) -> bool {
    let Some(value) = text.strip_prefix("0.,4.ping,") else {
        return false;
    };
    let Some((length, digits)) = value.split_once('.') else {
        return false;
    };
    let Some(digits) = digits.strip_suffix(';') else {
        return false;
    };
    if length.is_empty()
        || length.len() > 2
        || !length.bytes().all(|b| b.is_ascii_digit())
        || digits.is_empty()
        || digits.len() > 20
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return false;
    }
    length.parse::<usize>().ok() == Some(digits.len())
}

async fn observer_send<S, M>(sink: &mut S, message: M) -> bool
where
    S: futures_util::Sink<M> + Unpin,
{
    tokio::time::timeout(
        std::time::Duration::from_secs(OBSERVER_CHECK_TIMEOUT_SECONDS),
        sink.send(message),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Accept a single bounded ASCII Guacamole sync/ack instruction, never input.
fn observer_guacamole_ack(text: &str) -> bool {
    if text.len() > 4096 || !text.is_ascii() {
        return false;
    }
    let bytes = text.as_bytes();
    let mut pos = 0;
    let mut fields = Vec::new();
    loop {
        let start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos == start || pos >= bytes.len() || bytes[pos] != b'.' {
            return false;
        }
        let Ok(length) = text[start..pos].parse::<usize>() else {
            return false;
        };
        pos += 1;
        let Some(end) = pos.checked_add(length).filter(|end| *end < bytes.len()) else {
            return false;
        };
        fields.push(&text[pos..end]);
        pos = end;
        match bytes[pos] {
            b',' => {
                pos += 1;
                if fields.len() >= 4 {
                    return false;
                }
            }
            b';' if pos + 1 == bytes.len() => break,
            _ => return false,
        }
    }
    match fields.as_slice() {
        ["sync", timestamp] => {
            !timestamp.is_empty() && timestamp.bytes().all(|b| b.is_ascii_digit())
        }
        ["ack", stream, message, status] => {
            !stream.is_empty()
                && stream.bytes().all(|b| b.is_ascii_digit())
                && message.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
                && !status.is_empty()
                && status.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    }
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
            ("run_group_id", run.run_group_id.to_string()),
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
            .with_context("run_group_id", run.run_group_id.to_string()),
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
        ConnectionProtocol::Ssh | ConnectionProtocol::Rdp | ConnectionProtocol::Vnc => {
            ws.protocols(["guacamole"])
        }
        ConnectionProtocol::App
        | ConnectionProtocol::Cctv
        | ConnectionProtocol::Winrm
        | ConnectionProtocol::Psrp => ws,
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
    /// A second operator attach was refused because the connection already holds
    /// a live operator leg. Only the refused leg ends: the existing operator,
    /// observer, generation and upstream keep running, so the DB session must
    /// stay active and must not gain a terminal-disconnect audit.
    ConcurrentAttachRejected,
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
            Self::ConcurrentAttachRejected => WS_REASON_CONCURRENT_ATTACH_REJECTED,
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
            // A refused second attach is not a detachment either: the live
            // operator still owns the connection, so the session and its
            // relay state stay exactly as they are.
            | Self::ConcurrentAttachRejected
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

    fn audit_outcome(self) -> AuditOutcome {
        match self {
            Self::ClientClosed | Self::DetachedAlive | Self::ProxyClosed => {
                AuditOutcome::NotApplicable
            }
            Self::ProxyPolicyViolation => AuditOutcome::Denied,
            Self::ConcurrentAttachRejected => AuditOutcome::Denied,
            Self::ProxyInternalError
            | Self::ProxyTryAgainLater
            | Self::ClientBridgeError
            | Self::ProxyBridgeError => AuditOutcome::Failed,
        }
    }
}

fn map_proxy_close_reason(close_code: Option<CloseCode>, reason: &str) -> WsDisconnectReason {
    if reason == PROXY_REASON_DETACHED_ALIVE {
        return WsDisconnectReason::DetachedAlive;
    }
    // Checked before the generic 1008 policy mapping: this exact cause refuses
    // one attach leg and leaves the live session untouched.
    if close_code == Some(CloseCode::Policy) && reason == WS_REASON_CONCURRENT_ATTACH_REJECTED {
        return WsDisconnectReason::ConcurrentAttachRejected;
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
        let mut socket = socket;
        log_ws_lifecycle_event(
            "websocket_connect",
            &session,
            user_id,
            protocol_label,
            &session.status,
            &correlation_id,
        );

        let connection_service = state.services.connection.clone();
        let setup_failure = WsSetupFailure {
            state: &state,
            session: &session,
            user_id,
            correlation_id: &correlation_id,
            connection_service: connection_service.as_ref(),
        };
        let proxy_socket =
            match connect_proxy_ws(&setup_failure, proxy_path, rdp_display_params).await {
                Some(ProxyWsAdmission::Admitted(socket)) => socket,
                Some(ProxyWsAdmission::Refused) => {
                    deny_ws_attach(&setup_failure, &mut socket, protocol_label).await;
                    return;
                }
                None => return,
            };

        // Only an admitted leg owns relay state. Marking connecting/open, the
        // session activation and the connected audit all follow the private
        // proxy's admission, so a refused attach cannot borrow the live leg's.
        state.session_relay_health.mark_connecting(session.id).await;

        if let Err(error) = connection_service
            .mark_session_active(session.id, user_id, Some(&correlation_id))
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
            "connection.session.websocket.connected",
            Some(AuditTransportState::Connected),
            AuditOutcome::Succeeded,
            None,
            &correlation_id,
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
                "connection.session.websocket.failed",
                Some(AuditTransportState::Failed),
                AuditOutcome::Failed,
                Some(reason),
                self.correlation_id,
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
            .mark_session_failed(
                self.session.id,
                self.user_id,
                Some(self.correlation_id),
                Some(reason),
            )
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

/// The private proxy's admission decision. Only `Admitted` may activate a leg:
/// a `Refused` leg is denied without touching the live operator's session.
enum ProxyWsAdmission {
    Admitted(WsProxySocket),
    Refused,
}

/// Run the six proxied-websocket setup steps. On the first failure the matching
/// `WsSetupFailure::fail` side effects run and `None` is returned (the caller
/// then returns without relaying). A concurrent refusal is not a setup failure:
/// it is returned as its own admission outcome, without any session effect.
async fn connect_proxy_ws(
    failure: &WsSetupFailure<'_>,
    proxy_path: &str,
    rdp_display_params: Option<RdpDisplayParams>,
) -> Option<ProxyWsAdmission> {
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
/// websocket, reporting the request-build and connect stages on failure. The
/// proxy's exact pre-upgrade refusal is returned as an admission decision, so
/// the caller never treats it as a live leg or a failed session.
async fn open_proxy_connection(
    failure: &WsSetupFailure<'_>,
    proxy_url: &str,
    protocol: &ConnectionProtocol,
) -> Option<ProxyWsAdmission> {
    let mut request = match proxy_url.into_client_request() {
        Ok(request) => request,
        Err(error) => {
            failure
                .fail("proxy_request_build", "proxy_request_error", true, &error)
                .await;
            return None;
        }
    };

    if matches!(
        protocol,
        ConnectionProtocol::Ssh | ConnectionProtocol::Rdp | ConnectionProtocol::Vnc
    ) {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("guacamole"),
        );
    }

    match connect_async(request).await {
        Ok((socket, _response)) => Some(ProxyWsAdmission::Admitted(socket)),
        Err(error) if is_concurrent_attach_refusal(&error) => Some(ProxyWsAdmission::Refused),
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

/// Deny one attach leg the proxy refused. The browser still receives the same
/// canonical close frame the proxy used to send, while nothing about the live
/// operator — relay health, session status, audit trail, observers, generation
/// or upstream — is touched.
async fn deny_ws_attach(
    failure: &WsSetupFailure<'_>,
    socket: &mut WebSocket,
    protocol_label: &'static str,
) {
    send_ws_close(socket, 1008, WS_REASON_CONCURRENT_ATTACH_REJECTED).await;
    audit_attach_denied(
        failure,
        protocol_label,
        WsDisconnectReason::ConcurrentAttachRejected,
    )
    .await;
}

async fn send_ws_close(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

/// The refused leg's only lifecycle record. Shared by the pre-upgrade refusal
/// and the residual post-upgrade close so both deny identically.
async fn audit_attach_denied(
    failure: &WsSetupFailure<'_>,
    protocol_label: &'static str,
    reason: WsDisconnectReason,
) {
    if let Err(error) = record_ws_audit_event(
        failure.state,
        failure.session,
        "connection.session.websocket.attach_denied",
        None,
        reason.audit_outcome(),
        Some(reason.as_str()),
        failure.correlation_id,
    )
    .await
    {
        log_ws_failure(
            failure.session,
            failure.user_id,
            failure.correlation_id,
            "audit_record_attach_denied",
            &error,
        );
    }
    log_ws_lifecycle_event(
        "websocket_leg_denied",
        failure.session,
        failure.user_id,
        protocol_label,
        &ConnectionStatus::Active,
        failure.correlation_id,
    );
}

/// Terminal handling once the relay ends: a preserving reason keeps the DB
/// session re-attachable, otherwise the session is audited and marked
/// disconnected.
async fn finalize_ws_disconnect(
    failure: &WsSetupFailure<'_>,
    disconnect_reason: WsDisconnectReason,
    protocol_label: &'static str,
) {
    // A refused second attach belongs to the refused leg alone: audit the denial,
    // never a terminal disconnect. The live operator keeps its session, relay
    // health, observer, generation and upstream, and the session must not gain a
    // second, terminal audit for a lifecycle that never ended.
    if disconnect_reason == WsDisconnectReason::ConcurrentAttachRejected {
        audit_attach_denied(failure, protocol_label, disconnect_reason).await;
        return;
    }
    if disconnect_reason.preserves_session() {
        if let Err(error) = record_ws_audit_event(
            failure.state,
            failure.session,
            "connection.session.websocket.disconnected",
            Some(AuditTransportState::Detached),
            disconnect_reason.audit_outcome(),
            Some(disconnect_reason.as_str()),
            failure.correlation_id,
        )
        .await
        {
            log_ws_failure(
                failure.session,
                failure.user_id,
                failure.correlation_id,
                "audit_record_detached",
                &error,
            );
        }
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
        "connection.session.websocket.disconnected",
        Some(AuditTransportState::Disconnected),
        disconnect_reason.audit_outcome(),
        Some(disconnect_reason.as_str()),
        failure.correlation_id,
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
        .mark_session_disconnected(
            failure.session.id,
            failure.user_id,
            Some(failure.correlation_id),
            Some(disconnect_reason.as_str()),
        )
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
    event_type: &str,
    transport: Option<AuditTransportState>,
    outcome: AuditOutcome,
    reason: Option<&str>,
    request_id: &str,
) -> Result<(), AppError> {
    state
        .services
        .audit
        .record_event(build_ws_audit_event(
            session, event_type, transport, outcome, reason, request_id,
        )?)
        .await
}

fn build_ws_audit_event(
    session: &ConnectionSession,
    event_type: &str,
    transport: Option<AuditTransportState>,
    outcome: AuditOutcome,
    reason: Option<&str>,
    request_id: &str,
) -> Result<AuditEvent, AppError> {
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: event_type.to_string(),
        actor_id: None,
        actor_kind: "system".to_string(),
        target_type: Some("connection_session".to_string()),
        target_id: Some(session.id),
        metadata: json!({
            "target_id": session.target_id.to_string(),
            "protocol": session.protocol.as_str(),
            "session_owner_id": session.user_id.to_string(),
            "proxy_id": session.proxy_id.map(|value| value.to_string()),
            "proxy_node_id": session.proxy_node_id.map(|value| value.to_string()),
            "routing_source": session.routing_source.as_str(),
            "reason": reason,
        }),
        created_at: Utc::now(),
    }
    .with_context(AuditContext {
        outcome,
        state: transport.map(AuditLifecycleState::Transport),
        emitter: Some(AuditEmitter {
            kind: AuditEmitterKind::Api,
            id: None,
        }),
        correlation: AuditCorrelation {
            request_id: Uuid::parse_str(request_id).ok(),
            session_id: Some(session.id),
            ..Default::default()
        },
        ..Default::default()
    })
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
    use crate::contexts::audit::domain::{
        AuditDecision, AuditEmitterKind, AuditEvent, AuditLifecycleState, AuditOutcome,
        AuditTransportState,
    };
    use crate::proxy::RdpDisplayParams;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    #[test]
    fn websocket_transport_audit_does_not_attribute_network_end_to_owner() {
        use crate::contexts::connections::domain::{
            ConnectionProtocol, ConnectionSession, ConnectionStatus, RoutingSource,
        };
        let owner = uuid::Uuid::now_v7();
        let session = ConnectionSession {
            id: uuid::Uuid::now_v7(),
            target_id: uuid::Uuid::now_v7(),
            user_id: owner,
            protocol: ConnectionProtocol::Ssh,
            status: ConnectionStatus::Active,
            proxy_id: Some(uuid::Uuid::now_v7()),
            proxy_node_id: None,
            routing_source: RoutingSource::EmbeddedDefault,
            client_ip: None,
            last_activity: chrono::Utc::now(),
            session_metadata: serde_json::json!({}),
            started_at: chrono::Utc::now(),
            ended_at: None,
        };
        let request_id = uuid::Uuid::now_v7();
        let event = super::build_ws_audit_event(
            &session,
            "connection.session.websocket.disconnected",
            Some(AuditTransportState::Detached),
            WsDisconnectReason::ClientClosed.audit_outcome(),
            Some("client_closed"),
            &request_id.to_string(),
        )
        .expect("typed transport event");
        assert_eq!(event.actor_id, None);
        assert_eq!(event.actor_kind, "system");
        assert_eq!(event.target_id, Some(session.id));
        let context = event.audit_context();
        assert_eq!(context.outcome, AuditOutcome::NotApplicable);
        assert_eq!(
            context.state,
            Some(AuditLifecycleState::Transport(
                AuditTransportState::Detached
            ))
        );
        assert_eq!(context.correlation.request_id, Some(request_id));
        assert_eq!(context.correlation.session_id, Some(session.id));
        assert_eq!(
            context.emitter.as_ref().unwrap().kind,
            AuditEmitterKind::Api
        );
        assert!(event.metadata.get("status").is_none());
        assert_eq!(event.metadata["reason"], "client_closed");
        assert_eq!(event.metadata["session_owner_id"], owner.to_string());
        let restored: AuditEvent =
            serde_json::from_value(serde_json::to_value(&event).unwrap()).unwrap();
        assert_eq!(restored.audit_context(), context);

        let refused = super::build_ws_audit_event(
            &session,
            "connection.session.websocket.attach_denied",
            None,
            AuditOutcome::Denied,
            Some(super::WS_REASON_CONCURRENT_ATTACH_REJECTED),
            &request_id.to_string(),
        )
        .expect("typed attach refusal");
        assert_eq!(refused.audit_context().outcome, AuditOutcome::Denied);
        assert_eq!(refused.audit_context().state, None);
    }

    #[test]
    fn observer_stop_audit_keeps_observer_and_request_without_claiming_initiator() {
        let observer = uuid::Uuid::now_v7();
        let session = uuid::Uuid::now_v7();
        let target = uuid::Uuid::now_v7();
        let request_id = uuid::Uuid::now_v7();
        let event = super::build_observe_audit_event(
            None,
            session,
            Some(target),
            Some(observer),
            &request_id.to_string(),
            "connection.session.observe.stopped",
            "runtime_ended",
        )
        .expect("typed observer event");
        assert_eq!(event.actor_id, None);
        assert_eq!(event.actor_kind, "system");
        assert_eq!(event.target_id, Some(session));
        assert_eq!(event.metadata["observer_id"], observer.to_string());
        assert_eq!(event.metadata["target_id"], target.to_string());
        let context = event.audit_context();
        assert_eq!(context.correlation.request_id, Some(request_id));
        assert_eq!(context.correlation.session_id, Some(session));
        assert_eq!(context.outcome, AuditOutcome::NotApplicable);
        assert_eq!(
            context.state,
            Some(AuditLifecycleState::Transport(
                AuditTransportState::Disconnected
            ))
        );
        assert_eq!(context.emitter.unwrap().kind, AuditEmitterKind::Api);
        assert_eq!(event.metadata["reason"], "runtime_ended");
    }

    #[test]
    fn observer_admission_is_not_evidence_of_a_connected_display() {
        let observer = uuid::Uuid::now_v7();
        let session = uuid::Uuid::now_v7();
        let event = super::build_observe_audit_event(
            Some(observer),
            session,
            None,
            Some(observer),
            "not-a-request-uuid",
            "connection.session.observe.started",
            "authorized",
        )
        .expect("typed admission event");
        let context = event.audit_context();
        assert_eq!(event.actor_id, Some(observer));
        assert_eq!(context.outcome, AuditOutcome::Unknown);
        assert_eq!(context.decision, Some(AuditDecision::Allowed));
        assert_eq!(context.state, None);
        assert_eq!(context.correlation.request_id, None);
        assert_eq!(context.correlation.session_id, Some(session));
        assert_eq!(context.display.actor, None);
        assert_eq!(context.display.target, None);
    }

    #[test]
    fn observer_denial_does_not_invent_an_unavailable_policy_decision() {
        for (reason, decision) in [
            ("authorization_failed", None),
            ("policy_denied_or_unavailable", None),
            ("input_denied", Some(AuditDecision::Denied)),
            ("origin_denied", Some(AuditDecision::Denied)),
        ] {
            let event = super::build_observe_audit_event(
                None,
                uuid::Uuid::now_v7(),
                None,
                None,
                "unknown",
                "connection.session.observe.denied",
                reason,
            )
            .expect("typed denial event");
            let context = event.audit_context();
            assert_eq!(context.outcome, AuditOutcome::Denied);
            assert_eq!(context.decision, decision);
            assert_eq!(context.state, None);
        }
    }

    #[test]
    fn observer_lease_expiry_boundary_only_accepts_the_exact_pair() {
        use super::{
            OBSERVER_LEASE_EXPIRED_CLOSE_CODE, WS_REASON_OBSERVER_LEASE_EXPIRED,
            is_observer_lease_expiry_close,
        };
        assert!(is_observer_lease_expiry_close(
            OBSERVER_LEASE_EXPIRED_CLOSE_CODE,
            WS_REASON_OBSERVER_LEASE_EXPIRED
        ));
        // Wrong code with the right reason, and the right code with any other
        // reason, stay terminal: no reconnect signal leaks into other closes.
        assert!(!is_observer_lease_expiry_close(
            1008,
            WS_REASON_OBSERVER_LEASE_EXPIRED
        ));
        assert!(!is_observer_lease_expiry_close(1000, "detached_alive"));
        assert!(!is_observer_lease_expiry_close(
            1000,
            "observer_lease_expired "
        ));
        assert!(!is_observer_lease_expiry_close(1001, ""));
    }

    #[test]
    fn concurrent_attach_rejection_ends_only_the_refused_leg() {
        use super::WS_REASON_CONCURRENT_ATTACH_REJECTED;
        let reason = map_proxy_close_reason(
            Some(CloseCode::Policy),
            WS_REASON_CONCURRENT_ATTACH_REJECTED,
        );
        assert_eq!(reason, WsDisconnectReason::ConcurrentAttachRejected);
        assert!(reason.preserves_session());
        assert_eq!(reason.audit_outcome(), AuditOutcome::Denied);
        assert_eq!(reason.as_str(), WS_REASON_CONCURRENT_ATTACH_REJECTED);
    }

    #[test]
    fn other_policy_violations_still_end_the_session() {
        // Any other policy close keeps the previous terminal behaviour: only the
        // exact concurrent-attach cause is leg-scoped.
        let reason = map_proxy_close_reason(
            Some(CloseCode::Policy),
            "Observation generation is no longer available",
        );
        assert_eq!(reason, WsDisconnectReason::ProxyPolicyViolation);
        assert!(!reason.preserves_session());
        assert_eq!(reason.audit_outcome(), AuditOutcome::Denied);
        assert_eq!(
            map_proxy_close_reason(
                Some(CloseCode::Error),
                super::WS_REASON_CONCURRENT_ATTACH_REJECTED
            ),
            WsDisconnectReason::ProxyInternalError
        );
    }

    #[test]
    fn transport_failure_outcome_is_independent_of_preserved_session_lifecycle() {
        assert!(WsDisconnectReason::ClientBridgeError.preserves_session());
        assert_eq!(
            WsDisconnectReason::ClientBridgeError.audit_outcome(),
            AuditOutcome::Failed
        );
        assert_eq!(
            WsDisconnectReason::ProxyPolicyViolation.audit_outcome(),
            AuditOutcome::Denied
        );
    }

    #[test]
    fn observer_cookie_origin_must_match_public_origin_or_allowlist() {
        use axum::http::{HeaderMap, HeaderValue, header};
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("session=existing"));
        headers.insert(header::HOST, HeaderValue::from_static("app.example.test"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.example"),
        );
        assert!(!super::observer_origin_allowed(&headers, &[], false));
        headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
        assert!(!super::observer_origin_allowed(&headers, &[], false));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.example.test"),
        );
        assert!(super::observer_origin_allowed(&headers, &[], false));
        headers.remove(header::ORIGIN);
        assert!(!super::observer_origin_allowed(&headers, &[], false));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer pat_test"),
        );
        assert!(super::observer_origin_allowed(&headers, &[], true));
    }

    #[test]
    fn observer_only_accepts_bounded_sync_and_ack() {
        assert!(super::observer_guacamole_ack("4.sync,3.123;"));
        assert!(super::observer_guacamole_ack("3.ack,1.1,2.OK,1.0;"));
        for input in [
            "3.key,2.65,1.1;",
            "4.sync,3.123;4.sync,1.1;",
            "4.sync,2.1;",
            "4.sync,1.é;",
            "6.select,1.1;",
        ] {
            assert!(!super::observer_guacamole_ack(input), "{input}");
        }
        assert!(!super::observer_guacamole_ack(&"x".repeat(4097)));
    }

    #[test]
    fn observer_heartbeat_accepts_only_a_single_numeric_ping() {
        assert!(super::observer_guacamole_ping("0.,4.ping,3.123;"));
        for bad in [
            "0.,4.ping,2.123;",
            "0.,4.ping,3.abc;",
            "0.,4.pong,3.123;",
            "0.,4.ping,3.123;4.sync,1.1;",
            "0.,4.ping,0.;",
        ] {
            assert!(!super::observer_guacamole_ping(bad), "{bad}");
        }
    }

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
