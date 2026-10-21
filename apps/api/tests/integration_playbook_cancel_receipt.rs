//! Cancellation is addressed by the durable dispatch receipt on both sides of
//! the runner boundary, and terminal receipts remain valid responses to
//! repeated cancel.
//!
//! The backend half is exercised against a fake runner over a real socket:
//! `AnsibleRunnerClient::cancel_dispatch` must POST
//! `/dispatches/{dispatch_id}/cancel` with no `run_group_id` identity. The
//! runner resolves cancellation by the same `dispatch_id` path parameter in its
//! own `apps/ansible-runner` unit tests, so no source-text mirror is kept here.

use std::time::Duration;

use piping_backend::contexts::playbooks::infrastructure::{
    AnsibleDispatchStatus, AnsibleJobState, AnsibleRunnerClient,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

#[allow(
    clippy::expect_used,
    reason = "test-only fake runner running in a detached tokio task; a socket failure here has no caller to propagate to, and the test awaits the JoinHandle, so a panic surfaces as a test failure rather than a hang"
)]
async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).await.expect("read request");
        assert!(count > 0, "client closed before sending a complete request");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8(bytes).expect("HTTP request is UTF-8");
            }
        }
    }
}

/// Serve one cancel request, assert the exact path the backend addressed, and
/// answer with `response_status`/`response_body`.
#[allow(
    clippy::expect_used,
    reason = "test-only fake runner running in a detached tokio task; a socket failure here has no caller to propagate to, and the test awaits the JoinHandle, so a panic surfaces as a test failure rather than a hang"
)]
async fn serve_cancel_once(
    listener: TcpListener,
    expected_path: String,
    response_status: &'static str,
    response_body: String,
) {
    let (mut stream, _) = listener.accept().await.expect("accept cancel request");
    let request = read_request(&mut stream).await;
    let expected_line = format!("POST {expected_path} HTTP/1.1");
    assert_eq!(
        request.lines().next(),
        Some(expected_line.as_str()),
        "backend cancellation must address the durable dispatch receipt"
    );
    assert!(
        !request.contains("run_group_id"),
        "run_group_id must not survive as a second cancellation identity"
    );
    let response = format!(
        "HTTP/1.1 {response_status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("write cancel response");
}

#[allow(
    clippy::expect_used,
    reason = "test-only runner client construction against the fake runner; a failure here is a broken test fixture, not a product error"
)]
fn cancel_client(address: std::net::SocketAddr) -> AnsibleRunnerClient {
    AnsibleRunnerClient::new(
        format!("http://{address}"),
        "test-control-secret".to_string(),
        Duration::from_secs(2),
    )
    .expect("runner client")
}

#[tokio::test]
async fn cancel_dispatch_addresses_the_durable_dispatch_receipt() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake runner");
    let address = listener.local_addr().expect("fake runner address");
    let dispatch_id = Uuid::from_u128(0xCA11CE2);
    let body = serde_json::json!({
        "dispatch_id": dispatch_id,
        "run_group_id": Uuid::from_u128(0x600D),
        "dispatch_attempt": 1,
        "status": "canceled",
        "rc": null
    })
    .to_string();
    let server = tokio::spawn(serve_cancel_once(
        listener,
        format!("/dispatches/{dispatch_id}/cancel"),
        "200 OK",
        body,
    ));

    let status = cancel_client(address)
        .cancel_dispatch(dispatch_id)
        .await
        .expect("cancel reaches the fake runner")
        .expect("a terminal receipt is returned");

    assert_eq!(status.receipt.dispatch_id, dispatch_id);
    assert_eq!(status.status, AnsibleJobState::Canceled);
    server.await.expect("fake runner task");
}

#[tokio::test]
async fn cancel_dispatch_maps_an_unaccepted_dispatch_to_none() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake runner");
    let address = listener.local_addr().expect("fake runner address");
    let dispatch_id = Uuid::from_u128(0xCA11CE3);
    let server = tokio::spawn(serve_cancel_once(
        listener,
        format!("/dispatches/{dispatch_id}/cancel"),
        "404 Not Found",
        String::new(),
    ));

    let status = cancel_client(address)
        .cancel_dispatch(dispatch_id)
        .await
        .expect("cancel transport succeeds");

    assert!(
        status.is_none(),
        "a dispatch the runner never accepted is not a cancellation failure"
    );
    server.await.expect("fake runner task");
}

#[test]
fn terminal_cancel_receipt_round_trips_after_completion()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dispatch_id = Uuid::from_u128(0xCA11CE1);
    let status: AnsibleDispatchStatus = serde_json::from_value(serde_json::json!({
        "dispatch_id": dispatch_id,
        "run_group_id": Uuid::from_u128(0x600D),
        "dispatch_attempt": 2,
        "status": "canceled",
        "rc": null
    }))?;

    assert_eq!(status.receipt.dispatch_id, dispatch_id);
    assert_eq!(status.status, AnsibleJobState::Canceled);
    let repeated = serde_json::from_value::<AnsibleDispatchStatus>(serde_json::to_value(&status)?)?;
    assert_eq!(repeated, status);
    Ok(())
}
