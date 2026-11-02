use super::cancel_tx::{revoke_terminal_callback_tokens_tx, rollup_run_groups_tx};
use super::event_rows::run_event_from_row;
use super::row_mappers::{run_from_row, run_group_from_row};
use super::selector::run_event_host;
use super::sql_strings::{RUN_COLUMNS, RUN_EVENT_COLUMNS, RUN_GROUP_COLUMNS};
use super::{
    CallbackTokenAuth, CompleteRunGroupBatchRecord, CompleteRunRecord,
    CompletedRunGroupBatchRecord, CompletedRunRecord, CreateRunEventRecord,
    CreateRunGroupEventRecord,
};
use crate::contexts::playbooks::domain::{PlaybookRunEvent, PlaybookRunEventKind};
use crate::errors::{AppError, classify_sqlx_error_owned};
use serde_json::Value;
use serde_json::json;
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;

pub(super) const RUN_GROUP_COMPLETION_EVENT_INSERT_CHUNK_SIZE: usize = 2_000;
pub(super) const CALLBACK_RATE_WINDOW_SECONDS: i64 = 60;
pub(super) const RUN_CALLBACK_EVENT_LIMIT: i64 = 50_000;
pub(super) const RUN_CALLBACK_ARTIFACT_LIMIT: i64 = 64;
pub(super) const RUN_CALLBACK_BYTE_LIMIT: i64 = 64 * 1024 * 1024;
pub(super) const RUN_CALLBACK_REQUEST_LIMIT: i32 = 120;
pub(super) const GROUP_CALLBACK_EVENT_LIMIT: i64 = 5_000_000;
pub(super) const GROUP_CALLBACK_ARTIFACT_LIMIT: i64 = 100_000;
pub(super) const GROUP_CALLBACK_BYTE_LIMIT: i64 = 8 * 1024 * 1024 * 1024;
pub(super) const GROUP_CALLBACK_REQUEST_LIMIT: i32 = 1_200;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CallbackUsage {
    pub(super) event_count: i64,
    pub(super) artifact_count: i64,
    pub(super) content_size_bytes: i64,
    pub(super) request_count: i32,
}

pub(super) async fn authorize_run_callback_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    auth: &CallbackTokenAuth,
    run_id: Uuid,
    required_scope: &str,
) -> Result<Uuid, AppError> {
    let row = sqlx::query(
        r#"
        SELECT job_token.scopes, run.run_group_id
        FROM playbook_job_token AS job_token
        JOIN playbook_run AS run
          ON run.id = job_token.run_id
         AND run.target_kind = job_token.target_kind
         AND run.target_id = job_token.target_id
         AND run.source_kind = job_token.source_kind
         AND run.source_id = job_token.source_id
        JOIN playbook_run_group AS run_group ON run_group.id = run.run_group_id
        WHERE job_token.id = $1
          AND job_token.run_id = $2
          AND job_token.token_hash = $3
          AND job_token.status = 'active'
          AND job_token.expires_at > $4
          AND run.status IN ('leased', 'dispatching', 'running')
          AND run.finished_at IS NULL
          AND run_group.status IN ('leased', 'dispatching', 'running')
          AND run_group.finished_at IS NULL
        FOR UPDATE OF job_token, run, run_group
        "#,
    )
    .bind(auth.token_id)
    .bind(run_id)
    .bind(&auth.token_hash)
    .bind(auth.now)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
    let scopes: Value = row.try_get("scopes").map_err(classify_sqlx_error_owned)?;
    if !scopes.as_array().is_some_and(|scopes| {
        scopes
            .iter()
            .any(|scope| scope.as_str() == Some(required_scope))
    }) {
        return Err(AppError::Forbidden(format!(
            "TOKEN_SCOPE_DENIED: {required_scope}"
        )));
    }
    sqlx::query("UPDATE playbook_job_token SET last_used_at = $2 WHERE id = $1")
        .bind(auth.token_id)
        .bind(auth.now)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    row.try_get("run_group_id")
        .map_err(classify_sqlx_error_owned)
}

pub(super) async fn authorize_group_callback_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    auth: &CallbackTokenAuth,
    run_group_id: Uuid,
) -> Result<(), AppError> {
    let token = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT callback_token.id
        FROM playbook_run_group_callback_token AS callback_token
        JOIN playbook_run_group AS run_group
          ON run_group.id = callback_token.run_group_id
        WHERE callback_token.id = $1
          AND callback_token.run_group_id = $2
          AND callback_token.token_hash = $3
          AND callback_token.status = 'active'
          AND callback_token.expires_at > $4
          AND run_group.status IN ('leased', 'dispatching', 'running')
          AND run_group.finished_at IS NULL
        FOR UPDATE OF callback_token, run_group
        "#,
    )
    .bind(auth.token_id)
    .bind(run_group_id)
    .bind(&auth.token_hash)
    .bind(auth.now)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
    sqlx::query("UPDATE playbook_run_group_callback_token SET last_used_at = $2 WHERE id = $1")
        .bind(token)
        .bind(auth.now)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

pub(super) async fn charge_run_callback_quota_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    run_id: Uuid,
    run_group_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    usage: CallbackUsage,
) -> Result<(), AppError> {
    let charged = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE playbook_run
        SET callback_event_count = callback_event_count + $4,
            callback_artifact_count = callback_artifact_count + $5,
            callback_bytes = callback_bytes + $6,
            callback_request_count = CASE
                WHEN $7 = 0 THEN callback_request_count
                WHEN callback_request_window_started_at <= $3 - make_interval(secs => $8)
                    THEN $7
                ELSE callback_request_count + $7
            END,
            callback_request_window_started_at = CASE
                WHEN $7 > 0 AND callback_request_window_started_at <= $3 - make_interval(secs => $8)
                    THEN $3
                ELSE callback_request_window_started_at
            END
        WHERE id = $1
          AND run_group_id = $2
          AND status IN ('leased', 'dispatching', 'running')
          AND finished_at IS NULL
          AND callback_event_count <= $9 - $4
          AND callback_artifact_count <= $10 - $5
          AND callback_bytes <= $11 - $6
          AND (
              $7 = 0
              OR callback_request_window_started_at <= $3 - make_interval(secs => $8)
              OR callback_request_count <= $12 - $7
          )
        RETURNING id
        "#,
    )
    .bind(run_id)
    .bind(run_group_id)
    .bind(now)
    .bind(usage.event_count)
    .bind(usage.artifact_count)
    .bind(usage.content_size_bytes)
    .bind(usage.request_count)
    .bind(CALLBACK_RATE_WINDOW_SECONDS)
    .bind(RUN_CALLBACK_EVENT_LIMIT)
    .bind(RUN_CALLBACK_ARTIFACT_LIMIT)
    .bind(RUN_CALLBACK_BYTE_LIMIT)
    .bind(RUN_CALLBACK_REQUEST_LIMIT)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if charged.is_none() {
        return Err(AppError::TooManyRequests(
            "CALLBACK_QUOTA_EXCEEDED: per-run callback quota exceeded".to_string(),
        ));
    }
    Ok(())
}

pub(super) async fn charge_group_callback_quota_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    run_group_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    usage: CallbackUsage,
) -> Result<(), AppError> {
    let charged = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE playbook_run_group
        SET callback_event_count = callback_event_count + $3,
            callback_artifact_count = callback_artifact_count + $4,
            callback_bytes = callback_bytes + $5,
            callback_request_count = CASE
                WHEN $6 = 0 THEN callback_request_count
                WHEN callback_request_window_started_at <= $2 - make_interval(secs => $7)
                    THEN $6
                ELSE callback_request_count + $6
            END,
            callback_request_window_started_at = CASE
                WHEN $6 > 0 AND callback_request_window_started_at <= $2 - make_interval(secs => $7)
                    THEN $2
                ELSE callback_request_window_started_at
            END,
            updated_at = $2
        WHERE id = $1
          AND status IN ('leased', 'dispatching', 'running')
          AND finished_at IS NULL
          AND callback_event_count <= $8 - $3
          AND callback_artifact_count <= $9 - $4
          AND callback_bytes <= $10 - $5
          AND (
              $6 = 0
              OR callback_request_window_started_at <= $2 - make_interval(secs => $7)
              OR callback_request_count <= $11 - $6
          )
        RETURNING id
        "#,
    )
    .bind(run_group_id)
    .bind(now)
    .bind(usage.event_count)
    .bind(usage.artifact_count)
    .bind(usage.content_size_bytes)
    .bind(usage.request_count)
    .bind(CALLBACK_RATE_WINDOW_SECONDS)
    .bind(GROUP_CALLBACK_EVENT_LIMIT)
    .bind(GROUP_CALLBACK_ARTIFACT_LIMIT)
    .bind(GROUP_CALLBACK_BYTE_LIMIT)
    .bind(GROUP_CALLBACK_REQUEST_LIMIT)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if charged.is_none() {
        return Err(AppError::TooManyRequests(
            "CALLBACK_QUOTA_EXCEEDED: run-group callback quota exceeded".to_string(),
        ));
    }
    Ok(())
}

pub(super) async fn complete_run_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record: &CompleteRunRecord,
) -> Result<CompletedRunRecord, AppError> {
    let run_sql = format!(
        r#"
        UPDATE playbook_run
        SET status = $3,
            finished_at = $4,
            exit_code = $5,
            error_message = $6,
            -- A run that carries a cancel request and then reports its own
            -- completion is the evidence the canceller was waiting for: the
            -- runner really stopped. Recording that here is what lets the
            -- deadline reconciler leave confirmed cancels alone.
            cancel_outcome = CASE
                WHEN cancel_requested_at IS NOT NULL THEN 'runner_confirmed'
                ELSE cancel_outcome
            END,
            lease_expires_at = NULL
        WHERE id = $1
          AND lease_owner = $2
          AND status IN ('leased', 'dispatching', 'running')
        RETURNING {RUN_COLUMNS}
        "#
    );
    let Some(run_row) = sqlx::query(&run_sql)
        .bind(record.run_id)
        .bind(&record.lease_owner)
        .bind(record.status.as_str())
        .bind(record.finished_at)
        .bind(record.exit_code)
        .bind(&record.error_message)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?
    else {
        let existing_sql = format!("SELECT {RUN_COLUMNS} FROM playbook_run WHERE id = $1");
        let Some(existing_row) = sqlx::query(&existing_sql)
            .bind(record.run_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound("Playbook run not found".to_string()));
        };
        let existing = run_from_row(&existing_row)?;
        if !existing.status.is_terminal() {
            return Err(AppError::Conflict(
                "Playbook run is not active for this worker or was already completed".to_string(),
            ));
        }
        let group_sql = format!("SELECT {RUN_GROUP_COLUMNS} FROM playbook_run_group WHERE id = $1");
        let Some(group_row) = sqlx::query(&group_sql)
            .bind(existing.run_group_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::Database(
                "Playbook run group not found during completion idempotency check".to_string(),
            ));
        };
        return Ok(CompletedRunRecord {
            run: existing,
            run_group: run_group_from_row(&group_row)?,
            newly_completed: false,
        });
    };
    let run = run_from_row(&run_row)?;

    append_run_event_tx(
        tx,
        CreateRunEventRecord {
            run_id: run.id,
            host: run_event_host(&run),
            event_kind: PlaybookRunEventKind::Result,
            event_type: "run_completed".to_string(),
            message: Some(
                record
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Run completed".to_string()),
            ),
            payload: serde_json::json!({
                "status": record.status.as_str(),
                "exit_code": record.exit_code,
                "lease_owner": record.lease_owner,
            }),
        },
    )
    .await?;

    let run_groups = rollup_run_groups_tx(tx, &[run.run_group_id], record.finished_at).await?;
    revoke_terminal_callback_tokens_tx(tx, &[run.run_group_id], record.finished_at).await?;
    let run_group = run_groups.into_iter().next().ok_or_else(|| {
        AppError::Database("Playbook run group not found during completion rollup".to_string())
    })?;
    Ok(CompletedRunRecord {
        run,
        run_group,
        newly_completed: true,
    })
}

pub(super) async fn complete_run_group_batch_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record: &CompleteRunGroupBatchRecord,
) -> Result<CompletedRunGroupBatchRecord, AppError> {
    if record.runs.is_empty() {
        return Err(AppError::Validation(
            "completion batch must contain at least one run".to_string(),
        ));
    }
    let run_ids = record.runs.iter().map(|run| run.run_id).collect::<Vec<_>>();
    let expected_count = i64::try_from(run_ids.len()).map_err(|_| {
        AppError::Validation("completion batch is too large to validate".to_string())
    })?;
    let valid_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM playbook_run
        WHERE run_group_id = $1
          AND id = ANY($2)
        "#,
    )
    .bind(record.run_group_id)
    .bind(&run_ids)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if valid_count != expected_count {
        return Err(AppError::Validation(
            "completion batch contains a run outside of the run group".to_string(),
        ));
    }

    let input_json = Value::Array(
        record
            .runs
            .iter()
            .map(|run| {
                serde_json::json!({
                    "run_id": run.run_id,
                    "next_status": run.status.as_str(),
                    "next_exit_code": run.exit_code,
                    "next_error_message": run.error_message,
                })
            })
            .collect(),
    );
    let run_sql = format!(
        r#"
        WITH input AS (
            SELECT run_id, next_status, next_exit_code, next_error_message
            FROM jsonb_to_recordset($3::jsonb) AS item(
                run_id uuid,
                next_status text,
                next_exit_code integer,
                next_error_message text
            )
        )
        UPDATE playbook_run AS run
        SET status = input.next_status,
            finished_at = $4,
            exit_code = input.next_exit_code,
            error_message = input.next_error_message,
            -- Same evidence rule as the single-run path: a member that was
            -- asked to cancel and then reported completion proves the runner
            -- actually stopped.
            cancel_outcome = CASE
                WHEN run.cancel_requested_at IS NOT NULL THEN 'runner_confirmed'
                ELSE run.cancel_outcome
            END,
            lease_expires_at = NULL
        FROM input
        WHERE run.id = input.run_id
          AND run.run_group_id = $1
          AND run.lease_owner = $2
          AND run.status IN ('leased', 'dispatching', 'running')
        RETURNING {RUN_COLUMNS}
        "#
    );
    let run_rows = sqlx::query(&run_sql)
        .bind(record.run_group_id)
        .bind(&record.lease_owner)
        .bind(input_json)
        .bind(record.finished_at)
        .fetch_all(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let newly_completed_runs = run_rows
        .iter()
        .map(run_from_row)
        .collect::<Result<Vec<_>, _>>()?;

    let still_active_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM playbook_run
        WHERE run_group_id = $1
          AND id = ANY($2)
          AND status IN ('leased', 'dispatching', 'running')
        "#,
    )
    .bind(record.run_group_id)
    .bind(&run_ids)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if still_active_count > 0 {
        return Err(AppError::Conflict(
            "Some playbook runs are not active for this worker or were already completed"
                .to_string(),
        ));
    }

    if !newly_completed_runs.is_empty() {
        let events = newly_completed_runs
            .iter()
            .map(|run| CreateRunGroupEventRecord {
                run_group_id: run.run_group_id,
                run_id: Some(run.id),
                host: run_event_host(run),
                event_kind: PlaybookRunEventKind::Result,
                event_type: "run_completed".to_string(),
                message: Some(
                    run.error_message
                        .clone()
                        .unwrap_or_else(|| "Run completed".to_string()),
                ),
                payload: serde_json::json!({
                    "status": run.status.as_str(),
                    "exit_code": run.exit_code,
                    "lease_owner": record.lease_owner,
                }),
            })
            .collect::<Vec<_>>();
        for chunk in events.chunks(RUN_GROUP_COMPLETION_EVENT_INSERT_CHUNK_SIZE) {
            append_run_group_events_tx(tx, chunk.to_vec()).await?;
        }
    }

    let run_group = if newly_completed_runs.is_empty() {
        let group_sql = format!("SELECT {RUN_GROUP_COLUMNS} FROM playbook_run_group WHERE id = $1");
        let Some(group_row) = sqlx::query(&group_sql)
            .bind(record.run_group_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Err(AppError::NotFound(
                "Playbook run group not found".to_string(),
            ));
        };
        run_group_from_row(&group_row)?
    } else {
        rollup_run_groups_tx(tx, &[record.run_group_id], record.finished_at)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AppError::Database(
                    "Playbook run group not found during batch completion rollup".to_string(),
                )
            })?
    };
    revoke_terminal_callback_tokens_tx(tx, &[record.run_group_id], record.finished_at).await?;
    Ok(CompletedRunGroupBatchRecord {
        run_group,
        newly_completed_runs,
    })
}

pub(super) async fn append_run_event_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record: CreateRunEventRecord,
) -> Result<PlaybookRunEvent, AppError> {
    let sql = format!(
        r#"
        WITH locked_run AS (
            SELECT id, run_group_id
            FROM playbook_run
            WHERE id = $1
            FOR UPDATE
        ), next_sequence AS (
            UPDATE playbook_run_group AS run_group
            SET event_sequence_counter = run_group.event_sequence_counter + 1,
                updated_at = NOW()
            FROM locked_run
            WHERE run_group.id = locked_run.run_group_id
            RETURNING
                run_group.id AS run_group_id,
                run_group.event_sequence_counter AS event_sequence
        )
        INSERT INTO playbook_run_event (
            run_group_id, run_id, host, event_sequence, event_kind, event_type, message, payload
        )
        SELECT
            next_sequence.run_group_id,
            locked_run.id,
            $2,
            next_sequence.event_sequence,
            $3,
            $4,
            $5,
            $6
        FROM locked_run
        JOIN next_sequence ON next_sequence.run_group_id = locked_run.run_group_id
        RETURNING {RUN_EVENT_COLUMNS}
        "#
    );
    let Some(row) = sqlx::query(&sql)
        .bind(record.run_id)
        .bind(record.host)
        .bind(record.event_kind.as_str())
        .bind(record.event_type)
        .bind(record.message)
        .bind(record.payload)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?
    else {
        return Err(AppError::NotFound("Playbook run not found".to_string()));
    };
    run_event_from_row(&row)
}

pub(super) async fn append_run_group_event_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record: CreateRunGroupEventRecord,
) -> Result<PlaybookRunEvent, AppError> {
    let sql = format!(
        r#"
        WITH selected_run AS (
            SELECT $2::uuid AS id, $1::uuid AS run_group_id
            WHERE $2::uuid IS NULL
            UNION ALL
            SELECT id, run_group_id
            FROM playbook_run
            WHERE id = $2
              AND run_group_id = $1
        ), next_sequence AS (
            UPDATE playbook_run_group AS run_group
            SET event_sequence_counter = run_group.event_sequence_counter + 1,
                updated_at = NOW()
            FROM selected_run
            WHERE run_group.id = selected_run.run_group_id
            RETURNING
                run_group.id AS run_group_id,
                run_group.event_sequence_counter AS event_sequence
        )
        INSERT INTO playbook_run_event (
            run_group_id, run_id, host, event_sequence, event_kind, event_type, message, payload
        )
        SELECT
            next_sequence.run_group_id,
            selected_run.id,
            $3,
            next_sequence.event_sequence,
            $4,
            $5,
            $6,
            $7
        FROM selected_run
        JOIN next_sequence ON next_sequence.run_group_id = selected_run.run_group_id
        RETURNING {RUN_EVENT_COLUMNS}
        "#
    );
    let Some(row) = sqlx::query(&sql)
        .bind(record.run_group_id)
        .bind(record.run_id)
        .bind(record.host)
        .bind(record.event_kind.as_str())
        .bind(record.event_type)
        .bind(record.message)
        .bind(record.payload)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?
    else {
        return Err(AppError::NotFound(
            "Playbook run group or run not found".to_string(),
        ));
    };
    run_event_from_row(&row)
}

pub(super) async fn append_run_group_events_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    records: Vec<CreateRunGroupEventRecord>,
) -> Result<Vec<PlaybookRunEvent>, AppError> {
    let Some(first_record) = records.first() else {
        return Ok(Vec::new());
    };
    let run_group_id = first_record.run_group_id;
    if records
        .iter()
        .any(|record| record.run_group_id != run_group_id)
    {
        return Err(AppError::Validation(
            "event batch must target exactly one run group".to_string(),
        ));
    }

    let count = i64::try_from(records.len())
        .map_err(|_| AppError::Validation("event batch is too large to sequence".to_string()))?;
    let sequence_row = sqlx::query(
        r#"
        UPDATE playbook_run_group
        SET event_sequence_counter = event_sequence_counter + $2,
            updated_at = NOW()
        WHERE id = $1
        RETURNING event_sequence_counter - $2 AS base_sequence
        "#,
    )
    .bind(run_group_id)
    .bind(count)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::NotFound("Playbook run group not found".to_string()))?;
    let base_sequence: i64 = sequence_row
        .try_get("base_sequence")
        .map_err(classify_sqlx_error_owned)?;

    let mut insert = QueryBuilder::<Postgres>::new(
        r#"
        INSERT INTO playbook_run_event (
            run_group_id, run_id, host, event_sequence, event_kind, event_type, message, payload
        )
        "#,
    );
    insert.push_values(
        records.into_iter().enumerate(),
        |mut row, (index, record)| {
            let event_sequence = base_sequence + i64::try_from(index).unwrap_or(0) + 1;
            row.push_bind(run_group_id)
                .push_bind(record.run_id)
                .push_bind(record.host)
                .push_bind(event_sequence)
                .push_bind(record.event_kind.as_str().to_string())
                .push_bind(record.event_type)
                .push_bind(record.message)
                .push_bind(record.payload);
        },
    );
    insert.push(" RETURNING ");
    insert.push(RUN_EVENT_COLUMNS);
    let rows = insert
        .build()
        .fetch_all(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let mut events = rows
        .iter()
        .map(run_event_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    events.sort_by_key(|event| event.event_sequence);
    mark_terminal_runs_from_group_events_tx(tx, run_group_id, &events).await?;
    Ok(events)
}

pub(super) async fn mark_terminal_runs_from_group_events_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    run_group_id: Uuid,
    events: &[PlaybookRunEvent],
) -> Result<(), AppError> {
    let failed_runs = events
        .iter()
        .filter(|event| terminal_failure_event_type(event.event_type.as_str()))
        .filter_map(|event| {
            event.run_id.map(|run_id| {
                json!({
                    "run_id": run_id,
                    "message": event.message,
                })
            })
        })
        .collect::<Vec<_>>();
    if failed_runs.is_empty() {
        return Ok(());
    }

    sqlx::query(
        r#"
        WITH input AS (
            SELECT DISTINCT ON (run_id) run_id, message
            FROM jsonb_to_recordset($2::jsonb) AS item(
                run_id uuid,
                message text
            )
            ORDER BY run_id
        )
        UPDATE playbook_run AS run
        SET status = 'failed',
            finished_at = COALESCE(run.finished_at, NOW()),
            exit_code = COALESCE(run.exit_code, 2),
            error_message = COALESCE(NULLIF(input.message, ''), run.error_message),
            lease_expires_at = NULL
        FROM input
        WHERE run.id = input.run_id
          AND run.run_group_id = $1
          AND run.status IN ('leased', 'dispatching', 'running')
        "#,
    )
    .bind(run_group_id)
    .bind(Value::Array(failed_runs))
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;

    rollup_run_groups_tx(tx, &[run_group_id], chrono::Utc::now()).await?;
    Ok(())
}

pub(super) fn terminal_failure_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "runner_on_fatal" | "runner_on_failed" | "runner_on_unreachable"
    )
}
