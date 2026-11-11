use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use chrono::Utc;
use futures_util::{Sink, SinkExt};
use uuid::Uuid;

use crate::errors::AppError;
use crate::integrations::proxy::{ProxyWsMode, ProxyWsTokenPayload};

use super::{
    ProxyConnection, ProxyConnectionType, ProxyMetricsResponse, ProxyRuntimePresence, ProxyState,
    parse_connection_type,
};

const MAX_OBSERVERS_PER_SESSION: u32 = 32;

struct ObserverGuard(Arc<AtomicU32>);

impl ObserverGuard {
    fn acquire(counter: Arc<AtomicU32>) -> Option<Self> {
        counter
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_OBSERVERS_PER_SESSION).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(counter))
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn permits_generation(token: &ProxyWsTokenPayload, entry: &ProxyConnection) -> bool {
    token.mode == ProxyWsMode::Observe
        && token.generation == Some(entry.generation)
        && !*entry.ended_tx.borrow()
        && token.expires_at > Utc::now().timestamp().max(0) as u64
}

fn apply_transport_evidence(presence: &mut ProxyRuntimePresence, metrics: &ProxyMetricsResponse) {
    if presence.state != "live" {
        return;
    }
    match metrics
        .links
        .iter()
        .find(|link| link.link_type == "proxy_protocol")
        .map(|link| link.status.as_str())
    {
        Some("healthy") => {}
        Some("degraded" | "unhealthy") => presence.state = "degraded".to_string(),
        _ => presence.state = "unknown".to_string(),
    }
}

pub(super) async fn runtime_presence(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
) -> Option<ProxyRuntimePresence> {
    let entry = state.connections.read().await.get(&connection_id).cloned();
    let Some(entry) = entry else {
        return state
            .retired_connections
            .read()
            .await
            .get(&connection_id)
            .cloned();
    };
    let mut presence = match parse_connection_type(&entry.payload).ok()? {
        ProxyConnectionType::Ssh => super::ssh::runtime_presence(state, connection_id).await,
        ProxyConnectionType::Rdp | ProxyConnectionType::Vnc => {
            super::rdp::runtime_presence(state, connection_id).await
        }
        ProxyConnectionType::App => None,
    }
    .unwrap_or_else(ProxyRuntimePresence::unknown);
    apply_transport_evidence(
        &mut presence,
        &entry.metrics.read().await.as_response(connection_id),
    );
    if state
        .connections
        .read()
        .await
        .get(&connection_id)
        .is_none_or(|current| current.generation != entry.generation)
    {
        return Some(ProxyRuntimePresence::unknown());
    }
    presence.generation = Some(entry.generation.to_string());
    presence.observer_count = Some(entry.observers.load(Ordering::Acquire));
    presence.observed_at = Some(Utc::now().to_rfc3339());
    Some(presence)
}

/// Canonical observer lease-expiry close. Owned here, re-exported from
/// `proxy` for the API classifier; never duplicated as an independent text.
pub(crate) const OBSERVER_LEASE_EXPIRED_CLOSE_CODE: u16 = 1000;
pub(crate) const WS_REASON_OBSERVER_LEASE_EXPIRED: &str = "observer_lease_expired";

/// True only for the exact expiry pair, so the viewer reconnect signal cannot
/// leak into policy closes, revocation, generation loss or transport errors.
pub(crate) fn is_observer_lease_expiry_close(code: u16, reason: &str) -> bool {
    code == OBSERVER_LEASE_EXPIRED_CLOSE_CODE && reason == WS_REASON_OBSERVER_LEASE_EXPIRED
}

/// Stop an expired observer leg without waiting indefinitely for a stalled peer.
pub(super) async fn send_observer_lease_close<S>(sender: &mut S)
where
    S: Sink<Message> + Unpin,
{
    let frame = Message::Close(Some(CloseFrame {
        code: OBSERVER_LEASE_EXPIRED_CLOSE_CODE,
        reason: WS_REASON_OBSERVER_LEASE_EXPIRED.into(),
    }));
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), sender.send(frame)).await;
}

/// Observer lifecycle never acquires the operator bridge lock or terminates its runtime.
pub(super) async fn proxy_ws_observe_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    token: ProxyWsTokenPayload,
    mut socket: WebSocket,
) {
    let remaining = token
        .expires_at
        .saturating_sub(Utc::now().timestamp().max(0) as u64);
    let Some(expiry) =
        tokio::time::Instant::now().checked_add(std::time::Duration::from_secs(remaining))
    else {
        super::send_close_frame(&mut socket, 1008, "Invalid observation expiry").await;
        return;
    };
    let entry = match tokio::time::timeout_at(expiry, async {
        state.connections.read().await.get(&connection_id).cloned()
    })
    .await
    {
        Ok(entry) => entry,
        Err(_) => {
            send_observer_lease_close(&mut socket).await;
            return;
        }
    };
    let Some(entry) = entry.filter(|entry| permits_generation(&token, entry)) else {
        super::send_close_frame(
            &mut socket,
            1008,
            "Observation generation is no longer available",
        )
        .await;
        return;
    };
    let connection_type = match parse_connection_type(&entry.payload) {
        Ok(
            value
            @ (ProxyConnectionType::Ssh | ProxyConnectionType::Rdp | ProxyConnectionType::Vnc),
        ) => value,
        _ => {
            super::send_close_frame(&mut socket, 1008, "Observation is not supported").await;
            return;
        }
    };
    let Some(_observer) = ObserverGuard::acquire(entry.observers.clone()) else {
        super::send_close_frame(&mut socket, 1013, "Observer limit reached").await;
        return;
    };
    let mut ended = entry.ended_tx.subscribe();
    let observe = async move {
        match connection_type {
            ProxyConnectionType::Ssh => {
                super::ssh::proxy_ws_ssh_observe_loop(
                    state,
                    connection_id,
                    socket,
                    entry.generation,
                    expiry,
                )
                .await
            }
            ProxyConnectionType::Rdp | ProxyConnectionType::Vnc => {
                super::rdp::proxy_ws_desktop_observe_loop(
                    state,
                    connection_id,
                    socket,
                    entry.generation,
                    expiry,
                )
                .await
            }
            ProxyConnectionType::App => Err(AppError::Forbidden(
                "Observation is not supported".to_string(),
            )),
        }
    };
    tokio::select! {
        result = observe => {
            if result.is_err() {
                tracing::warn!(event = "proxy.observation.failed", %connection_id, "Observation leg closed");
            }
        }
        _ = ended.wait_for(|value| *value) => {}
        // The deadline is enforced by the socket owner, which emits the
        // canonical expiry close; no outer arm may race to drop the socket.
    }
}

#[cfg(test)]
pub(in crate::proxy) mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::time::Duration;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    pub(in crate::proxy) struct WebSocketPair {
        pub(in crate::proxy) server_socket: WebSocket,
        pub(in crate::proxy) client_socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    }

    impl WebSocketPair {
        pub(in crate::proxy) async fn open() -> Self {
            use axum::{Router, extract::ws::WebSocketUpgrade, routing::get};
            let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
            let app = Router::new().route(
                "/",
                get(move |upgrade: WebSocketUpgrade| {
                    let sender = sender.clone();
                    async move {
                        upgrade.on_upgrade(move |socket| async move {
                            let _ = sender.send(socket).await;
                        })
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let connected = tokio::time::timeout(Duration::from_secs(3), async {
                let (client_socket, _) =
                    tokio_tungstenite::connect_async(format!("ws://{address}/"))
                        .await
                        .unwrap();
                let server_socket = receiver.recv().await.unwrap();
                (server_socket, client_socket)
            })
            .await;
            server.abort();
            // The upgraded socket is independent of the HTTP listener.
            let _ = server.await;
            let (server_socket, client_socket) = connected.expect("websocket upgrade");
            Self {
                server_socket,
                client_socket,
            }
        }
    }

    pub(in crate::proxy) async fn expect_lease_close(
        client: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    ) {
        use tokio_tungstenite::tungstenite::Message as ClientMessage;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match client
                    .next()
                    .await
                    .expect("lease close frame")
                    .expect("websocket frame")
                {
                    ClientMessage::Close(Some(frame)) => {
                        assert_eq!(u16::from(frame.code), 1000);
                        assert_eq!(frame.reason, "observer_lease_expired");
                        break;
                    }
                    ClientMessage::Text(_) | ClientMessage::Ping(_) | ClientMessage::Pong(_) => {}
                    other => panic!("unexpected observation frame: {other:?}"),
                }
            }
        })
        .await
        .expect("lease close deadline");
    }

    #[tokio::test]
    async fn observer_lease_expiry_bounds_ssh_and_desktop_resolution() {
        let state = Arc::new(ProxyState::new(&crate::config::ProxyConfig::default()).unwrap());
        let _blocked_resolution = state.connections.write().await;
        for desktop in [false, true] {
            let WebSocketPair {
                server_socket,
                mut client_socket,
            } = WebSocketPair::open().await;
            let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
            let observe = async {
                if desktop {
                    super::super::rdp::proxy_ws_desktop_observe_loop(
                        state.clone(),
                        Uuid::now_v7(),
                        server_socket,
                        Uuid::now_v7(),
                        deadline,
                    )
                    .await
                } else {
                    super::super::ssh::proxy_ws_ssh_observe_loop(
                        state.clone(),
                        Uuid::now_v7(),
                        server_socket,
                        Uuid::now_v7(),
                        deadline,
                    )
                    .await
                }
            };
            let (result, ()) = tokio::join!(observe, expect_lease_close(&mut client_socket));
            result.expect("observer expiry is not a target failure");
        }
    }

    use tokio::sync::RwLock;

    #[test]
    fn observer_token_cannot_follow_a_recreated_connection() {
        let entry = ProxyConnection::new(
            serde_json::json!({}),
            Arc::new(RwLock::new(Default::default())),
        );
        let mut token = ProxyWsTokenPayload {
            connection_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            mode: ProxyWsMode::Observe,
            generation: Some(entry.generation),
            expires_at: Utc::now().timestamp() as u64 + 60,
        };
        assert!(permits_generation(&token, &entry));
        token.mode = ProxyWsMode::Operator;
        assert!(!permits_generation(&token, &entry));
        token.mode = ProxyWsMode::Observe;
        token.generation = Some(Uuid::now_v7());
        assert!(!permits_generation(&token, &entry));
        token.generation = Some(entry.generation);
        entry.ended_tx.send_replace(true);
        assert!(!permits_generation(&token, &entry));
    }

    #[test]
    fn observers_are_independent_and_bounded() {
        let count = Arc::new(AtomicU32::new(0));
        let observers = (0..MAX_OBSERVERS_PER_SESSION)
            .map(|_| ObserverGuard::acquire(count.clone()).expect("capacity"))
            .collect::<Vec<_>>();
        assert!(ObserverGuard::acquire(count.clone()).is_none());
        assert_eq!(count.load(Ordering::Acquire), MAX_OBSERVERS_PER_SESSION);
        drop(observers);
        assert_eq!(count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn runtime_readiness_does_not_hide_degraded_transport_or_treat_idle_as_dead() {
        let id = Uuid::now_v7();
        let mut metrics = super::super::metrics::ConnectionMetricsState::default();
        metrics.mark_connected(None);
        let mut live = ProxyRuntimePresence {
            state: "live".to_string(),
            ..Default::default()
        };
        apply_transport_evidence(&mut live, &metrics.as_response(id));
        assert_eq!(live.state, "live");
        // No traffic was recorded. Readiness and transport evidence, not byte rate,
        // establish the live state.
        metrics.mark_proxy_unhealthy(None);
        apply_transport_evidence(&mut live, &metrics.as_response(id));
        assert_eq!(live.state, "degraded");
        let mut detached = ProxyRuntimePresence {
            state: "detached".to_string(),
            ..Default::default()
        };
        metrics.mark_disconnected();
        apply_transport_evidence(&mut detached, &metrics.as_response(id));
        assert_eq!(detached.state, "detached");
        let mut unconfirmed = ProxyRuntimePresence {
            state: "live".to_string(),
            ..Default::default()
        };
        apply_transport_evidence(&mut unconfirmed, &metrics.as_response(id));
        assert_eq!(unconfirmed.state, "unknown");
    }
}
