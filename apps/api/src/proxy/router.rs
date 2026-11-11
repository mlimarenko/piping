use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::Value;
use tokio::{process::Command, sync::RwLock, time::timeout};
use uuid::Uuid;

use crate::integrations::proxy::{ProxyWsMode, validate_proxy_ws_token_from_provider};

use super::{
    ConnectionMetricsState, ProxyConnection, ProxyCreateResponse, ProxyState, ProxyWsQuery,
    RdpDisplayParams,
};

pub fn build_proxy_router(state: Arc<ProxyState>) -> Router {
    Router::new()
        .route("/health", get(proxy_health))
        .route("/proxy/v1/connections", post(create_connection))
        .route(
            "/proxy/v1/connections/{connection_id}",
            delete(delete_connection),
        )
        .route(
            "/proxy/v1/connections/{connection_id}/metrics",
            get(get_metrics),
        )
        .route("/proxy/v1/ws/terminal/{connection_id}", get(ws_terminal))
        .route("/proxy/v1/ws/rdp/{connection_id}", get(ws_rdp))
        .route("/proxy/v1/ws/vnc/{connection_id}", get(ws_vnc))
        .route("/proxy/v1/ws/observe/{connection_id}", get(ws_observe))
        .with_state(state)
}

pub async fn serve_proxy(
    state: Arc<ProxyState>,
    bind: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_proxy_router(state);
    let addr = bind
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("Invalid bind address: {}", e))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(super::shutdown_signal())
        .await?;
    Ok(())
}

async fn proxy_health() -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}

async fn create_connection(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    handle_create_connection(state, headers, payload).await
}

async fn delete_connection(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Path(connection_id): Path<Uuid>,
) -> Response {
    handle_delete_connection(state, headers, connection_id).await
}

async fn get_metrics(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Path(connection_id): Path<Uuid>,
) -> Response {
    handle_get_metrics(state, headers, connection_id).await
}

async fn ws_terminal(
    State(state): State<Arc<ProxyState>>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    ws_proxy_handler(state, connection_id, query.token, None, ws, &["guacamole"])
}

async fn ws_rdp(
    State(state): State<Arc<ProxyState>>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    ws_proxy_handler(
        state,
        connection_id,
        query.token,
        Some(RdpDisplayParams {
            width: query.width,
            height: query.height,
            dpi: query.dpi,
        }),
        ws,
        &["guacamole"],
    )
}

async fn ws_vnc(
    State(state): State<Arc<ProxyState>>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    ws_proxy_handler(state, connection_id, query.token, None, ws, &["guacamole"])
}

async fn ws_observe(
    State(state): State<Arc<ProxyState>>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    ws_observe_handler(state, connection_id, query.token, ws)
}

pub fn ws_observe_handler(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    token: String,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    match validate_proxy_ws_token_from_provider(
        state.shared_secret_provider.as_ref(),
        &token,
        connection_id,
        state.ws_token_ttl_seconds,
    ) {
        Ok(Some(payload)) if payload.mode == ProxyWsMode::Observe => with_proxy_subprotocols(
            ws.max_message_size(4096).max_frame_size(4096),
            &["guacamole"],
        )
        .on_upgrade(move |socket| {
            super::observation::proxy_ws_observe_loop(state, connection_id, payload, socket)
        }),
        _ => super::error_response(StatusCode::UNAUTHORIZED, "Invalid observation credential"),
    }
}

pub fn ws_proxy_handler(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    token: String,
    rdp_display_params: Option<RdpDisplayParams>,
    ws: axum::extract::ws::WebSocketUpgrade,
    subprotocols: &'static [&'static str],
) -> Response {
    match validate_proxy_ws_token_from_provider(
        state.shared_secret_provider.as_ref(),
        &token,
        connection_id,
        state.ws_token_ttl_seconds,
    ) {
        Ok(Some(payload)) if payload.mode == ProxyWsMode::Operator => {
            // Admission is decided before the upgrade. A refused second attach
            // is an HTTP conflict, never a socket the caller could activate or
            // relay, so the live operator's leg and upstream stay untouched.
            let Some(admission) = super::ws::ActiveBridgeGuard::acquire(&state, connection_id)
            else {
                return super::ws::concurrent_attach_refused_response(connection_id);
            };
            with_proxy_subprotocols(ws, subprotocols).on_upgrade(move |socket| {
                super::proxy_ws_loop(state, connection_id, admission, socket, rdp_display_params)
            })
        }
        Ok(_) => super::error_response(StatusCode::UNAUTHORIZED, "Invalid proxy token"),
        Err(error) => super::error_response(
            StatusCode::BAD_REQUEST,
            format!("Invalid proxy token: {}", error),
        ),
    }
}

fn with_proxy_subprotocols(
    ws: axum::extract::ws::WebSocketUpgrade,
    subprotocols: &'static [&'static str],
) -> axum::extract::ws::WebSocketUpgrade {
    if subprotocols.is_empty() {
        ws
    } else {
        ws.protocols(subprotocols.iter().copied())
    }
}

fn is_probe_only(payload: &Value) -> bool {
    payload
        .as_object()
        .and_then(|map| map.get("probe_only"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub async fn handle_create_connection(
    state: Arc<ProxyState>,
    headers: HeaderMap,
    payload: Value,
) -> Response {
    if let Err(response) = super::require_secret(&state, &headers) {
        return *response;
    }
    let connection_id = match super::extract_connection_id(&payload) {
        Ok(id) => id,
        Err(error) => return super::error_response(StatusCode::BAD_REQUEST, error),
    };
    if let Err(error) = super::parse_connection_type(&payload) {
        return super::error_response(StatusCode::BAD_REQUEST, error);
    }
    let connect_timeout = match super::extract_connect_timeout_duration(&payload) {
        Ok(value) => value,
        Err(error) => return super::error_response(StatusCode::BAD_REQUEST, error),
    };

    if is_probe_only(&payload) {
        let (host, _port) = match super::extract_host_port(&payload) {
            Ok(value) => value,
            Err(error) => return super::error_response(StatusCode::BAD_REQUEST, error),
        };

        let ping_future = Command::new("ping")
            .arg("-c")
            .arg("1")
            .arg("-W")
            .arg("1")
            .arg(host.as_str())
            .output();

        match timeout(connect_timeout, ping_future).await {
            Ok(Ok(output)) if output.status.success() => {
                return (
                    StatusCode::CREATED,
                    Json(ProxyCreateResponse {
                        connection_id: connection_id.to_string(),
                        generation: None,
                    }),
                )
                    .into_response();
            }
            _ => {
                return super::error_response(StatusCode::BAD_GATEWAY, "Target is unreachable");
            }
        }
    }

    let entry = ProxyConnection::new(
        payload,
        Arc::new(RwLock::new(ConnectionMetricsState::default())),
    );
    let generation = entry.generation;
    {
        let mut connections = state.connections.write().await;
        if connections.contains_key(&connection_id) {
            return super::error_response(StatusCode::CONFLICT, "Connection already exists");
        }
        state
            .retired_connections
            .write()
            .await
            .remove(&connection_id);
        connections.insert(connection_id, entry);
    }
    (
        StatusCode::CREATED,
        Json(ProxyCreateResponse {
            connection_id: connection_id.to_string(),
            generation: Some(generation),
        }),
    )
        .into_response()
}

pub async fn handle_delete_connection(
    state: Arc<ProxyState>,
    headers: HeaderMap,
    connection_id: Uuid,
) -> Response {
    if let Err(response) = super::require_secret(&state, &headers) {
        return *response;
    }
    if remove_connection_state(state, connection_id).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        super::error_response(StatusCode::NOT_FOUND, "Connection not found")
    }
}

pub(super) async fn remove_connection_state(state: Arc<ProxyState>, connection_id: Uuid) -> bool {
    let init_lock = super::ssh::ssh_session_init_lock(&state, connection_id).await;
    let _init_guard = init_lock.lock().await;
    let vnc_init_lock = super::vnc::vnc_session_init_lock(&state, connection_id).await;
    let _vnc_init_guard = vnc_init_lock.lock().await;

    let desktop_init_lock = state
        .desktop_session_init_locks
        .read()
        .await
        .get(&connection_id)
        .cloned();
    let _desktop_init_guard = match &desktop_init_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let prior_presence = super::observation::runtime_presence(&state, connection_id).await;
    let removed = state.connections.write().await.remove(&connection_id);
    let Some(removed) = removed else {
        clear_connection_registries(&state, connection_id).await;
        return false;
    };
    removed.ended_tx.send_replace(true);
    let now = chrono::Utc::now();
    let terminal = super::ProxyRuntimePresence {
        state: "ended".to_string(),
        generation: Some(removed.generation.to_string()),
        operator_attached: Some(false),
        observer_count: Some(0),
        observed_at: Some(now.to_rfc3339()),
        ended_at: Some(
            prior_presence
                .as_ref()
                .and_then(|value| value.ended_at.clone())
                .unwrap_or_else(|| now.to_rfc3339()),
        ),
        last_operator_activity_at: prior_presence.and_then(|value| value.last_operator_activity_at),
        ..Default::default()
    };
    {
        let mut retired = state.retired_connections.write().await;
        let oldest = (now - chrono::Duration::hours(6)).to_rfc3339();
        retired.retain(|_, value| value.ended_at.as_ref().is_some_and(|time| time >= &oldest));
        if retired.len() >= 8192
            && let Some(id) = retired
                .iter()
                .min_by_key(|(_, value)| &value.ended_at)
                .map(|(id, _)| *id)
        {
            retired.remove(&id);
        }
        retired.insert(connection_id, terminal);
    }

    shutdown_removed_sessions(&state, connection_id).await;
    clear_connection_registries(&state, connection_id).await;
    true
}

/// Shuts down live session runtimes and drops their registry entries. Runs
/// after the connection entry itself is gone, so a re-attach cannot observe a
/// half-removed session.
async fn shutdown_removed_sessions(state: &Arc<ProxyState>, connection_id: Uuid) {
    if let Some(ssh_session) = state.ssh_sessions.write().await.remove(&connection_id) {
        ssh_session.shutdown().await;
    }
    state.vnc_sessions.write().await.remove(&connection_id);
    if let Some(desktop) = state.desktop_sessions.write().await.remove(&connection_id) {
        desktop.shutdown().await;
    }
}

/// Drops the per-connection init locks and file sidecar records. Shared by the
/// found and not-found removal paths, which must leave the same registry state.
async fn clear_connection_registries(state: &Arc<ProxyState>, connection_id: Uuid) {
    state
        .ssh_session_init_locks
        .write()
        .await
        .remove(&connection_id);
    state
        .ssh_file_session_sidecars
        .write()
        .await
        .remove(&connection_id);
    state
        .vnc_session_init_locks
        .write()
        .await
        .remove(&connection_id);
    state
        .desktop_session_init_locks
        .write()
        .await
        .remove(&connection_id);
}

pub async fn handle_get_metrics(
    state: Arc<ProxyState>,
    headers: HeaderMap,
    connection_id: Uuid,
) -> Response {
    if let Err(response) = super::require_secret(&state, &headers) {
        return *response;
    }
    let metrics = {
        let guard = state.connections.read().await;
        guard.get(&connection_id).map(|entry| entry.metrics.clone())
    };
    let runtime = super::observation::runtime_presence(&state, connection_id).await;
    if metrics.is_none() && runtime.is_none() {
        return super::error_response(StatusCode::NOT_FOUND, "Connection not found");
    }
    let mut response = match metrics {
        Some(metrics) => metrics.read().await.as_response(connection_id),
        None => ConnectionMetricsState::default().as_response(connection_id),
    };
    response.runtime = runtime;
    (StatusCode::OK, Json(response)).into_response()
}
