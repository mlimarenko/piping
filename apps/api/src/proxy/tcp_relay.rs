use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time::timeout;
use uuid::Uuid;

use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_error_event, log_event};

use super::metrics::{
    ConnectionMetricsState, LinkType, TrafficDirection, mark_connection_metrics_connected,
    mark_connection_metrics_disconnected, mark_connection_metrics_error,
    mark_connection_metrics_proxy_unhealthy, record_connection_traffic,
};

const TCP_RELAY_READ_BUFFER_SIZE: usize = 32 * 1024;

/// Rejects a payload that failed validation: record the error, mark the
/// connection unhealthy, and close the browser leg with the same 1008 frame
/// every payload-validation failure uses.
async fn reject_tcp_relay_payload(
    socket: &mut axum::extract::ws::WebSocket,
    connection_id: Uuid,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    error: impl Into<String>,
) {
    log_error_event(
        "proxy.connection.payload.invalid",
        "proxy_payload_error",
        error,
        EventFields::default()
            .with_operation("proxy_ws_loop")
            .with_resource("proxy_connection", connection_id.to_string()),
    );
    mark_connection_metrics_error(metrics).await;
    super::send_close_frame(socket, 1008, "Invalid proxy payload").await;
}

/// Connects to the relay target under the payload's connect timeout. On any
/// failure the caller's socket is closed (1013) and `Err(())` is returned, so
/// the caller only distinguishes "connected" from "already responded".
async fn connect_tcp_relay_target(
    socket: &mut axum::extract::ws::WebSocket,
    connection_id: Uuid,
    target: &str,
    connect_timeout: std::time::Duration,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) -> Result<TcpStream, ()> {
    let connect_started_at = std::time::Instant::now();
    match timeout(connect_timeout, TcpStream::connect(target)).await {
        Ok(Ok(stream)) => {
            let connect_latency_ms = super::elapsed_latency_ms_since(connect_started_at);
            super::configure_interactive_tcp_stream(&stream);
            mark_connection_metrics_connected(metrics, Some(connect_latency_ms)).await;
            Ok(stream)
        }
        Ok(Err(error)) => {
            log_error_event(
                "proxy.tcp_connect.failed",
                "proxy_connect_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("proxy_tcp_connect")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("target", target.to_string()),
            );
            mark_connection_metrics_proxy_unhealthy(metrics, None).await;
            super::send_close_frame(socket, 1013, "Target is unavailable").await;
            Err(())
        }
        Err(_) => {
            log_error_event(
                "proxy.tcp_connect.timeout",
                "proxy_connect_timeout",
                format!(
                    "Connection to target timed out after {}ms",
                    connect_timeout.as_millis()
                ),
                EventFields::default()
                    .with_operation("proxy_tcp_connect")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("target", target.to_string())
                    .with_context("timeout_ms", connect_timeout.as_millis().to_string()),
            );
            mark_connection_metrics_proxy_unhealthy(metrics, None).await;
            super::send_close_frame(socket, 1013, "Target connection timed out").await;
            Err(())
        }
    }
}

pub(super) async fn proxy_ws_tcp_relay_loop(
    connection_id: Uuid,
    mut socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) {
    let (host, port) = match super::extract_host_port(payload) {
        Ok(value) => value,
        Err(error) => {
            reject_tcp_relay_payload(&mut socket, connection_id, &metrics, error).await;
            return;
        }
    };
    let target = format!("{}:{}", host, port);
    let connect_timeout = match super::extract_connect_timeout_duration(payload) {
        Ok(value) => value,
        Err(error) => {
            reject_tcp_relay_payload(&mut socket, connection_id, &metrics, error).await;
            return;
        }
    };
    let Ok(stream) = connect_tcp_relay_target(
        &mut socket,
        connection_id,
        &target,
        connect_timeout,
        &metrics,
    )
    .await
    else {
        return;
    };

    log_event(
        LogLevel::Info,
        "proxy.relay.established",
        EventFields::default()
            .with_operation("proxy_relay")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_status("connected")
            .with_context("target", target.clone()),
    );

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (mut tcp_reader, mut tcp_writer) = stream.into_split();
    let metrics_ws_to_tcp = metrics.clone();
    let metrics_tcp_to_ws = metrics.clone();

    let ws_to_tcp = async {
        while let Some(message) = ws_receiver.next().await {
            let message = message.map_err(|error| {
                AppError::External(format!("Proxy websocket receive failed: {}", error))
            })?;
            match message {
                axum::extract::ws::Message::Text(text) => {
                    let payload_size = text.len();
                    tcp_writer
                        .write_all(text.as_bytes())
                        .await
                        .map_err(|error| {
                            AppError::External(format!("Proxy TCP write failed: {}", error))
                        })?;
                    record_connection_traffic(
                        &metrics_ws_to_tcp,
                        LinkType::ClientProxy,
                        TrafficDirection::Tx,
                        payload_size,
                    )
                    .await;
                    record_connection_traffic(
                        &metrics_ws_to_tcp,
                        LinkType::ProxyProtocol,
                        TrafficDirection::Tx,
                        payload_size,
                    )
                    .await;
                }
                axum::extract::ws::Message::Binary(bytes) => {
                    let payload_size = bytes.len();
                    tcp_writer.write_all(&bytes).await.map_err(|error| {
                        AppError::External(format!("Proxy TCP write failed: {}", error))
                    })?;
                    record_connection_traffic(
                        &metrics_ws_to_tcp,
                        LinkType::ClientProxy,
                        TrafficDirection::Tx,
                        payload_size,
                    )
                    .await;
                    record_connection_traffic(
                        &metrics_ws_to_tcp,
                        LinkType::ProxyProtocol,
                        TrafficDirection::Tx,
                        payload_size,
                    )
                    .await;
                }
                axum::extract::ws::Message::Close(_) => break,
                axum::extract::ws::Message::Ping(_) => {}
                axum::extract::ws::Message::Pong(_) => {}
            }
        }
        Ok::<(), AppError>(())
    };

    let tcp_to_ws = async {
        let mut buffer = vec![0u8; TCP_RELAY_READ_BUFFER_SIZE];
        loop {
            let bytes = match tcp_reader.read(&mut buffer).await {
                Ok(value) => value,
                Err(error) => {
                    let _ = ws_sender
                        .send(axum::extract::ws::Message::Close(Some(
                            axum::extract::ws::CloseFrame {
                                code: 1011,
                                reason: "Target read failed".into(),
                            },
                        )))
                        .await;
                    return Err(AppError::External(format!(
                        "Proxy TCP read failed: {}",
                        error
                    )));
                }
            };
            if bytes == 0 {
                let _ = ws_sender
                    .send(axum::extract::ws::Message::Close(Some(
                        axum::extract::ws::CloseFrame {
                            code: 1000,
                            reason: "Target closed connection".into(),
                        },
                    )))
                    .await;
                break;
            }
            ws_sender
                .send(axum::extract::ws::Message::Binary(
                    buffer[..bytes].to_vec().into(),
                ))
                .await
                .map_err(|error| {
                    AppError::External(format!("Proxy websocket send failed: {}", error))
                })?;
            record_connection_traffic(
                &metrics_tcp_to_ws,
                LinkType::ProxyProtocol,
                TrafficDirection::Rx,
                bytes,
            )
            .await;
            record_connection_traffic(
                &metrics_tcp_to_ws,
                LinkType::ClientProxy,
                TrafficDirection::Rx,
                bytes,
            )
            .await;
        }
        Ok::<(), AppError>(())
    };

    let relay_result = tokio::select! {
        result = ws_to_tcp => ("proxy_relay_ws_to_tcp", result),
        result = tcp_to_ws => ("proxy_relay_tcp_to_ws", result),
    };

    match relay_result {
        (_operation, Ok(())) => {
            mark_connection_metrics_disconnected(&metrics).await;
        }
        (operation, Err(error)) => {
            log_error_event(
                "proxy.relay.failed",
                "proxy_relay_error",
                error.to_string(),
                EventFields::default()
                    .with_operation(operation)
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("target", target),
            );
            mark_connection_metrics_error(&metrics).await;
        }
    }
}
