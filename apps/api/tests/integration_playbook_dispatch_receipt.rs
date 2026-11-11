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
    AnsibleDispatchOutcome, AnsibleJobRequest, AnsibleRunnerClient, RUNNER_CONTRACT_VERSION,
};
use piping_contracts::playbook_runner::{
    ProjectSnapshot, ResolvedInventory, ResolvedLaunch, SnapshotContent, SnapshotEntry,
    SupervisorCallbacks, snapshot_tree_sha256,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

fn sample_job(run_group_id: Uuid, dispatch_id: Uuid) -> AnsibleJobRequest {
    let entries = vec![
        SnapshotEntry {
            path: "inventory.yml".to_string(),
            content: SnapshotContent::File {
                bytes: b"all:\n  hosts: {}\n".to_vec(),
                executable: false,
            },
        },
        SnapshotEntry {
            path: "site.yml".to_string(),
            content: SnapshotContent::File {
                bytes: b"---\n- hosts: all\n  tasks: []\n".to_vec(),
                executable: false,
            },
        },
    ];
    let tree_sha256 = snapshot_tree_sha256(&entries).expect("snapshot digest");
    AnsibleJobRequest {
        protocol_version: RUNNER_CONTRACT_VERSION,
        dispatch_id,
        dispatch_attempt: 1,
        run_group_id,
        callbacks: SupervisorCallbacks {
            group_token: "pjt_group".to_string(),
            run_tokens: BTreeMap::new(),
        },
        sandbox_run_tokens: BTreeMap::new(),
        lease_owner: "worker-test".to_string(),
        callback_base_url: "http://backend.test".to_string(),
        project: ProjectSnapshot {
            project_id: Uuid::from_u128(1),
            revision_id: Uuid::from_u128(2),
            tree_sha256,
            source_commit_oid: None,
            entries,
        },
        launch: ResolvedLaunch {
            image: "registry.example/ansible".to_string(),
            image_digest: format!("sha256:{}", "0".repeat(64)),
            workdir: ".".to_string(),
            playbook_paths: vec!["site.yml".to_string()],
            inventory: ResolvedInventory::Native {
                sources: vec!["inventory.yml".to_string()],
            },
            connection: None,
            targets: Vec::new(),
            argv: Vec::new(),
            extra_vars: json!({}),
            limit: None,
            env: BTreeMap::new(),
            secrets: BTreeMap::new(),
            timeout_seconds: 60,
        },
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

/// Seed a native zero-host dispatch and its active group callback token.
async fn seed_running_group_with_group_callback_token(
    db: &sqlx::PgPool,
    lease_owner: &str,
) -> Result<(Uuid, Uuid, Uuid, Uuid, Uuid, String), Box<dyn std::error::Error + Send + Sync>> {
    let suffix = unique_suffix();
    let project_id = Uuid::now_v7();
    let revision_id = Uuid::now_v7();
    let environment_id = Uuid::now_v7();
    let template_id = Uuid::now_v7();
    let dispatch_id = Uuid::now_v7();
    let entries = vec![
        SnapshotEntry {
            path: "inventory.yml".to_string(),
            content: SnapshotContent::File {
                bytes: b"all:\n  hosts: {}\n".to_vec(),
                executable: false,
            },
        },
        SnapshotEntry {
            path: "site.yml".to_string(),
            content: SnapshotContent::File {
                bytes: b"---\n- hosts: all\n  tasks: []\n".to_vec(),
                executable: false,
            },
        },
    ];
    let digest = snapshot_tree_sha256(&entries)?;
    let inventory = json!({"kind": "native", "sources": ["inventory.yml"], "ssh_record_id": null});
    let options = json!({"argv": [], "extra_vars": {}, "limit": null, "env": {}, "secret_refs": {}, "timeout_seconds": 60});
    sqlx::query("INSERT INTO playbook_project (id, name, source) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("completion-project-{suffix}"))
        .bind(json!({"kind": "editable"}))
        .execute(db)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_project_revision (revision_id, project_id, tree_sha256, entries)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(revision_id)
    .bind(project_id)
    .bind(&digest)
    .bind(serde_json::to_value(&entries)?)
    .execute(db)
    .await?;
    sqlx::query("UPDATE playbook_project SET current_revision_id = $2 WHERE id = $1")
        .bind(project_id)
        .bind(revision_id)
        .execute(db)
        .await?;
    sqlx::query("INSERT INTO playbook_execution_environment (id, name, image) VALUES ($1, $2, $3)")
        .bind(environment_id)
        .bind(format!("completion-ee-{suffix}"))
        .bind("registry.example/ansible")
        .execute(db)
        .await?;
    sqlx::query(r#"
        INSERT INTO playbook_job_template
            (id, name, project_id, execution_environment_id, workdir, playbook_paths, revision_selection, inventory, options)
        VALUES ($1, $2, $3, $4, '.', ARRAY['site.yml'], $5, $6, $7)
    "#)
    .bind(template_id)
    .bind(format!("completion-template-{suffix}"))
    .bind(project_id)
    .bind(environment_id)
    .bind(json!({"kind": "fixed", "revision_id": revision_id}))
    .bind(&inventory)
    .bind(&options)
    .execute(db)
    .await?;

    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, template_id, launch_spec, trigger_kind, status, total_runs,
            dispatch_id, dispatch_attempt, lease_owner, leased_at, lease_expires_at
        )
        VALUES ($1, $2, $3, 'manual', 'running', 0, $4, 1, $5, now(), now() + interval '1 hour')
        "#,
    )
    .bind(group_id)
    .bind(template_id)
    .bind(json!({
        "template_id": template_id, "project_id": project_id, "revision_id": revision_id,
        "tree_sha256": digest, "execution_environment_id": environment_id,
        "image": "registry.example/ansible", "image_digest": format!("sha256:{}", "0".repeat(64)),
        "workdir": ".", "playbook_paths": ["site.yml"], "inventory": inventory,
        "options": options, "targets": []
    }))
    .bind(dispatch_id)
    .bind(lease_owner)
    .execute(db)
    .await?;

    // Group callback token, minted the way `PlaybookService` does: the raw
    // value is `pjt_<token_id>_<secret>` and only its SHA-256 hex is stored.
    let token_id = Uuid::now_v7();
    let secret = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
    let raw_token = format!("pjt_{token_id}_{secret}");
    let token_hash = hex::encode(Sha256::digest(raw_token.as_bytes()));
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

    Ok((
        group_id,
        dispatch_id,
        template_id,
        project_id,
        environment_id,
        raw_token,
    ))
}

async fn cleanup_seeded_group(
    db: &sqlx::PgPool,
    group_id: Uuid,
    template_id: Uuid,
    project_id: Uuid,
    environment_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM playbook_job_template WHERE id = $1")
        .bind(template_id)
        .execute(db)
        .await?;
    sqlx::query("UPDATE playbook_project SET current_revision_id = NULL WHERE id = $1")
        .bind(project_id)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM playbook_project WHERE id = $1")
        .bind(project_id)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM playbook_execution_environment WHERE id = $1")
        .bind(environment_id)
        .execute(db)
        .await?;
    Ok(())
}

/// A native zero-host group finishes with the supervisor's authoritative status
/// and seals the final durable console prefix.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis with PIPING_DATABASE__URL"]
async fn native_completion_seals_console_and_records_group_status()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;

    let lease_owner = format!("worker-test-{}", unique_suffix());
    let (group_id, dispatch_id, template_id, project_id, environment_id, token) =
        seed_running_group_with_group_callback_token(&db, &lease_owner).await?;

    let body = json!({
        "dispatch_id": dispatch_id,
        "dispatch_attempt": 1,
        "lease_owner": lease_owner,
        "console_next_sequence": 0,
        "group_status": "failed",
        "exit_code": 1,
        "error_message": "playbook failed",
        "runs": [],
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

    let (status, final_sequence): (String, Option<i64>) = sqlx::query_as(
        "SELECT status, console_final_sequence FROM playbook_run_group WHERE id = $1",
    )
    .bind(group_id)
    .fetch_one(&db)
    .await?;
    assert_eq!(status, "failed", "empty runs must not imply success");
    assert_eq!(final_sequence, Some(0));

    cleanup_seeded_group(&db, group_id, template_id, project_id, environment_id).await?;
    Ok(())
}
