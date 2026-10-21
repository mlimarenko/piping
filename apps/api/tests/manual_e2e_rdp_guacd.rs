//! Manual E2E smoke test for browser-style RDP via local guacd.
//!
//! This test is ignored by default because it requires:
//! - a running guacd instance (default: 127.0.0.1:4822)
//! - an reachable RDP target
//!
//! Run:
//!   PIPING_E2E_RDP_HOST=... PIPING_E2E_RDP_USER=... PIPING_E2E_RDP_PASSWORD=... \
//!   cargo test -q --test manual_e2e_rdp_guacd -- --ignored --nocapture
//!
//! Optional:
//!   PIPING_E2E_RDP_PORT=3389
//!   PIPING_E2E_GUACD_HOST=127.0.0.1
//!   PIPING_E2E_GUACD_PORT=4822

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};
use futures_util::StreamExt;
use piping_backend::config::ProxyConfig;
use piping_backend::integrations::proxy::StaticProxySharedSecretProvider;
use piping_backend::integrations::proxy::create_proxy_ws_token;
use piping_backend::proxy::{ProxyState, build_proxy_router, handle_create_connection};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use tokio_tungstenite::tungstenite::protocol::Message;
use uuid::Uuid;

fn env_required(key: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Ok(std::env::var(key).map_err(|_| format!("Missing required env var: {key}"))?)
}

fn env_u16(key: &str, default_value: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .unwrap_or(default_value)
}

fn parse_instruction(buf: &str) -> Option<Vec<String>> {
    let bytes = buf.as_bytes();
    let mut offset = 0usize;
    let mut elements: Vec<String> = Vec::new();
    loop {
        if offset >= bytes.len() {
            return None;
        }
        let start_digits = offset;
        while offset < bytes.len() && bytes[offset].is_ascii_digit() {
            offset += 1;
        }
        if offset == start_digits || offset >= bytes.len() || bytes[offset] != b'.' {
            return None;
        }
        let len = std::str::from_utf8(&bytes[start_digits..offset])
            .ok()
            .and_then(|s| s.parse::<usize>().ok())?;
        offset += 1;
        if bytes.len() < offset + len + 1 {
            return None;
        }
        let data = &bytes[offset..offset + len];
        offset += len;
        let delimiter = bytes[offset];
        offset += 1;
        elements.push(String::from_utf8_lossy(data).to_string());
        match delimiter {
            b',' => continue,
            b';' => return Some(elements),
            _ => return None,
        }
    }
}

#[tokio::test]
#[ignore]
async fn e2e_rdp_through_proxy_and_guacd_emits_ready()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let required = [
        "PIPING_E2E_RDP_HOST",
        "PIPING_E2E_RDP_USER",
        "PIPING_E2E_RDP_PASSWORD",
    ];
    // Loud, not skipped: the coverage lane excludes `e2e_rdp` by name, so the
    // only way this runs is that someone asked for it. Asking for a live RDP
    // test without a reachable host is an error. It used to `return Ok(())`
    // and be counted as a PASS in CI on every pipeline.
    let unset: Vec<&str> = required
        .iter()
        .copied()
        .filter(|key| {
            std::env::var(key)
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
        })
        .collect();
    if !unset.is_empty() {
        return Err(format!(
            "live RDP environment is not configured: {}",
            unset.join(", ")
        )
        .into());
    }

    let node_id = "manual-e2e-node";
    let shared_secret = "manual-e2e-secret";

    let rdp_host = env_required("PIPING_E2E_RDP_HOST")?;
    let rdp_port = env_u16("PIPING_E2E_RDP_PORT", 3389);
    let rdp_user = env_required("PIPING_E2E_RDP_USER")?;
    let rdp_password = env_required("PIPING_E2E_RDP_PASSWORD")?;

    let guacd_host = std::env::var("PIPING_E2E_GUACD_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let guacd_port = env_u16("PIPING_E2E_GUACD_PORT", 4822);

    // Proxy server wired to local/external guacd.
    let config = ProxyConfig::default();
    let shared_secret_provider = Arc::new(
        StaticProxySharedSecretProvider::new(node_id.to_string(), shared_secret.to_string())
            .expect("shared secret"),
    );
    let proxy_state = Arc::new(
        ProxyState::new_with_provider(&config, shared_secret_provider)
            .expect("proxy state with provider")
            .with_guacd_target(guacd_host, guacd_port),
    );
    let app = build_proxy_router(proxy_state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy bind");
    let proxy_addr = listener.local_addr().expect("proxy addr");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let proxy_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("proxy serve");
    });

    // Create connection entry.
    let connection_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let token = create_proxy_ws_token(shared_secret, connection_id, user_id).expect("token");

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-piping-proxy-node-id",
        HeaderValue::from_str(node_id).expect("node id header"),
    );
    headers.insert(
        "x-piping-proxy-secret",
        HeaderValue::from_str(shared_secret).expect("secret header"),
    );
    let payload = json!({
        "connection_id": connection_id.to_string(),
        "connection_type": "rdp",
        "host": rdp_host,
        "port": rdp_port,
        "credentials": {
            "username": rdp_user,
            "password": rdp_password,
        },
    });
    let _ = handle_create_connection(proxy_state.clone(), headers, payload).await;

    // Attach WebSocket and expect `ready` (guacamole protocol).
    let ws_url = format!(
        "ws://{}/proxy/v1/ws/rdp/{}?token={}",
        proxy_addr, connection_id, token
    );
    let mut request = ws_url.into_client_request().expect("ws request");
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("guacamole"),
    );
    let (mut ws, _response) = connect_async(request).await.expect("ws connect");

    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("ready timeout")
        .expect("ready stream")
        .expect("ready message");
    let Message::Text(text) = msg else {
        panic!("expected text message, got {:?}", msg);
    };
    let elements = parse_instruction(text.as_str()).expect("valid guacamole instruction");
    let opcode = elements.first().map(|s| s.as_str()).unwrap_or("");
    assert_eq!(opcode, "ready", "expected 'ready', got {:?}", elements);

    // Shutdown.
    let _ = shutdown_tx.send(());
    proxy_handle.await.expect("proxy join");
    Ok(())
}
