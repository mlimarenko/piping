//! Durable supervisor console storage, queued-group leasing and completion
//! sealing.
//!
//! This target creates a uniquely named child database from the explicitly
//! supplied `PIPING_DATABASE__URL`, applies the canonical migrations there, and
//! exercises the real `SqlxPlaybookRepository` console and lease methods. It
//! never mutates the parent/shared database.
//!
//! `cargo test -p piping-backend --test integration_run_console -- --ignored --test-threads=1`

use chrono::{DateTime, Duration, Utc};
use piping_backend::contexts::audit::application::AuditPostCommit;
use piping_backend::contexts::audit::domain::AuditEvent;
use piping_backend::contexts::audit::domain::{AuditContext, AuditCorrelation};
use piping_backend::contexts::playbooks::domain::{
    JobTokenAudience, PlaybookCancelOutcome, PlaybookRunEventKind, PlaybookRunStatus,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CallbackTokenAuth, CompleteRunBatchItemRecord, CompleteRunGroupBatchRecord, CompleteRunRecord,
    CreateJobTokenRecord, CreateRunGroupCallbackTokenRecord, CreateRunGroupEventRecord,
    FinalizeRunGroupCancelRecord, LeaseQueuedRunsRecord, PlaybookRepository,
    SqlxPlaybookRepository, StartLeasedRunsRecord, TimeoutStuckRunsRecord,
};
use piping_backend::errors::AppError;
use piping_contracts::playbook_runner::{ConsoleChunk, ConsoleStream, RunnerConsoleBatch};
use serde_json::json;
use sqlx::migrate::MigrateDatabase;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres};
use std::sync::{Arc, Mutex};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn isolated_database_url() -> TestResult<String> {
    let parent_url = std::env::var("PIPING_DATABASE__URL").map_err(|_| {
        "PIPING_DATABASE__URL must be set to a PostgreSQL server with CREATEDB for this isolated test"
    })?;
    let mut url = Url::parse(&parent_url)?;
    url.set_path(&format!("/run_console_{}", Uuid::now_v7().simple()));
    Ok(url.to_string())
}

async fn create_disposable_database(database_url: &str) -> TestResult {
    if <Postgres as MigrateDatabase>::database_exists(database_url).await? {
        <Postgres as MigrateDatabase>::force_drop_database(database_url).await?;
    }
    <Postgres as MigrateDatabase>::create_database(database_url).await?;
    Ok(())
}

async fn drop_disposable_database(database_url: &str) -> TestResult {
    if <Postgres as MigrateDatabase>::database_exists(database_url).await? {
        <Postgres as MigrateDatabase>::force_drop_database(database_url).await?;
    }
    Ok(())
}

struct Fixture {
    group_id: Uuid,
    token_id: Uuid,
    token_hash: String,
    dispatch_id: Uuid,
}

/// Project, revision, execution environment and job template shared by console
/// fixtures, plus the native launch spec with no resolved host rows.
struct TemplateChain {
    template_id: Uuid,
    launch_spec: serde_json::Value,
}

async fn seed_template_chain(pool: &PgPool, suffix: &str) -> TestResult<TemplateChain> {
    let project_id = Uuid::now_v7();
    let environment_id = Uuid::now_v7();
    let template_id = Uuid::now_v7();
    let revision_id = Uuid::now_v7();
    let options = json!({
        "argv": [],
        "extra_vars": {},
        "limit": null,
        "env": {},
        "secret_refs": {},
        "timeout_seconds": 60
    });
    let launch_spec = json!({
        "template_id": template_id,
        "project_id": project_id,
        "revision_id": revision_id,
        "tree_sha256": "0".repeat(64),
        "execution_environment_id": environment_id,
        "image": "registry.example/ansible",
        "image_digest": format!("sha256:{}", "0".repeat(64)),
        "workdir": "/work",
        "playbook_paths": ["site.yml"],
        "inventory": {"kind": "native", "sources": [], "ssh_record_id": null},
        "options": options,
        "targets": []
    });

    sqlx::query(
        r#"
        INSERT INTO playbook_project (id, name, source)
        VALUES ($1, $2, '{"kind":"editable"}'::jsonb)
        "#,
    )
    .bind(project_id)
    .bind(format!("console-project-{suffix}"))
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_execution_environment (id, name, image)
        VALUES ($1, $2, 'registry.example/ansible')
        "#,
    )
    .bind(environment_id)
    .bind(format!("console-env-{suffix}"))
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_job_template (
            id, name, project_id, workdir, playbook_paths,
            revision_selection, inventory, options, execution_environment_id
        )
        VALUES (
            $1, $2, $3, '/work', ARRAY['site.yml'],
            '{"kind":"current"}'::jsonb,
            '{"kind":"native","sources":[],"ssh_record_id":null}'::jsonb,
            $4, $5
        )
        "#,
    )
    .bind(template_id)
    .bind(format!("console-template-{suffix}"))
    .bind(project_id)
    .bind(&options)
    .bind(environment_id)
    .execute(pool)
    .await?;
    Ok(TemplateChain {
        template_id,
        launch_spec,
    })
}

async fn seed_group(pool: &PgPool, suffix: &str) -> TestResult<Fixture> {
    let chain = seed_template_chain(pool, suffix).await?;
    let group_id = Uuid::now_v7();
    let dispatch_id = Uuid::now_v7();
    let token_id = Uuid::now_v7();
    let token_hash = format!("console-hash-{suffix}");
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, template_id, launch_spec, trigger_kind, status, total_runs,
            dispatch_id, dispatch_attempt, lease_owner, lease_expires_at, leased_at
        )
        VALUES (
            $1, $2, $3, 'manual', 'running', 0,
            $4, 1, 'console-test', now() + interval '1 hour', now()
        )
        "#,
    )
    .bind(group_id)
    .bind(chain.template_id)
    .bind(&chain.launch_spec)
    .bind(dispatch_id)
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group_callback_token (
            id, run_group_id, token_hash, token_prefix, expires_at
        )
        VALUES ($1, $2, $3, 'pjt_abcdefgh', now() + interval '1 hour')
        "#,
    )
    .bind(token_id)
    .bind(group_id)
    .bind(&token_hash)
    .execute(pool)
    .await?;

    Ok(Fixture {
        group_id,
        token_id,
        token_hash,
        dispatch_id,
    })
}

async fn seed_run(pool: &PgPool, group_id: Uuid, hostname: &str) -> TestResult<Uuid> {
    let run_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run (
            id, run_group_id, hostname, target_kind, target_id, source_kind, source_id,
            target_name, status, lease_owner, lease_expires_at, leased_at,
            queued_at, attempt, timeout_seconds, metadata
        )
        VALUES (
            $1, $2, $3, 'connection_target', $4, 'device', $4,
            $3, 'running', 'console-test', now() + interval '1 hour', now(),
            now(), 1, 60, '{}'::jsonb
        )
        "#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(hostname)
    .bind(target_id)
    .execute(pool)
    .await?;
    Ok(run_id)
}

/// Queued group with a caller-chosen creation time, so lease ranking is fixed by
/// the fixture rather than by wall-clock insertion order.
async fn seed_queued_group(
    pool: &PgPool,
    chain: &TemplateChain,
    total_runs: i32,
    created_at: DateTime<Utc>,
) -> TestResult<Uuid> {
    let group_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run_group (
            id, template_id, launch_spec, trigger_kind, status, total_runs,
            created_at, updated_at
        )
        VALUES ($1, $2, $3, 'manual', 'queued', $4, $5, $5)
        "#,
    )
    .bind(group_id)
    .bind(chain.template_id)
    .bind(&chain.launch_spec)
    .bind(total_runs)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(group_id)
}

/// Queued member run with a caller-chosen queue time for lease rank ordering.
async fn seed_queued_run(
    pool: &PgPool,
    group_id: Uuid,
    hostname: &str,
    queued_at: DateTime<Utc>,
) -> TestResult<Uuid> {
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO playbook_run (
            id, run_group_id, hostname, target_kind, source_kind, target_name,
            status, queued_at, attempt, timeout_seconds, metadata
        )
        VALUES (
            $1, $2, $3, 'connection_target', 'device', $3,
            'queued', $4, 1, 60, '{}'::jsonb
        )
        "#,
    )
    .bind(run_id)
    .bind(group_id)
    .bind(hostname)
    .bind(queued_at)
    .execute(pool)
    .await?;
    Ok(run_id)
}

fn auth(fixture: &Fixture) -> CallbackTokenAuth {
    CallbackTokenAuth {
        token_id: fixture.token_id,
        token_hash: fixture.token_hash.clone(),
        now: Utc::now(),
    }
}

fn batch(dispatch_id: Uuid, chunks: &[(u64, ConsoleStream, &[u8])]) -> RunnerConsoleBatch {
    RunnerConsoleBatch {
        dispatch_id,
        dispatch_attempt: 1,
        chunks: chunks
            .iter()
            .map(|(sequence, stream, bytes)| ConsoleChunk {
                sequence: *sequence,
                stream: *stream,
                bytes: bytes.to_vec(),
            })
            .collect(),
    }
}

#[tokio::test]
#[ignore] // Requires a disposable PostgreSQL server with CREATEDB.
async fn console_storage_is_durable_and_sealed() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|error| format!("run migrations: {error}"))?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let fanout = Arc::new(RecordedEvents::default());
    repo.configure_audit_post_commit(fanout.clone())?;

    let first = seed_group(&pool, "a").await?;
    let second = seed_group(&pool, "b").await?;

    let ack = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(first.dispatch_id, &[(0, ConsoleStream::Stdout, b"a")]),
            auth(&first),
        )
        .await?;
    assert_eq!(ack.next_sequence, 1);

    // Exact retry after a lost ACK succeeds without duplicating the chunk.
    let ack = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(first.dispatch_id, &[(0, ConsoleStream::Stdout, b"a")]),
            auth(&first),
        )
        .await?;
    assert_eq!(ack.next_sequence, 1);

    // Changed bytes at an already-durable sequence conflict.
    let conflict = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(first.dispatch_id, &[(0, ConsoleStream::Stdout, b"changed")]),
            auth(&first),
        )
        .await;
    assert!(matches!(conflict, Err(AppError::Conflict(_))));

    // A gap is rejected rather than silently accepted.
    let gap = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(first.dispatch_id, &[(2, ConsoleStream::Stderr, b"c")]),
            auth(&first),
        )
        .await;
    assert!(matches!(gap, Err(AppError::Conflict(_))));

    let ack = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(
                first.dispatch_id,
                &[
                    (1, ConsoleStream::Stderr, b"b"),
                    (2, ConsoleStream::Stdout, b"c"),
                ],
            ),
            auth(&first),
        )
        .await?;
    assert_eq!(ack.next_sequence, 3);

    let page = repo.list_console(first.group_id, 0, 2, None).await?;
    assert_eq!(page.dispatch_id, Some(first.dispatch_id));
    assert_eq!(page.next_sequence, 2);
    assert!(!page.complete);
    assert_eq!(page.chunks.len(), 2);
    assert_eq!(page.chunks[0].bytes, vec![b'a']);
    assert_eq!(page.chunks[1].stream, ConsoleStream::Stderr);

    let page = repo.list_console(first.group_id, 2, 64, None).await?;
    assert_eq!(page.next_sequence, 3);
    assert!(!page.complete);
    assert_eq!(page.chunks.len(), 1);
    assert_eq!(page.chunks[0].bytes, vec![b'c']);

    // Receipt mismatch is a conflict, not a best-effort append.
    let wrong_receipt = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(Uuid::now_v7(), &[(3, ConsoleStream::Stdout, b"d")]),
            auth(&first),
        )
        .await;
    assert!(matches!(wrong_receipt, Err(AppError::Conflict(_))));

    // A token is scoped to its exact group.
    let cross_group = repo
        .append_console_batch_with_callback(
            second.group_id,
            batch(second.dispatch_id, &[(0, ConsoleStream::Stdout, b"x")]),
            auth(&first),
        )
        .await;
    assert!(matches!(cross_group, Err(AppError::Auth(_))));

    // Simulate the server-authored launch context, not caller-provided request metadata.
    let launch_request_id = Uuid::now_v7();
    let launch_context = AuditContext {
        correlation: AuditCorrelation {
            request_id: Some(launch_request_id),
            ..Default::default()
        },
        ..Default::default()
    };
    sqlx::query("UPDATE playbook_run_group SET metadata = $2 WHERE id = $1")
        .bind(first.group_id)
        .bind(json!({"audit_context": launch_context, "request_id": Uuid::now_v7()}))
        .execute(&pool)
        .await?;

    // Zero-host completion seals the exact durable prefix with terminal status.
    let completed = repo
        .complete_run_group_batch_with_callback(
            auth(&first),
            CompleteRunGroupBatchRecord {
                run_group_id: first.group_id,
                lease_owner: "console-test".to_string(),
                runs: Vec::new(),
                group_status: PlaybookRunStatus::Succeeded,
                exit_code: Some(0),
                error_message: None,
                dispatch_id: Some(first.dispatch_id),
                dispatch_attempt: Some(1),
                console_next_sequence: Some(3),
                finished_at: Utc::now(),
            },
        )
        .await?;
    assert_eq!(completed.run_group.status, PlaybookRunStatus::Succeeded);
    assert!(completed.run_group.finished_at.is_some());

    // Later exact console retries still succeed after sealing.
    let ack = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(
                first.dispatch_id,
                &[
                    (0, ConsoleStream::Stdout, b"a"),
                    (1, ConsoleStream::Stderr, b"b"),
                    (2, ConsoleStream::Stdout, b"c"),
                ],
            ),
            auth(&first),
        )
        .await?;
    assert_eq!(ack.next_sequence, 3);

    // New chunks after sealing conflict.
    let sealed = repo
        .append_console_batch_with_callback(
            first.group_id,
            batch(first.dispatch_id, &[(3, ConsoleStream::Stdout, b"d")]),
            auth(&first),
        )
        .await;
    assert!(matches!(sealed, Err(AppError::Conflict(_))));

    let page = repo.list_console(first.group_id, 0, 64, None).await?;
    assert_eq!(page.next_sequence, 3);
    assert!(page.complete);
    assert_eq!(page.chunks.len(), 3);

    // Exact completion replay is idempotent and appends no second events.
    let completion = CompleteRunGroupBatchRecord {
        run_group_id: first.group_id,
        lease_owner: "console-test".to_string(),
        runs: Vec::new(),
        group_status: PlaybookRunStatus::Succeeded,
        exit_code: Some(0),
        error_message: None,
        dispatch_id: Some(first.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(3),
        finished_at: Utc::now(),
    };
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(first.group_id)
    .fetch_one(&pool)
    .await?;
    let replay = repo
        .complete_run_group_batch_with_callback(auth(&first), completion.clone())
        .await?;
    assert_eq!(replay.run_group.status, PlaybookRunStatus::Succeeded);
    assert!(replay.newly_completed_runs.is_empty());
    let replay_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(first.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(event_count, replay_event_count);
    let audit_rows: Vec<(Option<Uuid>, String, serde_json::Value)> = sqlx::query_as(
        "SELECT actor_id, actor_kind, metadata FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1",
    )
    .bind(first.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(audit_rows.len(), 1);
    let (actor_id, actor_kind, metadata) = &audit_rows[0];
    assert_eq!(*actor_id, None);
    assert_eq!(actor_kind, "system");
    assert_eq!(metadata["audit_context"]["outcome"], "succeeded");
    assert_eq!(metadata["audit_context"]["state"]["value"], "succeeded");
    assert_eq!(metadata["audit_context"]["emitter"]["kind"], "api");
    assert_eq!(
        metadata["audit_context"]["correlation"]["run_group_id"],
        first.group_id.to_string()
    );
    assert_eq!(
        metadata["audit_context"]["correlation"]["request_id"],
        launch_request_id.to_string()
    );

    let conflicting_completion = repo
        .complete_run_group_batch_with_callback(
            auth(&first),
            CompleteRunGroupBatchRecord {
                group_status: PlaybookRunStatus::Failed,
                ..completion
            },
        )
        .await;
    assert!(matches!(conflicting_completion, Err(AppError::Conflict(_))));

    // Empty run results never silently orphan managed members. Both a success
    // and a failure callback that omit the membership are refused before any
    // write; the authoritative batch must carry one item per target.
    let members = seed_group(&pool, "c").await?;
    let member_run_id = seed_run(&pool, members.group_id, "member-a").await?;
    let empty_success = repo
        .complete_run_group_batch_with_callback(
            auth(&members),
            CompleteRunGroupBatchRecord {
                run_group_id: members.group_id,
                lease_owner: "console-test".to_string(),
                runs: Vec::new(),
                group_status: PlaybookRunStatus::Succeeded,
                exit_code: Some(0),
                error_message: None,
                dispatch_id: Some(members.dispatch_id),
                dispatch_attempt: Some(1),
                console_next_sequence: Some(0),
                finished_at: Utc::now(),
            },
        )
        .await;
    assert!(matches!(empty_success, Err(AppError::Validation(_))));
    let empty_failure = CompleteRunGroupBatchRecord {
        run_group_id: members.group_id,
        lease_owner: "console-test".to_string(),
        runs: Vec::new(),
        group_status: PlaybookRunStatus::Failed,
        exit_code: Some(1),
        error_message: Some("supervisor failed".to_string()),
        dispatch_id: Some(members.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(0),
        finished_at: Utc::now(),
    };
    let refused_failure = repo
        .complete_run_group_batch_with_callback(auth(&members), empty_failure)
        .await;
    assert!(matches!(refused_failure, Err(AppError::Validation(_))));
    let member_status: String = sqlx::query_scalar("SELECT status FROM playbook_run WHERE id = $1")
        .bind(member_run_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(member_status, "running");
    let events_after_refusal: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(members.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(events_after_refusal, 0);

    let failure = CompleteRunGroupBatchRecord {
        run_group_id: members.group_id,
        lease_owner: "console-test".to_string(),
        runs: vec![CompleteRunBatchItemRecord {
            run_id: member_run_id,
            status: PlaybookRunStatus::Unreported,
            exit_code: Some(1),
            error_message: Some("supervisor failed".to_string()),
        }],
        group_status: PlaybookRunStatus::Failed,
        exit_code: Some(1),
        error_message: Some("supervisor failed".to_string()),
        dispatch_id: Some(members.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(0),
        finished_at: Utc::now(),
    };
    let failed = repo
        .complete_run_group_batch_with_callback(auth(&members), failure.clone())
        .await?;
    assert_eq!(failed.run_group.status, PlaybookRunStatus::Failed);
    assert_eq!(failed.run_group.unreported_runs, 1);
    assert_eq!(failed.newly_completed_runs.len(), 1);
    let member_status: String = sqlx::query_scalar("SELECT status FROM playbook_run WHERE id = $1")
        .bind(member_run_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(member_status, "unreported");
    let failure_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(members.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(failure_events, 1);
    let replay = repo
        .complete_run_group_batch_with_callback(auth(&members), failure.clone())
        .await?;
    assert!(replay.newly_completed_runs.is_empty());
    let replay_failure_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(members.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(failure_events, replay_failure_events);
    let audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1",
    )
    .bind(members.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["audit_context"]["outcome"], "failed");
    assert_eq!(audits[0]["audit_context"]["state"]["value"], "failed");
    assert!(!audits[0].to_string().contains("supervisor failed"));
    let member_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run.completed' AND target_id = $1",
    ).bind(member_run_id).fetch_all(&pool).await?;
    assert_eq!(member_audits.len(), 1);
    assert_eq!(
        member_audits[0]["audit_context"]["correlation"]["run_id"],
        member_run_id.to_string()
    );
    assert_eq!(
        member_audits[0]["audit_context"]["correlation"]["run_group_id"],
        members.group_id.to_string()
    );
    assert_eq!(
        member_audits[0]["audit_context"]["state"]["value"],
        "unreported"
    );
    assert_eq!(member_audits[0]["audit_context"]["outcome"], "unknown");
    assert_eq!(member_audits[0]["audit_context"]["emitter"]["kind"], "api");
    assert!(!member_audits[0].to_string().contains("supervisor failed"));
    let conflicting_failure = repo
        .complete_run_group_batch_with_callback(
            auth(&members),
            CompleteRunGroupBatchRecord {
                runs: vec![CompleteRunBatchItemRecord {
                    run_id: member_run_id,
                    status: PlaybookRunStatus::Succeeded,
                    exit_code: Some(1),
                    error_message: Some("supervisor failed".to_string()),
                }],
                ..failure
            },
        )
        .await;
    assert!(matches!(conflicting_failure, Err(AppError::Conflict(_))));

    let canceled = seed_group(&pool, "cancel-audit").await?;
    let canceled_run = seed_run(&pool, canceled.group_id, "cancel-member").await?;
    let now = Utc::now();
    repo.request_run_group_cancel(canceled.group_id, 100, now, now)
        .await?;
    let finalize = FinalizeRunGroupCancelRecord {
        run_group_id: canceled.group_id,
        dispatch_id: canceled.dispatch_id,
        outcome: PlaybookCancelOutcome::DeadlineExpired,
        now,
    };
    assert!(
        repo.finalize_run_group_cancel(finalize.clone())
            .await?
            .is_some()
    );
    assert!(repo.finalize_run_group_cancel(finalize).await?.is_none());
    let cancel_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1",
    )
    .bind(canceled.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(cancel_audits.len(), 1);
    assert_eq!(
        cancel_audits[0]["audit_context"]["outcome"],
        "not_applicable"
    );
    assert_eq!(
        cancel_audits[0]["audit_context"]["state"]["value"],
        "canceled"
    );
    assert_eq!(
        cancel_audits[0]["audit_context"]["emitter"]["kind"],
        "worker"
    );
    let canceled_run_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run.completed' AND target_id = $1",
    ).bind(canceled_run).fetch_all(&pool).await?;
    assert_eq!(canceled_run_audits.len(), 1);
    assert_eq!(
        canceled_run_audits[0]["audit_context"]["outcome"],
        "not_applicable"
    );

    for (suffix, status, state, outcome) in [
        (
            "worker-success",
            PlaybookRunStatus::Succeeded,
            "succeeded",
            "succeeded",
        ),
        (
            "worker-failure",
            PlaybookRunStatus::Failed,
            "failed",
            "failed",
        ),
        (
            "worker-timeout",
            PlaybookRunStatus::TimedOut,
            "timed_out",
            "failed",
        ),
        (
            "worker-cancel",
            PlaybookRunStatus::Canceled,
            "canceled",
            "not_applicable",
        ),
    ] {
        let fixture = seed_group(&pool, suffix).await?;
        let run_id = seed_run(&pool, fixture.group_id, suffix).await?;
        let completion = CompleteRunRecord {
            run_id,
            lease_owner: "console-test".to_string(),
            status,
            exit_code: None,
            error_message: Some("sensitive failure details".to_string()),
            finished_at: Utc::now(),
        };
        assert!(repo.complete_run(completion.clone()).await?.newly_completed);
        assert!(!repo.complete_run(completion).await?.newly_completed);
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1",
        )
        .bind(fixture.group_id)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["audit_context"]["state"]["value"], state);
        assert_eq!(rows[0]["audit_context"]["outcome"], outcome);
        assert_eq!(rows[0]["audit_context"]["emitter"]["kind"], "worker");
        assert!(!rows[0].to_string().contains("sensitive failure details"));
        let run_rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run.completed' AND target_id = $1",
        ).bind(run_id).fetch_all(&pool).await?;
        assert_eq!(run_rows.len(), 1);
        assert_eq!(run_rows[0]["audit_context"]["state"]["value"], state);
        assert_eq!(run_rows[0]["audit_context"]["emitter"]["kind"], "worker");
        assert!(
            !run_rows[0]
                .to_string()
                .contains("sensitive failure details")
        );
    }

    let queued = seed_group(&pool, "queued-cancel-audit").await?;
    let queued_run = seed_run(&pool, queued.group_id, "queued-cancel-member").await?;
    // A group that was never dispatched carries no lease at all: clearing only
    // the owner leaves chk_playbook_run_group_lease_pair violated with the
    // fixture's lease timestamps still set.
    sqlx::query("UPDATE playbook_run_group SET status = 'queued', dispatch_id = NULL, dispatch_attempt = 0, lease_owner = NULL, lease_expires_at = NULL, leased_at = NULL WHERE id = $1")
        .bind(queued.group_id).execute(&pool).await?;
    sqlx::query("UPDATE playbook_run SET status = 'queued', lease_owner = NULL, lease_expires_at = NULL, leased_at = NULL WHERE id = $1")
        .bind(queued_run)
        .execute(&pool)
        .await?;
    let now = Utc::now();
    repo.request_run_group_cancel(queued.group_id, 100, now, now)
        .await?;
    repo.request_run_group_cancel(queued.group_id, 100, now, now)
        .await?;
    let queued_audits: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, metadata FROM audit_event WHERE (event_type = 'playbook.run.completed' AND target_id = $1) OR (event_type = 'playbook.run_group.completed' AND target_id = $2)",
    ).bind(queued_run).bind(queued.group_id).fetch_all(&pool).await?;
    assert_eq!(queued_audits.len(), 2);
    for (_, metadata) in queued_audits {
        assert_eq!(metadata["audit_context"]["state"]["value"], "canceled");
        assert_eq!(metadata["audit_context"]["emitter"]["kind"], "api");
    }

    let partial = seed_group(&pool, "single-callback-audit").await?;
    let callback_run = seed_run(&pool, partial.group_id, "callback-member").await?;
    let worker_run = seed_run(&pool, partial.group_id, "worker-member").await?;
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(user_id).bind(format!("audit-{user_id}")).bind(format!("{user_id}@test.local"))
        .execute(&pool).await?;
    let request_id = Uuid::now_v7();
    let launch_context = AuditContext {
        correlation: AuditCorrelation {
            request_id: Some(request_id),
            ..Default::default()
        },
        ..Default::default()
    };
    sqlx::query(
        "UPDATE playbook_run_group SET requested_by_user_id = $2, metadata = $3 WHERE id = $1",
    )
    .bind(partial.group_id)
    .bind(user_id)
    .bind(json!({"audit_context": launch_context}))
    .execute(&pool)
    .await?;
    let token_id = Uuid::now_v7();
    let token_hash = "single-callback-audit-token".to_owned();
    repo.create_job_token(CreateJobTokenRecord {
        id: token_id,
        run_id: callback_run,
        token_hash: token_hash.clone(),
        token_prefix: "pjt_abcdefgh".to_owned(),
        scopes: json!(["status:update"]),
        expires_at: Utc::now() + Duration::hours(1),
        audience: JobTokenAudience::Supervisor,
        metadata: json!({}),
    })
    .await?;
    let callback_auth = CallbackTokenAuth {
        token_id,
        token_hash,
        now: Utc::now(),
    };
    let callback_completion = CompleteRunRecord {
        run_id: callback_run,
        lease_owner: "console-test".to_owned(),
        status: PlaybookRunStatus::Failed,
        exit_code: Some(1),
        error_message: Some("private callback error".to_owned()),
        finished_at: Utc::now(),
    };
    let sandbox_id = Uuid::now_v7();
    let sandbox_hash = "sandbox-callback-denied".to_owned();
    repo.create_job_token(CreateJobTokenRecord {
        id: sandbox_id,
        run_id: callback_run,
        token_hash: sandbox_hash.clone(),
        token_prefix: "pjt_sandbox1".to_owned(),
        scopes: json!(["status:update"]),
        expires_at: Utc::now() + Duration::hours(1),
        audience: JobTokenAudience::Sandbox,
        metadata: json!({}),
    })
    .await?;
    let sandbox_auth = CallbackTokenAuth {
        token_id: sandbox_id,
        token_hash: sandbox_hash,
        now: Utc::now(),
    };
    assert!(matches!(
        repo.complete_run_with_callback(sandbox_auth, callback_completion.clone()).await,
        Err(AppError::Auth(reason)) if reason == "TOKEN_INVALID"
    ));
    let foreign_completion = CompleteRunRecord {
        run_id: worker_run,
        ..callback_completion.clone()
    };
    assert!(matches!(
        repo.complete_run_with_callback(callback_auth.clone(), foreign_completion).await,
        Err(AppError::Auth(reason)) if reason == "TOKEN_INVALID"
    ));
    let foreign_group = seed_group(&pool, "foreign-callback").await?;
    let foreign_run = seed_run(&pool, foreign_group.group_id, "foreign-member").await?;
    let foreign_completion = CompleteRunRecord {
        run_id: foreign_run,
        ..callback_completion.clone()
    };
    assert!(matches!(
        repo.complete_run_with_callback(callback_auth.clone(), foreign_completion).await,
        Err(AppError::Auth(reason)) if reason == "TOKEN_INVALID"
    ));
    let completed = repo
        .complete_run_with_callback(callback_auth.clone(), callback_completion.clone())
        .await?;
    assert!(!completed.run_group.status.is_terminal());
    assert!(
        repo.complete_run_with_callback(callback_auth, callback_completion)
            .await
            .is_err()
    );
    let group_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1")
        .bind(partial.group_id).fetch_one(&pool).await?;
    assert_eq!(group_events, 0);
    repo.complete_run(CompleteRunRecord {
        run_id: worker_run,
        lease_owner: "console-test".to_owned(),
        status: PlaybookRunStatus::Succeeded,
        exit_code: Some(0),
        error_message: None,
        finished_at: Utc::now(),
    })
    .await?;
    for (run_id, emitter) in [(callback_run, "api"), (worker_run, "worker")] {
        let events: Vec<(Option<Uuid>, String, serde_json::Value)> = sqlx::query_as(
            "SELECT actor_id, actor_kind, metadata FROM audit_event WHERE event_type = 'playbook.run.completed' AND target_id = $1",
        ).bind(run_id).fetch_all(&pool).await?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, Some(user_id));
        assert_eq!(events[0].1, "user");
        let context = &events[0].2["audit_context"];
        assert_eq!(context["emitter"]["kind"], emitter);
        assert_eq!(context["correlation"]["run_id"], run_id.to_string());
        assert_eq!(
            context["correlation"]["run_group_id"],
            partial.group_id.to_string()
        );
        assert_eq!(context["correlation"]["request_id"], request_id.to_string());
        assert!(!events[0].2.to_string().contains("private callback error"));
    }

    let fatal = seed_group(&pool, "fatal-audit").await?;
    let fatal_run = seed_run(&pool, fatal.group_id, "fatal-member").await?;
    let fatal_event = CreateRunGroupEventRecord {
        run_group_id: fatal.group_id,
        run_id: Some(fatal_run),
        host: None,
        event_kind: PlaybookRunEventKind::Result,
        event_type: "runner_on_failed".to_owned(),
        message: Some("private fatal detail".to_owned()),
        payload: json!({}),
    };
    repo.append_run_group_event_batch_with_callback(
        auth(&fatal),
        Uuid::now_v7(),
        vec![(fatal_event, 0)],
    )
    .await?;
    let fatal_audits: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, metadata FROM audit_event WHERE (event_type = 'playbook.run.completed' AND target_id = $1) OR (event_type = 'playbook.run_group.completed' AND target_id = $2)",
    ).bind(fatal_run).bind(fatal.group_id).fetch_all(&pool).await?;
    assert_eq!(fatal_audits.len(), 2);
    for (_, metadata) in fatal_audits {
        assert_eq!(metadata["audit_context"]["outcome"], "failed");
        assert_eq!(metadata["audit_context"]["emitter"]["kind"], "api");
        assert!(!metadata.to_string().contains("private fatal detail"));
    }

    let timed_out = seed_group(&pool, "reaper-audit").await?;
    let timed_out_run = seed_run(&pool, timed_out.group_id, "reaper-member").await?;
    sqlx::query("UPDATE playbook_run SET started_at = now() - interval '5 minutes', timeout_seconds = 1 WHERE id = $1")
        .bind(timed_out_run)
        .execute(&pool)
        .await?;
    let reap = TimeoutStuckRunsRecord {
        now: Utc::now(),
        limit: 100,
    };
    assert!(
        repo.timeout_stuck_runs(reap.clone())
            .await?
            .runs
            .iter()
            .any(|run| run.id == timed_out_run)
    );
    assert!(
        repo.timeout_stuck_runs(reap)
            .await?
            .runs
            .iter()
            .all(|run| run.id != timed_out_run)
    );
    let reaper_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1",
    )
    .bind(timed_out.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(reaper_audits.len(), 1);
    let reaper_run_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run.completed' AND target_id = $1",
    ).bind(timed_out_run).fetch_all(&pool).await?;
    assert_eq!(reaper_run_audits.len(), 1);
    assert_eq!(reaper_run_audits[0]["audit_context"]["outcome"], "failed");
    assert_eq!(
        reaper_audits[0]["audit_context"]["state"]["value"],
        "timed_out"
    );
    assert_eq!(reaper_audits[0]["audit_context"]["outcome"], "failed");
    assert_eq!(
        reaper_audits[0]["audit_context"]["emitter"]["kind"],
        "worker"
    );

    // Start rechecks the durable lease/cancel state even with no host rows.
    let empty_start = seed_group(&pool, "d").await?;
    sqlx::query(
        "UPDATE playbook_run_group SET lease_expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(empty_start.group_id)
    .execute(&pool)
    .await?;
    let expired = repo
        .start_leased_runs(StartLeasedRunsRecord {
            run_group_id: empty_start.group_id,
            run_ids: Vec::new(),
            lease_owner: "console-test".to_string(),
            dispatch_id: Uuid::now_v7(),
            dispatch_attempt: 1,
            now: Utc::now(),
            job_tokens: Vec::new(),
            sandbox_job_tokens: Vec::new(),
            group_callback_token: CreateRunGroupCallbackTokenRecord {
                id: Uuid::now_v7(),
                token_hash: "start-hash".to_string(),
                token_prefix: "pjt_abcdefgh".to_string(),
                expires_at: Utc::now() + Duration::hours(1),
                metadata: json!({}),
            },
        })
        .await;
    assert!(matches!(expired, Err(AppError::Conflict(_))));

    sqlx::query(
        "UPDATE playbook_run_group SET lease_expires_at = now() + interval '1 hour', cancel_requested_at = now() WHERE id = $1",
    )
    .bind(empty_start.group_id)
    .execute(&pool)
    .await?;
    let canceled = repo
        .start_leased_runs(StartLeasedRunsRecord {
            run_group_id: empty_start.group_id,
            run_ids: Vec::new(),
            lease_owner: "console-test".to_string(),
            dispatch_id: Uuid::now_v7(),
            dispatch_attempt: 1,
            now: Utc::now(),
            job_tokens: Vec::new(),
            sandbox_job_tokens: Vec::new(),
            group_callback_token: CreateRunGroupCallbackTokenRecord {
                id: Uuid::now_v7(),
                token_hash: "start-hash".to_string(),
                token_prefix: "pjt_abcdefgh".to_string(),
                expires_at: Utc::now() + Duration::hours(1),
                metadata: json!({}),
            },
        })
        .await;
    assert!(matches!(canceled, Err(AppError::Conflict(_))));

    let mut persisted_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM audit_event WHERE event_type IN ('playbook.run.completed', 'playbook.run_group.completed')",
    ).fetch_all(&pool).await?;
    let mut fanout_ids: Vec<Uuid> = fanout
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|event| event.id)
        .collect();
    persisted_ids.sort_unstable();
    fanout_ids.sort_unstable();
    assert_eq!(fanout_ids, persisted_ids);
    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}

#[derive(Default)]
struct RecordedEvents(Mutex<Vec<AuditEvent>>);

impl AuditPostCommit for RecordedEvents {
    fn fanout_recorded_event(&self, event: AuditEvent) {
        self.0.lock().unwrap().push(event);
    }
}

struct CommitVisibleEvents {
    pool: PgPool,
    recorded: RecordedEvents,
    visible: Mutex<Vec<bool>>,
}

impl AuditPostCommit for CommitVisibleEvents {
    fn fanout_recorded_event(&self, event: AuditEvent) {
        // A pool checkout outside the mutation transaction must see the row at
        // invocation time, not only after the repository call has returned.
        let visible = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM audit_event WHERE id = $1)",
                )
                .bind(event.id)
                .fetch_one(&self.pool)
                .await
                .expect("read committed audit from post-commit sink")
            })
        });
        self.visible.lock().unwrap().push(visible);
        self.recorded.fanout_recorded_event(event);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated postgres database and CREATEDB"]
async fn terminal_audit_fanout_follows_commit_and_failed_insert_rolls_back() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let sink = Arc::new(CommitVisibleEvents {
        pool: pool.clone(),
        recorded: RecordedEvents::default(),
        visible: Mutex::new(Vec::new()),
    });
    // Configuring a clone also covers the repository handle used directly by workers.
    repo.clone().configure_audit_post_commit(sink.clone())?;
    let fixture = seed_group(&pool, "fanout-rollback").await?;
    let run_id = seed_run(&pool, fixture.group_id, "fanout-member").await?;
    let token_id = Uuid::now_v7();
    repo.create_job_token(CreateJobTokenRecord {
        id: token_id,
        run_id,
        token_hash: "fanout-rollback-job-token".to_owned(),
        token_prefix: "pjt_abcdefgh".to_owned(),
        scopes: json!(["status:update"]),
        expires_at: Utc::now() + Duration::hours(1),
        audience: JobTokenAudience::Supervisor,
        metadata: json!({}),
    })
    .await?;
    let callback_auth = CallbackTokenAuth {
        token_id,
        token_hash: "fanout-rollback-job-token".to_owned(),
        now: Utc::now(),
    };
    let completion = CompleteRunRecord {
        run_id,
        lease_owner: "console-test".to_owned(),
        status: PlaybookRunStatus::Failed,
        exit_code: Some(1),
        error_message: Some("private error".to_owned()),
        finished_at: Utc::now(),
    };
    let snapshot = |pool: PgPool| async move {
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT jsonb_build_object('run_status', r.status, 'run_requests', r.callback_request_count, 'run_window', r.callback_request_window_started_at, 'group_status', g.status, 'group_requests', g.callback_request_count, 'group_window', g.callback_request_window_started_at, 'console_final_sequence', g.console_final_sequence, 'token_status', t.status, 'token_last_used_at', t.last_used_at, 'token_revoked_at', t.revoked_at, 'job_token_status', j.status, 'job_token_last_used_at', j.last_used_at, 'job_token_revoked_at', j.revoked_at) FROM playbook_run r JOIN playbook_run_group g ON g.id = r.run_group_id JOIN playbook_run_group_callback_token t ON t.run_group_id = g.id JOIN playbook_job_token j ON j.run_id = r.id WHERE r.id = $1"
        ).bind(run_id).fetch_one(&pool).await
    };
    let before = snapshot(pool.clone()).await?;
    assert_eq!(before["run_status"], "running");
    assert_eq!(before["group_status"], "running");
    assert_eq!(before["token_status"], "active");
    assert_eq!(before["job_token_status"], "active");
    // Single-run completion inserts the group audit and revokes callback tokens
    // before inserting the run audit. Fail that later insert to exercise rollback.
    sqlx::query("CREATE FUNCTION test_reject_terminal_run_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_type = 'playbook.run.completed' THEN RAISE EXCEPTION 'injected audit failure'; END IF; RETURN NEW; END $$")
        .execute(&pool).await?;
    sqlx::query("CREATE TRIGGER test_reject_terminal_run_audit BEFORE INSERT ON audit_event FOR EACH ROW EXECUTE FUNCTION test_reject_terminal_run_audit()")
        .execute(&pool).await?;
    assert!(
        repo.complete_run_with_callback(callback_auth.clone(), completion.clone())
            .await
            .is_err()
    );
    assert_eq!(snapshot(pool.clone()).await?, before);
    assert!(sink.recorded.0.lock().unwrap().is_empty());
    assert!(sink.visible.lock().unwrap().is_empty());
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event WHERE target_id = $1 OR target_id = $2",
    )
    .bind(run_id)
    .bind(fixture.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(count, 0);
    sqlx::query("DROP TRIGGER test_reject_terminal_run_audit ON audit_event")
        .execute(&pool)
        .await?;
    sqlx::query("DROP FUNCTION test_reject_terminal_run_audit()")
        .execute(&pool)
        .await?;
    repo.complete_run_with_callback(callback_auth.clone(), completion.clone())
        .await?;
    let after = snapshot(pool.clone()).await?;
    assert_eq!(after["run_status"], "failed");
    assert_eq!(after["group_status"], "failed");
    assert_eq!(after["run_requests"], 1);
    assert_eq!(after["group_requests"], 1);
    assert_eq!(after["token_status"], "revoked");
    assert!(!after["token_revoked_at"].is_null());
    assert_eq!(after["job_token_status"], "revoked");
    assert!(!after["job_token_revoked_at"].is_null());
    let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM audit_event WHERE (event_type = 'playbook.run.completed' AND target_id = $1) OR (event_type = 'playbook.run_group.completed' AND target_id = $2)")
        .bind(run_id).bind(fixture.group_id).fetch_all(&pool).await?;
    let mut dispatched: Vec<Uuid> = sink
        .recorded
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|event| event.id)
        .collect();
    ids.sort_unstable();
    dispatched.sort_unstable();
    assert_eq!(ids.len(), 2);
    assert_eq!(dispatched, ids);
    assert_eq!(*sink.visible.lock().unwrap(), vec![true, true]);
    assert!(
        repo.complete_run_with_callback(callback_auth, completion)
            .await
            .is_err()
    );
    assert_eq!(sink.recorded.0.lock().unwrap().len(), 2);
    assert_eq!(*sink.visible.lock().unwrap(), vec![true, true]);
    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}

/// A terminal run without a valid structured host result is persisted as
/// `unreported`: it counts separately, never as success or failure, while the
/// completion batch's process outcome stays the authoritative group status.
#[tokio::test]
#[ignore = "requires an isolated postgres database and CREATEDB"]
async fn unreported_terminal_runs_keep_the_authoritative_group_outcome() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());

    // A succeeded process may finish with a host that never reported a
    // structured result: the group keeps its success, the host does not.
    let succeeded = seed_group(&pool, "unreported-in-succeeded").await?;
    let unreported_member = seed_run(&pool, succeeded.group_id, "no-result-host").await?;
    let first_batch = CompleteRunGroupBatchRecord {
        run_group_id: succeeded.group_id,
        lease_owner: "console-test".to_string(),
        runs: vec![CompleteRunBatchItemRecord {
            run_id: unreported_member,
            status: PlaybookRunStatus::Unreported,
            exit_code: None,
            error_message: Some("no structured host result".to_string()),
        }],
        group_status: PlaybookRunStatus::Succeeded,
        exit_code: Some(0),
        error_message: None,
        dispatch_id: Some(succeeded.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(0),
        finished_at: Utc::now(),
    };
    let completed = repo
        .complete_run_group_batch_with_callback(auth(&succeeded), first_batch.clone())
        .await?;
    assert_eq!(completed.run_group.status, PlaybookRunStatus::Succeeded);
    assert_eq!(completed.run_group.total_runs, 1);
    assert_eq!(completed.run_group.succeeded_runs, 0);
    assert_eq!(completed.run_group.failed_runs, 0);
    assert_eq!(completed.run_group.unreported_runs, 1);
    assert_eq!(completed.newly_completed_runs.len(), 1);
    assert_eq!(
        completed.newly_completed_runs[0].status,
        PlaybookRunStatus::Unreported
    );
    let member_status: String = sqlx::query_scalar("SELECT status FROM playbook_run WHERE id = $1")
        .bind(unreported_member)
        .fetch_one(&pool)
        .await?;
    assert_eq!(member_status, "unreported");
    let events: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_kind, event_type, payload FROM playbook_run_event WHERE run_group_id = $1 AND run_id = $2",
    )
    .bind(succeeded.group_id)
    .bind(unreported_member)
    .fetch_all(&pool)
    .await?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "result");
    assert_eq!(events[0].1, "run_completed");
    assert_eq!(events[0].2["status"], "unreported");
    let run_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run.completed' AND target_id = $1",
    )
    .bind(unreported_member)
    .fetch_all(&pool)
    .await?;
    assert_eq!(run_audits.len(), 1);
    assert_eq!(
        run_audits[0]["audit_context"]["state"]["domain"],
        "automation"
    );
    assert_eq!(
        run_audits[0]["audit_context"]["state"]["value"],
        "unreported"
    );
    assert_eq!(run_audits[0]["audit_context"]["outcome"], "unknown");
    let group_audits: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type = 'playbook.run_group.completed' AND target_id = $1",
    )
    .bind(succeeded.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(group_audits.len(), 1);
    assert_eq!(
        group_audits[0]["audit_context"]["state"]["value"],
        "succeeded"
    );
    assert_eq!(group_audits[0]["audit_context"]["outcome"], "succeeded");

    // Replaying the exact batch appends no second result event or audit.
    let replay = repo
        .complete_run_group_batch_with_callback(auth(&succeeded), first_batch)
        .await?;
    assert!(replay.newly_completed_runs.is_empty());
    assert_eq!(replay.run_group.status, PlaybookRunStatus::Succeeded);
    assert_eq!(replay.run_group.unreported_runs, 1);
    let replay_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(succeeded.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(replay_events, 1);
    let replay_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM audit_event WHERE (event_type = 'playbook.run.completed' AND target_id = $1) OR (event_type = 'playbook.run_group.completed' AND target_id = $2)",
    )
    .bind(unreported_member)
    .bind(succeeded.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(replay_audits, 2);

    // A failed process whose hosts all went unreported stays failed: no member
    // becomes succeeded just because no host reported a failure.
    let failed = seed_group(&pool, "unreported-in-failed").await?;
    let failed_members = vec![
        seed_run(&pool, failed.group_id, "no-result-a").await?,
        seed_run(&pool, failed.group_id, "no-result-b").await?,
    ];
    let failed_batch = CompleteRunGroupBatchRecord {
        run_group_id: failed.group_id,
        lease_owner: "console-test".to_string(),
        runs: failed_members
            .iter()
            .map(|run_id| CompleteRunBatchItemRecord {
                run_id: *run_id,
                status: PlaybookRunStatus::Unreported,
                exit_code: None,
                error_message: None,
            })
            .collect(),
        group_status: PlaybookRunStatus::Failed,
        exit_code: Some(1),
        error_message: Some("supervisor failed before host results".to_string()),
        dispatch_id: Some(failed.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(0),
        finished_at: Utc::now(),
    };
    let completed = repo
        .complete_run_group_batch_with_callback(auth(&failed), failed_batch)
        .await?;
    assert_eq!(completed.run_group.status, PlaybookRunStatus::Failed);
    assert_eq!(completed.run_group.unreported_runs, 2);
    assert_eq!(completed.run_group.succeeded_runs, 0);
    assert_eq!(completed.run_group.failed_runs, 0);
    assert_eq!(completed.newly_completed_runs.len(), 2);
    let member_statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM playbook_run WHERE run_group_id = $1 ORDER BY hostname",
    )
    .bind(failed.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(member_statuses, vec!["unreported", "unreported"]);

    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated postgres database and CREATEDB"]
async fn internal_unobserved_failure_replay_is_idempotent() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());

    let fixture = seed_group(&pool, "internal-unobserved").await?;
    let member = seed_run(&pool, fixture.group_id, "member-a").await?;
    let record = CompleteRunGroupBatchRecord {
        run_group_id: fixture.group_id,
        lease_owner: "console-test".to_string(),
        runs: Vec::new(),
        group_status: PlaybookRunStatus::Failed,
        exit_code: None,
        error_message: Some("pre-dispatch failure".to_string()),
        dispatch_id: None,
        dispatch_attempt: None,
        console_next_sequence: None,
        finished_at: Utc::now(),
    };

    // A worker success with no member results must be refused, not projected
    // onto the members.
    let unsupported_success = CompleteRunGroupBatchRecord {
        group_status: PlaybookRunStatus::Succeeded,
        exit_code: Some(0),
        ..record.clone()
    };
    assert!(matches!(
        repo.complete_run_group_batch(unsupported_success).await,
        Err(AppError::Validation(_))
    ));
    let status: String = sqlx::query_scalar("SELECT status FROM playbook_run WHERE id = $1")
        .bind(member)
        .fetch_one(&pool)
        .await?;
    assert_eq!(status, "running");

    let completed = repo.complete_run_group_batch(record.clone()).await?;
    assert_eq!(completed.run_group.status, PlaybookRunStatus::Failed);
    assert_eq!(completed.run_group.unreported_runs, 1);
    assert_eq!(completed.run_group.failed_runs, 0);
    assert_eq!(completed.newly_completed_runs.len(), 1);
    assert_eq!(
        completed.newly_completed_runs[0].status,
        PlaybookRunStatus::Unreported
    );
    let status: String = sqlx::query_scalar("SELECT status FROM playbook_run WHERE id = $1")
        .bind(member)
        .fetch_one(&pool)
        .await?;
    assert_eq!(status, "unreported");
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(fixture.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(events, 1);

    // Exact replay of the internal empty-run failure is idempotent.
    let replay = repo.complete_run_group_batch(record.clone()).await?;
    assert!(replay.newly_completed_runs.is_empty());
    assert_eq!(replay.run_group.status, PlaybookRunStatus::Failed);
    let replay_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
    )
    .bind(fixture.group_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(replay_events, events);

    // An altered terminal outcome is still a conflict.
    let altered = CompleteRunGroupBatchRecord {
        exit_code: Some(2),
        ..record.clone()
    };
    assert!(matches!(
        repo.complete_run_group_batch(altered).await,
        Err(AppError::Conflict(_))
    ));
    let altered_message = CompleteRunGroupBatchRecord {
        error_message: Some("different failure".to_string()),
        ..record
    };
    assert!(matches!(
        repo.complete_run_group_batch(altered_message).await,
        Err(AppError::Conflict(_))
    ));

    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated postgres database and CREATEDB"]
async fn incomplete_managed_completion_is_rejected_without_partial_writes() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());

    let fixture = seed_group(&pool, "incomplete-managed").await?;
    let first = seed_run(&pool, fixture.group_id, "member-first").await?;
    let second = seed_run(&pool, fixture.group_id, "member-second").await?;
    let foreign = Uuid::now_v7();
    let finished_at = Utc::now();
    let batch = |runs: Vec<(Uuid, PlaybookRunStatus)>| CompleteRunGroupBatchRecord {
        run_group_id: fixture.group_id,
        lease_owner: "console-test".to_string(),
        runs: runs
            .into_iter()
            .map(|(run_id, status)| CompleteRunBatchItemRecord {
                run_id,
                status,
                exit_code: None,
                error_message: Some("incomplete membership".to_string()),
            })
            .collect(),
        group_status: PlaybookRunStatus::Failed,
        exit_code: Some(1),
        error_message: Some("incomplete membership".to_string()),
        dispatch_id: Some(fixture.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(0),
        finished_at,
    };

    // Missing, duplicated and foreign member IDs must be refused before any
    // write; a managed callback may never derive host outcomes from the group.
    for (label, runs) in [
        (
            "missing member",
            vec![(first, PlaybookRunStatus::Unreported)],
        ),
        (
            "duplicate member",
            vec![
                (first, PlaybookRunStatus::Unreported),
                (first, PlaybookRunStatus::Unreported),
            ],
        ),
        (
            "foreign member",
            vec![
                (first, PlaybookRunStatus::Unreported),
                (foreign, PlaybookRunStatus::Unreported),
            ],
        ),
    ] {
        assert!(
            repo.complete_run_group_batch_with_callback(auth(&fixture), batch(runs))
                .await
                .is_err(),
            "{label} must be refused"
        );
        let group: (
            String,
            Option<DateTime<Utc>>,
            i32,
            Option<String>,
            Option<i64>,
        ) = sqlx::query_as(
            "SELECT status, finished_at, total_runs, lease_owner, console_final_sequence \
                 FROM playbook_run_group WHERE id = $1",
        )
        .bind(fixture.group_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(group.0, "running", "{label}");
        assert!(group.1.is_none(), "{label}");
        assert_eq!(group.2, 0, "{label}");
        assert_eq!(group.3.as_deref(), Some("console-test"), "{label}");
        assert!(group.4.is_none(), "{label}: console prefix was sealed");
        let members: Vec<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
            "SELECT status, finished_at, error_message FROM playbook_run \
             WHERE run_group_id = $1 ORDER BY hostname",
        )
        .bind(fixture.group_id)
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            members,
            vec![
                ("running".to_string(), None, None),
                ("running".to_string(), None, None),
            ],
            "{label}"
        );
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM playbook_run_event WHERE run_group_id = $1",
        )
        .bind(fixture.group_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(events, 0, "{label}: events were appended");
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM audit_event \
             WHERE event_type IN ('playbook.run.completed', 'playbook.run_group.completed') \
               AND (target_id = $1 OR target_id = ANY($2))",
        )
        .bind(fixture.group_id)
        .bind(vec![first, second])
        .fetch_one(&pool)
        .await?;
        assert_eq!(audits, 0, "{label}: terminal audit was written");
        let token_status: String = sqlx::query_scalar(
            "SELECT status FROM playbook_run_group_callback_token WHERE id = $1",
        )
        .bind(fixture.token_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            token_status, "active",
            "{label}: callback token was revoked"
        );
    }

    // The exact membership is accepted and records the unobserved hosts
    // truthfully as unreported while the failed process stays failed.
    let completed = repo
        .complete_run_group_batch_with_callback(
            auth(&fixture),
            batch(vec![
                (first, PlaybookRunStatus::Unreported),
                (second, PlaybookRunStatus::Unreported),
            ]),
        )
        .await?;
    assert_eq!(completed.run_group.status, PlaybookRunStatus::Failed);
    assert_eq!(completed.run_group.total_runs, 2);
    assert_eq!(completed.run_group.unreported_runs, 2);
    assert_eq!(completed.run_group.failed_runs, 0);
    assert_eq!(completed.newly_completed_runs.len(), 2);

    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated postgres database and CREATEDB"]
async fn mixed_reported_and_unreported_batch_keeps_group_succeeded() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());

    let fixture = seed_group(&pool, "mixed-unreported").await?;
    let reported = seed_run(&pool, fixture.group_id, "member-reported").await?;
    let unreported = seed_run(&pool, fixture.group_id, "member-unreported").await?;
    let batch = CompleteRunGroupBatchRecord {
        run_group_id: fixture.group_id,
        lease_owner: "console-test".to_string(),
        runs: vec![
            CompleteRunBatchItemRecord {
                run_id: reported,
                status: PlaybookRunStatus::Succeeded,
                exit_code: Some(0),
                error_message: None,
            },
            CompleteRunBatchItemRecord {
                run_id: unreported,
                status: PlaybookRunStatus::Unreported,
                exit_code: None,
                error_message: Some("host outcome unreported".to_string()),
            },
        ],
        group_status: PlaybookRunStatus::Succeeded,
        exit_code: Some(0),
        error_message: None,
        dispatch_id: Some(fixture.dispatch_id),
        dispatch_attempt: Some(1),
        console_next_sequence: Some(0),
        finished_at: Utc::now(),
    };
    let completed = repo
        .complete_run_group_batch_with_callback(auth(&fixture), batch.clone())
        .await?;
    assert_eq!(completed.run_group.status, PlaybookRunStatus::Succeeded);
    assert_eq!(completed.run_group.total_runs, 2);
    assert_eq!(completed.run_group.succeeded_runs, 1);
    assert_eq!(completed.run_group.unreported_runs, 1);
    assert_eq!(completed.run_group.failed_runs, 0);
    let statuses: Vec<(String, String)> = sqlx::query_as(
        "SELECT hostname, status FROM playbook_run WHERE run_group_id = $1 ORDER BY hostname",
    )
    .bind(fixture.group_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        statuses,
        vec![
            ("member-reported".to_string(), "succeeded".to_string()),
            ("member-unreported".to_string(), "unreported".to_string()),
        ]
    );
    let replay = repo
        .complete_run_group_batch_with_callback(auth(&fixture), batch)
        .await?;
    assert!(replay.newly_completed_runs.is_empty());
    assert_eq!(replay.run_group.status, PlaybookRunStatus::Succeeded);
    assert_eq!(replay.run_group.unreported_runs, 1);

    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated postgres database and CREATEDB"]
async fn queued_group_lease_hands_out_native_zero_run_and_member_runs() -> TestResult {
    let database_url = isolated_database_url()?;
    create_disposable_database(&database_url).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let repo = SqlxPlaybookRepository::new(pool.clone());

    let now = Utc::now();
    // A native launch resolves no host rows, so its group owns zero
    // `playbook_run` rows: the member-run lease statement must still execute.
    let zero_run_chain = seed_template_chain(&pool, "lease-native").await?;
    let zero_run_group =
        seed_queued_group(&pool, &zero_run_chain, 0, now - Duration::seconds(120)).await?;
    let member_chain = seed_template_chain(&pool, "lease-members").await?;
    let member_group =
        seed_queued_group(&pool, &member_chain, 2, now - Duration::seconds(60)).await?;
    let first_member = seed_queued_run(
        &pool,
        member_group,
        "lease-member-first",
        now - Duration::seconds(30),
    )
    .await?;
    let second_member = seed_queued_run(
        &pool,
        member_group,
        "lease-member-second",
        now - Duration::seconds(20),
    )
    .await?;

    let lease_owner = "lease-test-owner";
    let first_lease = repo
        .lease_queued_groups(LeaseQueuedRunsRecord {
            lease_owner: lease_owner.to_owned(),
            limit: 10,
            max_concurrent_groups: 10,
            max_concurrent_runs_per_group: 1,
            now,
            lease_expires_at: now + Duration::seconds(60),
        })
        .await?;
    let leased_group_ids = first_lease.iter().map(|group| group.id).collect::<Vec<_>>();
    assert_eq!(
        leased_group_ids,
        vec![zero_run_group, member_group],
        "lease keeps the created_at, id rank order"
    );
    let zero_run = first_lease
        .iter()
        .find(|group| group.id == zero_run_group)
        .expect("native zero-run group is leased");
    assert_eq!(zero_run.status, PlaybookRunStatus::Leased);
    assert_eq!(zero_run.lease_owner.as_deref(), Some(lease_owner));
    assert_eq!(zero_run.total_runs, 0);

    let zero_run_persisted = repo
        .get_run_group_with_runs(zero_run_group, 10)
        .await?
        .ok_or("native zero-run group missing after lease")?;
    assert_eq!(
        zero_run_persisted.run_group.status,
        PlaybookRunStatus::Leased
    );
    assert_eq!(
        zero_run_persisted.run_group.lease_owner.as_deref(),
        Some(lease_owner)
    );
    assert!(zero_run_persisted.run_group.leased_at.is_some());
    assert!(zero_run_persisted.run_group.started_at.is_some());
    assert!(zero_run_persisted.runs.is_empty());

    // Per-group cap: only the earliest queued member is leased.
    let members = repo
        .get_run_group_with_runs(member_group, 10)
        .await?
        .ok_or("member group missing after lease")?;
    let leased_members = members
        .runs
        .iter()
        .filter(|run| run.status == PlaybookRunStatus::Leased)
        .map(|run| (run.id, run.lease_owner.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        leased_members,
        vec![(first_member, Some(lease_owner.to_owned()))]
    );
    let queued_member = members
        .runs
        .iter()
        .find(|run| run.id == second_member)
        .expect("second member is present");
    assert_eq!(queued_member.status, PlaybookRunStatus::Queued);
    assert_eq!(queued_member.lease_owner, None);

    // A wider per-group cap hands out the remaining member, ranked after the
    // unexpired lease that now counts as active work.
    let second_lease = repo
        .lease_queued_groups(LeaseQueuedRunsRecord {
            lease_owner: lease_owner.to_owned(),
            limit: 10,
            max_concurrent_groups: 10,
            max_concurrent_runs_per_group: 2,
            now: now + Duration::seconds(5),
            lease_expires_at: now + Duration::seconds(120),
        })
        .await?;
    assert!(second_lease.iter().any(|group| group.id == member_group));
    let members = repo
        .get_run_group_with_runs(member_group, 10)
        .await?
        .ok_or("member group missing after second lease")?;
    assert_eq!(members.runs.len(), 2);
    for run in &members.runs {
        assert_eq!(run.status, PlaybookRunStatus::Leased);
        assert_eq!(run.lease_owner.as_deref(), Some(lease_owner));
        assert!(run.leased_at.is_some());
    }

    pool.close().await;
    drop_disposable_database(&database_url).await?;
    Ok(())
}
