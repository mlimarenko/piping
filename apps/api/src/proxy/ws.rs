use std::sync::Arc;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Number, Value};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::observability::{EventFields, LogLevel, log_error_event, log_event};

use super::metrics::{
    ConnectionMetricsState, mark_connection_metrics_connecting, mark_connection_metrics_error,
};
use super::{ProxyConnectionType, ProxyState, RdpDisplayParams, parse_connection_type};

/// Canonical refusal reason for a *second* attach to a connection that already
/// has a live operator leg. The rejected leg owns no session lifecycle: the
/// existing operator, observer and upstream must survive it, so every emitter
/// and the API classifier share this exact text.
pub(crate) const WS_REASON_CONCURRENT_ATTACH_REJECTED: &str = "concurrent_attach_rejected";

/// Private admission contract: a refused operator attach is answered *before*
/// the WebSocket upgrade, so the caller never receives a socket it could
/// mistake for a live leg. Status and header belong to this route only; the
/// caller must match both (see [`is_concurrent_attach_refusal`]).
const PROXY_ATTACH_REFUSAL_STATUS: StatusCode = StatusCode::CONFLICT;
const PROXY_ATTACH_REFUSAL_HEADER: &str = "x-piping-proxy-attach-refusal";

/// One live operator leg per connection, acquired before the private upgrade.
/// The latch is synchronous on purpose: admission must be decided in the router
/// without yielding, and dropping the leg must release it immediately.
pub(super) struct ActiveBridgeGuard {
    connection_id: Uuid,
    active_bridges: Arc<std::sync::Mutex<std::collections::HashSet<Uuid>>>,
}

impl ActiveBridgeGuard {
    pub(super) fn acquire(state: &Arc<ProxyState>, connection_id: Uuid) -> Option<Self> {
        let mut active = lock_active_bridges(&state.active_bridges);
        if !active.insert(connection_id) {
            return None;
        }
        drop(active);
        Some(Self {
            connection_id,
            active_bridges: state.active_bridges.clone(),
        })
    }
}

impl Drop for ActiveBridgeGuard {
    fn drop(&mut self) {
        lock_active_bridges(&self.active_bridges).remove(&self.connection_id);
    }
}

fn lock_active_bridges(
    active_bridges: &std::sync::Mutex<std::collections::HashSet<Uuid>>,
) -> std::sync::MutexGuard<'_, std::collections::HashSet<Uuid>> {
    active_bridges
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Refuse a second operator attach before the upgrade. The caller gets an HTTP
/// conflict instead of a socket, so it cannot activate a leg, audit a success
/// or start a second upstream; the live operator, observers, generation and
/// upstream are untouched.
pub(super) fn concurrent_attach_refused_response(connection_id: Uuid) -> Response {
    log_event(
        LogLevel::Warning,
        "proxy.websocket.concurrent_attach_rejected",
        EventFields::default()
            .with_operation("ws_proxy_handler")
            .with_resource("proxy_connection", connection_id.to_string()),
    );
    (
        PROXY_ATTACH_REFUSAL_STATUS,
        [(
            PROXY_ATTACH_REFUSAL_HEADER,
            WS_REASON_CONCURRENT_ATTACH_REJECTED,
        )],
        Json(super::ErrorResponse {
            error: WS_REASON_CONCURRENT_ATTACH_REJECTED.to_string(),
        }),
    )
        .into_response()
}

/// True only for this proxy's exact refusal pair. Every other connect failure —
/// policy, token, transport, internal error — stays a genuine setup failure.
pub(crate) fn is_concurrent_attach_refusal(error: &tokio_tungstenite::tungstenite::Error) -> bool {
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        return false;
    };
    response.status() == PROXY_ATTACH_REFUSAL_STATUS
        && response
            .headers()
            .get(PROXY_ATTACH_REFUSAL_HEADER)
            .and_then(|value| value.to_str().ok())
            == Some(WS_REASON_CONCURRENT_ATTACH_REJECTED)
}

pub(super) async fn proxy_ws_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    // Admission was decided before the upgrade; holding this latch here is what
    // keeps the connection single-operator for the whole leg.
    _admission: ActiveBridgeGuard,
    mut socket: axum::extract::ws::WebSocket,
    rdp_display_params: Option<RdpDisplayParams>,
) {
    let entry = {
        let guard = state.connections.read().await;
        guard.get(&connection_id).cloned()
    };
    let Some(entry) = entry else {
        send_close_frame(&mut socket, 1008, "Connection not found").await;
        return;
    };
    let metrics = entry.metrics.clone();

    let mut payload = entry.payload.clone();
    let connection_type = match parse_connection_type(&payload) {
        Ok(value) => value,
        Err(error) => {
            log_event(
                LogLevel::Error,
                "proxy.websocket.connection_type.invalid",
                EventFields::default()
                    .with_operation("proxy_ws_loop")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("error", error),
            );
            mark_connection_metrics_error(&metrics).await;
            send_close_frame(&mut socket, 1008, error).await;
            return;
        }
    };
    if matches!(
        connection_type,
        ProxyConnectionType::Rdp | ProxyConnectionType::Vnc
    ) {
        apply_rdp_display_params(&mut payload, rdp_display_params);
    }

    match connection_type {
        ProxyConnectionType::Ssh => {
            bridge_ssh(&state, connection_id, socket, &payload, &metrics).await;
        }
        ProxyConnectionType::Rdp => {
            bridge_rdp(&state, connection_id, socket, &payload, &metrics).await;
        }
        ProxyConnectionType::Vnc => {
            bridge_rdp(&state, connection_id, socket, &payload, &metrics).await;
        }
        ProxyConnectionType::App => {
            bridge_app(connection_id, socket, &payload, &metrics).await;
        }
    }
}

async fn bridge_ssh(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
    socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) {
    if let Err(error) = super::ssh::proxy_ws_ssh_attachable_loop(
        state.clone(),
        connection_id,
        socket,
        payload,
        metrics.clone(),
    )
    .await
    {
        log_error_event(
            "proxy.ssh.relay.failed",
            "proxy_ssh_error",
            error.to_string(),
            EventFields::default()
                .with_operation("proxy_ssh_bridge")
                .with_resource("proxy_connection", connection_id.to_string()),
        );
        mark_connection_metrics_error(metrics).await;
    }
}

async fn bridge_rdp(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
    socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) {
    mark_connection_metrics_connecting(metrics).await;
    if let Err(error) = super::rdp::proxy_ws_desktop_loop(
        state.clone(),
        connection_id,
        socket,
        payload,
        metrics.clone(),
    )
    .await
    {
        log_error_event(
            "proxy.rdp.guacd.relay.failed",
            "proxy_rdp_guacd_error",
            error.to_string(),
            EventFields::default()
                .with_operation("proxy_rdp_guacd_bridge")
                .with_resource("proxy_connection", connection_id.to_string()),
        );
        mark_connection_metrics_error(metrics).await;
    }
}

async fn bridge_app(
    connection_id: Uuid,
    socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) {
    mark_connection_metrics_connecting(metrics).await;
    super::tcp_relay::proxy_ws_tcp_relay_loop(connection_id, socket, payload, metrics.clone())
        .await;
}

fn apply_rdp_display_params(payload: &mut Value, params: Option<RdpDisplayParams>) {
    let Some(params) = params.filter(|value| value.has_any()) else {
        return;
    };
    let Some(map) = payload.as_object_mut() else {
        return;
    };
    let options = map
        .entry("options".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Value::Object(options) = options else {
        return;
    };

    if let Some(width) = params.width {
        options.insert("width".to_string(), Value::Number(Number::from(width)));
    }
    if let Some(height) = params.height {
        options.insert("height".to_string(), Value::Number(Number::from(height)));
    }
    if let Some(dpi) = params.dpi {
        options.insert("dpi".to_string(), Value::Number(Number::from(dpi)));
    }
}

pub(super) async fn send_close_frame(
    socket: &mut axum::extract::ws::WebSocket,
    code: u16,
    reason: &str,
) {
    let _ = socket
        .send(axum::extract::ws::Message::Close(Some(
            axum::extract::ws::CloseFrame {
                code,
                reason: reason.to_string().into(),
            },
        )))
        .await;
}

pub(super) async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::{RdpDisplayParams, apply_rdp_display_params};
    use serde_json::json;

    #[tokio::test]
    async fn concurrent_attach_is_refused_before_the_upgrade_and_the_live_leg_survives() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        use axum::http::StatusCode;
        use futures_util::{SinkExt, StreamExt};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::RwLock;
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as ClientMessage;
        use uuid::Uuid;

        use crate::integrations::proxy::{
            ProxySharedSecretProvider, StaticProxySharedSecretProvider, create_proxy_ws_token,
        };
        use crate::proxy::ProxyState;
        use crate::proxy::metrics::ConnectionMetricsState;

        use super::{
            PROXY_ATTACH_REFUSAL_HEADER, PROXY_ATTACH_REFUSAL_STATUS,
            WS_REASON_CONCURRENT_ATTACH_REJECTED, is_concurrent_attach_refusal,
        };

        type Client = tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >;

        async fn next_binary(client: &mut Client) -> Vec<u8> {
            loop {
                match tokio::time::timeout(Duration::from_secs(3), client.next())
                    .await
                    .expect("frame deadline")
                    .expect("socket open")
                    .expect("websocket frame")
                {
                    ClientMessage::Binary(bytes) => return bytes.to_vec(),
                    ClientMessage::Text(_) | ClientMessage::Ping(_) | ClientMessage::Pong(_) => {}
                    other => panic!("unexpected frame before echo: {other:?}"),
                }
            }
        }

        // The live leg's upstream: a local echo target that records every
        // accepted connection, so a refused attach that started a second
        // upstream would be visible here.
        let upstream = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("upstream listener");
        let upstream_port = upstream.local_addr().expect("upstream address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_by_upstream = accepted.clone();
        let upstream_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = upstream.accept().await else {
                    return;
                };
                accepted_by_upstream.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 64];
                    loop {
                        let Ok(bytes) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if bytes == 0 {
                            return;
                        }
                        if stream.write_all(&buffer[..bytes]).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });

        let provider = StaticProxySharedSecretProvider::ephemeral_embedded().expect("provider");
        let credential = provider.get().expect("credential");
        let state = Arc::new(
            ProxyState::new_with_provider(
                &crate::config::ProxyConfig::default(),
                Arc::new(provider),
            )
            .expect("proxy state"),
        );
        let connection_id = Uuid::now_v7();
        let entry = super::super::ProxyConnection::new(
            json!({"connection_type": "app", "host": "127.0.0.1", "port": upstream_port}),
            Arc::new(RwLock::new(ConnectionMetricsState::default())),
        );
        let generation = entry.generation;
        state.connections.write().await.insert(connection_id, entry);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().expect("loopback address");
        let router = super::super::build_proxy_router(state.clone());
        let server =
            tokio::spawn(
                async move { axum::serve(listener, router).await.expect("fixture server") },
            );

        let token = create_proxy_ws_token(&credential.secret, connection_id, Uuid::now_v7())
            .expect("operator token");
        // The route names the protocol surface; the stored payload drives the
        // relay. Admission itself is protocol-agnostic, and the app relay gives
        // the test a local upstream it can hold open.
        let url = format!("ws://{address}/proxy/v1/ws/terminal/{connection_id}?token={token}");

        let (mut live, response) = connect_async(url.as_str())
            .await
            .expect("first operator attach is admitted");
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        live.send(ClientMessage::Text("alive".into()))
            .await
            .expect("send through the live leg");
        assert_eq!(next_binary(&mut live).await, b"alive".to_vec());

        let refused = match connect_async(url.as_str()).await {
            Ok(_) => panic!("second attach must be refused before the upgrade"),
            Err(error) => error,
        };
        assert!(
            is_concurrent_attach_refusal(&refused),
            "unexpected second-attach error: {refused}"
        );
        let refusal = match &refused {
            tokio_tungstenite::tungstenite::Error::Http(response) => response,
            other => panic!("a refusal must be an HTTP response, got {other:?}"),
        };
        assert_eq!(refusal.status(), PROXY_ATTACH_REFUSAL_STATUS);
        assert_eq!(
            refusal
                .headers()
                .get(PROXY_ATTACH_REFUSAL_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(WS_REASON_CONCURRENT_ATTACH_REJECTED)
        );

        // The refusal starts no upstream and changes nothing the live leg owns.
        assert_eq!(
            accepted.load(Ordering::SeqCst),
            1,
            "refused attach must not open a second upstream"
        );
        let admission = state
            .active_bridges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(admission.len(), 1);
        assert!(admission.contains(&connection_id));
        drop(admission);
        let current = state
            .connections
            .read()
            .await
            .get(&connection_id)
            .cloned()
            .expect("live connection entry");
        assert_eq!(current.generation, generation);
        assert_eq!(current.observers.load(Ordering::Acquire), 0);
        assert!(!*current.ended_tx.borrow());

        // The admitted leg still relays through its original upstream.
        live.send(ClientMessage::Text("still-alive".into()))
            .await
            .expect("live leg keeps its relay");
        assert_eq!(next_binary(&mut live).await, b"still-alive".to_vec());
        assert_eq!(accepted.load(Ordering::SeqCst), 1);

        // Releasing the live leg releases admission for the next attach.
        live.close(None).await.expect("close live leg");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut reattached = None;
        while tokio::time::Instant::now() < deadline {
            match connect_async(url.as_str()).await {
                Ok(pair) => {
                    reattached = Some(pair);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        let (mut again, response) = reattached.expect("re-attach after the live leg closed");
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        again
            .send(ClientMessage::Text("again".into()))
            .await
            .expect("re-attached leg send");
        assert_eq!(next_binary(&mut again).await, b"again".to_vec());
        assert_eq!(accepted.load(Ordering::SeqCst), 2);

        again.close(None).await.expect("close re-attached leg");
        server.abort();
        upstream_task.abort();
    }

    #[test]
    fn rdp_display_params_merge_into_existing_options() {
        let mut payload = json!({
            "connection_type": "rdp",
            "options": {
                "security": "nla",
                "disable_gfx": true
            }
        });

        apply_rdp_display_params(
            &mut payload,
            Some(RdpDisplayParams {
                width: Some(1216),
                height: Some(794),
                dpi: None,
            }),
        );

        assert_eq!(payload["options"]["security"], "nla");
        assert_eq!(payload["options"]["disable_gfx"], true);
        assert_eq!(payload["options"]["width"], 1216);
        assert_eq!(payload["options"]["height"], 794);
    }

    #[test]
    fn rdp_display_params_create_options_when_missing() {
        let mut payload = json!({ "connection_type": "rdp" });

        apply_rdp_display_params(
            &mut payload,
            Some(RdpDisplayParams {
                width: Some(1024),
                height: None,
                dpi: Some(144),
            }),
        );

        assert_eq!(payload["options"]["width"], 1024);
        assert_eq!(payload["options"]["dpi"], 144);
    }

    #[test]
    fn empty_rdp_display_params_leave_payload_unchanged() {
        let mut payload = json!({ "connection_type": "rdp" });
        let original = payload.clone();

        apply_rdp_display_params(&mut payload, Some(RdpDisplayParams::default()));

        assert_eq!(payload, original);
    }
}
