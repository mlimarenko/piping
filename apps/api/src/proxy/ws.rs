use std::sync::Arc;

use serde_json::{Number, Value};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::observability::{EventFields, LogLevel, log_error_event, log_event};

use super::metrics::{
    ConnectionMetricsState, mark_connection_metrics_connecting, mark_connection_metrics_error,
};
use super::{ProxyConnectionType, ProxyState, RdpDisplayParams, parse_connection_type};

struct ActiveBridgeGuard {
    connection_id: Uuid,
    active_bridges: Arc<RwLock<std::collections::HashSet<Uuid>>>,
}

impl ActiveBridgeGuard {
    async fn acquire(state: &Arc<ProxyState>, connection_id: Uuid) -> Option<Self> {
        let inserted = {
            let mut guard = state.active_bridges.write().await;
            guard.insert(connection_id)
        };
        if !inserted {
            return None;
        }
        Some(Self {
            connection_id,
            active_bridges: state.active_bridges.clone(),
        })
    }
}

impl Drop for ActiveBridgeGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.active_bridges.try_write() {
            guard.remove(&self.connection_id);
            return;
        }

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let active_bridges = self.active_bridges.clone();
            let connection_id = self.connection_id;
            handle.spawn(async move {
                active_bridges.write().await.remove(&connection_id);
            });
        }
    }
}

pub(super) async fn proxy_ws_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
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

    let _active_bridge = match ActiveBridgeGuard::acquire(&state, connection_id).await {
        Some(guard) => guard,
        None => {
            log_event(
                LogLevel::Warning,
                "proxy.websocket.concurrent_attach_rejected",
                EventFields::default()
                    .with_operation("proxy_ws_loop")
                    .with_resource("proxy_connection", connection_id.to_string()),
            );
            send_close_frame(&mut socket, 1008, "Connection is already attached").await;
            return;
        }
    };
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
    if matches!(connection_type, ProxyConnectionType::Rdp) {
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
            bridge_vnc(connection_id, socket, &payload, &metrics).await;
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
    if let Err(error) = super::rdp::proxy_ws_rdp_guacd_loop(
        connection_id,
        socket,
        payload,
        &state.guacd_host,
        state.guacd_port,
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

async fn bridge_vnc(
    connection_id: Uuid,
    socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) {
    mark_connection_metrics_connecting(metrics).await;
    super::vnc::proxy_ws_vnc_loop(connection_id, socket, payload, metrics.clone()).await;
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
