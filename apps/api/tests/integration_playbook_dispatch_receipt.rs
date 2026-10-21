//! Lost runner responses must reconcile the durable acceptance receipt instead
//! of causing the API to create a second playbook execution.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use common::{build_test_app_state, build_test_db_pool, request_with_auth, unique_suffix};
use piping_backend::contexts::playbooks::infrastructure::{
    AnsibleDispatchOutcome, AnsibleJobRequest, AnsibleJobTarget, AnsibleRunnerClient,
    RUNNER_CONTRACT_VERSION,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

fn sample_job(run_group_id: Uuid, dispatch_id: Uuid) -> AnsibleJobRequest {
    AnsibleJobRequest {
        protocol_version: RUNNER_CONTRACT_VERSION,
        dispatch_id,
        dispatch_attempt: 1,
        run_group_id,
        group_callback_token: "test-group-callback-token".to_string(),
        lease_owner: "worker-test".to_string(),
        callback_base_url: "http://backend.test".to_string(),
        playbook: "---\n- hosts: all\n  tasks: []\n".to_string(),
        entrypoint: None,
        project_files: Vec::new(),
        inventory: "host1 ansible_connection=local\n".to_string(),
        targets: vec![AnsibleJobTarget {
            alias: "host1".to_string(),
            run_id: Uuid::from_u128(2),
            job_token: "test-job-token".to_string(),
            target_kind: "connection_target".to_string(),
            target_id: Uuid::from_u128(3),
            source_kind: "device".to_string(),
            source_id: Some(Uuid::from_u128(3)),
            ssh_private_key: None,
            known_hosts_lines: vec![],
        }],
        extravars: json!({}),
        ssh_key: None,
        passwords: BTreeMap::new(),
        settings: json!({}),
        playbook_env: BTreeMap::new(),
    }
}

#[allow(
    clippy::expect_used,
    reason = "test-only fault-injecting runner running in a detached tokio task; a socket failure here has no caller to propagate to, and the test awaits the JoinHandle, so a panic surfaces as a test failure rather than a hang"
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

#[allow(
    clippy::expect_used,
    reason = "test-only fault-injecting runner running in a detached tokio task; a socket failure here has no caller to propagate to, and the test awaits the JoinHandle, so a panic surfaces as a test failure rather than a hang"
)]
async fn serve_fault_injecting_runner(
    listener: TcpListener,
    accepted_executions: Arc<AtomicUsize>,
    run_group_id: Uuid,
    dispatch_id: Uuid,
) {
    let (mut first, _) = listener.accept().await.expect("accept dispatch");
    let request = read_request(&mut first).await;
    assert!(request.starts_with("POST /jobs HTTP/1.1"));
    accepted_executions.fetch_add(1, Ordering::SeqCst);
    // The runner accepted and started the execution, but the connection dies
    // before any HTTP response reaches the API.
    drop(first);

    let (mut lookup, _) = listener.accept().await.expect("accept receipt lookup");
    let request = read_request(&mut lookup).await;
    assert!(request.starts_with(&format!("GET /dispatches/{dispatch_id} HTTP/1.1")));
    let body = json!({
        "dispatch_id": dispatch_id,
        "run_group_id": run_group_id,
        "dispatch_attempt": 1,
        "status": "running",
        "rc": null
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    lookup
        .write_all(response.as_bytes())
        .await
        .expect("write receipt lookup response");
}

#[tokio::test]
async fn accepted_job_with_lost_response_is_reconciled_without_second_execution() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake runner");
    let address = listener.local_addr().expect("fake runner address");
    let accepted_executions = Arc::new(AtomicUsize::new(0));
    let run_group_id = Uuid::from_u128(1);
    let dispatch_id = Uuid::from_u128(0x10);
    let server = tokio::spawn(serve_fault_injecting_runner(
        listener,
        Arc::clone(&accepted_executions),
        run_group_id,
        dispatch_id,
    ));

    let client = AnsibleRunnerClient::new(
        format!("http://{address}"),
        "test-control-secret".to_string(),
        Duration::from_secs(2),
    )
    .expect("runner client");
    let dispatch = client
        .dispatch_job(&sample_job(run_group_id, dispatch_id))
        .await;

    assert!(
        matches!(&dispatch, AnsibleDispatchOutcome::Accepted(_)),
        "lost response must reconcile the accepted execution: {dispatch:?}"
    );
    server.await.expect("fake runner task");
    assert_eq!(
        accepted_executions.load(Ordering::SeqCst),
        1,
        "the API must never start a second execution after runner acceptance"
    );
}

/// A valid `ssh-ed25519` base64 blob; a fixed test vector, not a secret.
const ED25519_BLOB: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIKdwFDb7uXDNTayo3mt/sJyW1Hut13OuWSjKUWvi64be";

/// Seed a running group and a running run leased by `lease_owner`, plus the
/// active group callback token the runner presents on completion. Returns
/// `(run_group_id, run_id, raw_token)`.
async fn seed_running_group_with_group_callback_token(
    db: &sqlx::PgPool,
    lease_owner: &str,
) -> Result<(Uuid, Uuid, String), Box<dyn std::error::Error + Send + Sync>> {
    let suffix = unique_suffix();
    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook (id, slug, name, status, runtime, risk, category)
        VALUES ($1, $2, 'Pin Completion Test', 'active', 'ansible', 'low', 'general')
        "#,
    )
    .bind(playbook_id)
    .bind(format!("pin-completion-{suffix}"))
    .execute(db)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_version (
            id, playbook_id, version, language, runtime, content, content_sha256
        )
        VALUES ($1, $2, 1, 'yaml', 'ansible', '---\n- hosts: all\n', $3)
        "#,
    )
    .bind(version_id)
    .bind(playbook_id)
    .bind("0000000000000000000000000000000000000000000000000000000000000000")
    .execute(db)
    .await?;
    sqlx::query("UPDATE playbook SET current_version_id = $2 WHERE id = $1")
        .bind(playbook_id)
        .bind(version_id)
        .execute(db)
        .await?;

    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, playbook_id, playbook_version_id, trigger_kind, status, total_runs
        )
        VALUES ($1, $2, $3, 'manual', 'running', 1)
        "#,
    )
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(db)
    .await?;

    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run (
            id, run_group_id, playbook_id, playbook_version_id,
            target_kind, target_id, source_kind, source_id, status, lease_owner
        )
        VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $6, 'running', $7)
        "#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(lease_owner)
    .execute(db)
    .await?;

    // Group callback token, minted the way `PlaybookService` does: the raw
    // value is `pjt_<token_id>_<secret>` and only its SHA-256 hex is stored.
    let token_id = Uuid::now_v7();
    let secret = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
    let raw_token = format!("pjt_{token_id}_{secret}");
    let token_hash = format!("{:x}", Sha256::digest(raw_token.as_bytes()));
    let token_prefix = format!("pjt_{}", &secret[..8]);
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group_callback_token (
            id, run_group_id, token_hash, token_prefix, status, expires_at
        )
        VALUES ($1, $2, $3, $4, 'active', now() + interval '1 hour')
        "#,
    )
    .bind(token_id)
    .bind(group_id)
    .bind(token_hash)
    .bind(token_prefix)
    .execute(db)
    .await?;

    Ok((group_id, run_id, raw_token))
}

async fn cleanup_seeded_group(
    db: &sqlx::PgPool,
    group_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(db)
        .await?;
    Ok(())
}

/// End-to-end proof of the TOFU harvest: a completion batch that reports
/// `new_host_keys` must leave exactly one pin row through the real
/// HTTP -> service -> repository -> `upsert_lines` path.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis with PIPING_DATABASE__URL"]
async fn completion_batch_new_host_keys_are_persisted_as_pins()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;

    let lease_owner = format!("worker-test-{}", unique_suffix());
    let (group_id, run_id, token) =
        seed_running_group_with_group_callback_token(&db, &lease_owner).await?;

    // The test owns this endpoint; a prior failed run may have left a pin.
    let suffix = unique_suffix();
    let host = format!("pwa14-completion-{suffix}.example.org");
    let line = format!("{host} ssh-ed25519 {ED25519_BLOB}");
    sqlx::query("DELETE FROM ssh_host_key_pin WHERE host = $1 AND port = $2")
        .bind(&host)
        .bind(22_i32)
        .execute(&db)
        .await?;

    let body = json!({
        "lease_owner": lease_owner,
        "runs": [{
            "run_id": run_id,
            "status": "succeeded",
            "exit_code": 0,
            "error_message": null
        }],
        "new_host_keys": [line],
    })
    .to_string();
    let uri = format!("/api/v1/playbooks/run-groups/{group_id}/completion-batch");
    let (status, response) = request_with_auth(&app, "POST", &uri, Some(&body), None, Some(&token))
        .await
        .expect("completion batch request");
    assert_eq!(
        status,
        StatusCode::OK,
        "completion batch rejected: {}",
        String::from_utf8_lossy(&response)
    );

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT known_hosts_line, fingerprint FROM ssh_host_key_pin WHERE host = $1 AND port = $2",
    )
    .bind(&host)
    .bind(22_i32)
    .fetch_all(&db)
    .await?;
    assert_eq!(
        rows.len(),
        1,
        "the reported key must leave exactly one pin row"
    );
    assert_eq!(rows[0].0, line, "the pin must store the reported line");
    assert!(
        rows[0].1.starts_with("SHA256:"),
        "the pin must carry an OpenSSH fingerprint: {}",
        rows[0].1
    );

    // Teardown: drop this endpoint's pin and the seeded group (cascades runs,
    // tokens and events).
    sqlx::query("DELETE FROM ssh_host_key_pin WHERE host = $1 AND port = $2")
        .bind(&host)
        .bind(22_i32)
        .execute(&db)
        .await?;
    cleanup_seeded_group(&db, group_id).await?;
    Ok(())
}
