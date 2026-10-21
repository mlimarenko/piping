//! Verifies the proxy can bridge browser-style RDP sessions via guacd (Guacamole protocol over TCP).
//!
//! The proxy is expected to:
//! - Connect to guacd (TCP)
//! - Perform the guacd handshake on the server side (select/args/connect)
//! - Relay guacamole instructions as WebSocket text frames
//! - Enforce connect args (hostname/port/username/password) from the proxy payload (browser does not send them)

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};
use futures_util::{SinkExt, StreamExt};
use piping_backend::config::ProxyConfig;
use piping_backend::integrations::proxy::{
    ProxySharedSecretProvider, StaticProxySharedSecretProvider, create_proxy_ws_token,
};
use piping_backend::proxy::{ProxyState, build_proxy_router, handle_create_connection};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};

/// How long to wait for the proxy to forward an instruction.
///
/// This is a hang detector, not a latency assertion: locally the whole test
/// finishes in ~20ms. The previous 2s deadline was tight enough that the shared
/// CI runner — serialized to one build job and under memory pressure — timed the
/// handshake out and failed a working proxy. A generous bound still fails a
/// genuinely stuck relay while surviving a loaded machine.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use tokio_tungstenite::tungstenite::protocol::Message;
use uuid::Uuid;

fn encode_instruction(elements: &[&str]) -> String {
    let mut out = String::new();
    for (idx, element) in elements.iter().enumerate() {
        out.push_str(&element.len().to_string());
        out.push('.');
        out.push_str(element);
        out.push(if idx + 1 == elements.len() { ';' } else { ',' });
    }
    out
}

fn parse_instruction(buf: &[u8]) -> Option<(usize, Vec<String>)> {
    let mut offset = 0usize;
    let mut elements: Vec<String> = Vec::new();
    loop {
        if offset >= buf.len() {
            return None;
        }
        let start_digits = offset;
        while offset < buf.len() && buf[offset].is_ascii_digit() {
            offset += 1;
        }
        if offset == start_digits || offset >= buf.len() || buf[offset] != b'.' {
            return None;
        }
        let len = std::str::from_utf8(&buf[start_digits..offset])
            .ok()
            .and_then(|s| s.parse::<usize>().ok())?;
        offset += 1;
        if buf.len() < offset + len + 1 {
            return None;
        }
        let data = &buf[offset..offset + len];
        offset += len;
        let delimiter = buf[offset];
        offset += 1;
        elements.push(String::from_utf8_lossy(data).to_string());
        match delimiter {
            b',' => continue,
            b';' => return Some((offset, elements)),
            _ => return None,
        }
    }
}

async fn read_instruction_from_stream(
    stream: &mut tokio::net::TcpStream,
    buffer: &mut Vec<u8>,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut chunk = [0u8; 1024];
    loop {
        if let Some((consumed, elements)) = parse_instruction(buffer) {
            buffer.drain(..consumed);
            return Ok(elements);
        }
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("guacd read: {e}"))?;
        assert!(n > 0, "guacd peer closed before instruction");
        buffer.extend_from_slice(&chunk[..n]);
    }
}

#[tokio::test]
async fn proxy_rdp_guacd_performs_handshake_and_enforces_connect_args() {
    let node_id = "test-proxy-node";
    let shared_secret = "test-proxy-secret";

    // Fake guacd server.
    let guacd_listener = TcpListener::bind("127.0.0.1:0").await.expect("guacd bind");
    let guacd_addr = guacd_listener.local_addr().expect("guacd addr");

    let expected_host = "192.0.2.55";
    let expected_port = "3389";
    let expected_username = "leader";
    let expected_password = "test-password";

    let args_instruction = encode_instruction(&[
        "args",
        "VERSION_1_6_0",
        "hostname",
        "port",
        "username",
        "password",
    ]);
    let ready_instruction = encode_instruction(&["ready", "1"]);
    let nop_instruction = encode_instruction(&["nop"]);

    let args_instruction_for_guacd = args_instruction.clone();
    let ready_instruction_for_guacd = ready_instruction.clone();
    let guacd_handle = tokio::spawn(async move {
        let (mut stream, _peer) = guacd_listener.accept().await.expect("guacd accept");
        let mut buf: Vec<u8> = Vec::new();

        let select = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read select instruction");
        assert_eq!(select.first().map(String::as_str), Some("select"));
        assert_eq!(select.get(1).map(String::as_str), Some("rdp"));

        stream
            .write_all(args_instruction_for_guacd.as_bytes())
            .await
            .expect("guacd write args");

        let size = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read size instruction");
        assert_eq!(size.first().map(String::as_str), Some("size"));
        assert!(
            size.len() >= 4,
            "expected size instruction with width/height/dpi"
        );

        let audio = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read audio instruction");
        assert_eq!(audio.first().map(String::as_str), Some("audio"));

        let video = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read video instruction");
        assert_eq!(video.first().map(String::as_str), Some("video"));

        let image = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read image instruction");
        assert_eq!(image.first().map(String::as_str), Some("image"));

        let timezone = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read timezone instruction");
        assert_eq!(timezone.first().map(String::as_str), Some("timezone"));

        let connect = read_instruction_from_stream(&mut stream, &mut buf)
            .await
            .expect("read connect instruction");
        assert_eq!(connect.first().map(String::as_str), Some("connect"));
        assert_eq!(connect.get(1).map(String::as_str), Some("VERSION_1_6_0"));
        assert_eq!(connect.get(2).map(String::as_str), Some(expected_host));
        assert_eq!(connect.get(3).map(String::as_str), Some(expected_port));
        assert_eq!(connect.get(4).map(String::as_str), Some(expected_username));
        assert_eq!(connect.get(5).map(String::as_str), Some(expected_password));

        stream
            .write_all(ready_instruction_for_guacd.as_bytes())
            .await
            .expect("guacd write ready");

        // The proxy must ignore non-guacamole WS text frames (e.g. connect data), so the next
        // instruction we see must still be a valid guacamole instruction.
        let nop = timeout(
            READY_TIMEOUT,
            read_instruction_from_stream(&mut stream, &mut buf),
        )
        .await
        .expect("nop timeout")
        .expect("read nop instruction");
        assert_eq!(
            nop.first().map(String::as_str),
            Some("nop"),
            "expected forwarded nop instruction"
        );
    });

    // Proxy server (guacd-backed RDP).
    let config = ProxyConfig {
        ws_token_ttl_seconds: 120,
        ..Default::default()
    };
    let shared_secret_provider: Arc<dyn ProxySharedSecretProvider> = Arc::new(
        StaticProxySharedSecretProvider::new(node_id.to_string(), shared_secret.to_string())
            .expect("shared secret provider"),
    );
    let proxy_state = Arc::new(
        ProxyState::new_with_provider(&config, shared_secret_provider)
            .expect("proxy state")
            .with_guacd_target(guacd_addr.ip().to_string(), guacd_addr.port()),
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
        "host": expected_host,
        "port": 3389,
        "credentials": {
            "username": expected_username,
            "password": expected_password,
        },
    });
    let _ = handle_create_connection(proxy_state.clone(), headers, payload).await;

    // Attach WebSocket and run guacamole handshake.
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
    let expected_tunnel_uuid = connection_id.to_string();

    // Proxy must send a tunnel UUID as the first instruction so WebSocketTunnel can start sending.
    let uuid_msg = timeout(READY_TIMEOUT, ws.next())
        .await
        .expect("uuid timeout")
        .expect("uuid stream")
        .expect("uuid message");
    let Message::Text(uuid_text) = uuid_msg else {
        panic!("expected uuid text, got {uuid_msg:?}");
    };
    let uuid_elements = parse_instruction(uuid_text.as_bytes())
        .expect("uuid instruction parse")
        .1;
    assert_eq!(
        uuid_elements.first().map(String::as_str),
        Some(""),
        "expected internal opcode"
    );
    assert_eq!(
        uuid_elements.get(1).map(String::as_str),
        Some(expected_tunnel_uuid.as_str()),
        "expected tunnel UUID = connection id"
    );

    // Some clients send connect data as the first message (not a guacamole instruction).
    ws.send(Message::Text("token=ignored".into()))
        .await
        .expect("ws send connect data");
    // Some naive clients may try to handshake; the proxy must ignore these.
    ws.send(Message::Text(encode_instruction(&["select", "rdp"]).into()))
        .await
        .expect("ws send select (ignored)");
    ws.send(Message::Text(
        encode_instruction(&["connect", "wrong-host", "1234", "u", "p"]).into(),
    ))
    .await
    .expect("ws send connect (ignored)");
    ws.send(Message::Text(nop_instruction.clone().into()))
        .await
        .expect("ws send nop");

    // Proxy consumes `args` internally and should forward `ready`.
    let ready_msg = timeout(READY_TIMEOUT, ws.next())
        .await
        .expect("ready timeout")
        .expect("ready stream")
        .expect("ready message");
    let Message::Text(ready_text) = ready_msg else {
        panic!("expected ready text, got {ready_msg:?}");
    };
    assert_eq!(ready_text.as_str(), ready_instruction.as_str());

    // Shutdown.
    let _ = shutdown_tx.send(());
    proxy_handle.await.expect("proxy join");
    guacd_handle.await.expect("guacd join");
}
