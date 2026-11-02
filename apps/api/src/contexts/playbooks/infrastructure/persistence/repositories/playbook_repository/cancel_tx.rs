use super::callback_tx::append_run_group_events_tx;
use super::row_mappers::{run_from_row, run_group_from_row};
use super::selector::run_event_host;
use super::sql_strings::{RUN_COLUMNS, RUN_GROUP_COLUMNS};
use super::{CreateRunGroupEventRecord, RunGroupWithRuns};
use crate::contexts::playbooks::domain::{
    PlaybookCancelOutcome, PlaybookRun, PlaybookRunEventKind, PlaybookRunGroup, PlaybookRunStatus,
};
use crate::errors::{AppError, classify_sqlx_error_owned};
use crate::observability::metrics;
use sqlx::Postgres;
use uuid::Uuid;

pub(super) async fn request_run_group_cancel_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    run_group_id: Uuid,
    limit: i64,
    now: chrono::DateTime<chrono::Utc>,
    deadline_at: chrono::DateTime<chrono::Utc>,
) -> Result<Option<(RunGroupWithRuns, Vec<PlaybookRun>)>, AppError> {
    let group_sql =
        format!("SELECT {RUN_GROUP_COLUMNS} FROM playbook_run_group WHERE id = $1 FOR UPDATE");
    let Some(group_row) = sqlx::query(&group_sql)
        .bind(run_group_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?
    else {
        return Ok(None);
    };
    let mut run_group = run_group_from_row(&group_row)?;
    let mut newly_canceled_runs = Vec::new();

    if !run_group.status.is_terminal() && run_group.cancel_requested_at.is_none() {
        if run_group.dispatch_id.is_none()
            && matches!(
                run_group.status,
                PlaybookRunStatus::Queued | PlaybookRunStatus::Leased
            )
        {
            sqlx::query(
                r#"
                UPDATE playbook_run_group
                SET cancel_requested_at = $2,
                    cancel_outcome = 'not_dispatched',
                    updated_at = $2
                WHERE id = $1
                "#,
            )
            .bind(run_group_id)
            .bind(now)
            .execute(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            let update_sql = format!(
                r#"
                UPDATE playbook_run
                SET status = 'canceled',
                    cancel_requested_at = $2,
                    cancel_outcome = 'not_dispatched',
                    finished_at = $2,
                    lease_owner = NULL,
                    lease_expires_at = NULL
                WHERE run_group_id = $1
                  AND status IN ('queued', 'leased')
                RETURNING {RUN_COLUMNS}
                "#
            );
            let rows = sqlx::query(&update_sql)
                .bind(run_group_id)
                .bind(now)
                .fetch_all(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            newly_canceled_runs = rows
                .iter()
                .map(run_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            append_cancel_finalized_events_tx(
                tx,
                &newly_canceled_runs,
                PlaybookCancelOutcome::NotDispatched,
                "Canceled before dispatch",
            )
            .await?;
            run_group = rollup_run_groups_tx(tx, &[run_group_id], now)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| AppError::Database("Playbook run group not found".to_string()))?;
            // A member that cancels immediately is terminal right now, so its job
            // token has to die with it — otherwise a runner that is still holding
            // that token can keep posting callbacks against a canceled run. Same
            // rule the completion paths follow; the guard inside makes it a no-op
            // while the group itself is still running.
            revoke_terminal_callback_tokens_tx(tx, &[run_group_id], now).await?;
        } else {
            if run_group.dispatch_id.is_none() {
                return Err(AppError::Database(
                    "Active dispatched playbook group has no dispatch_id".to_string(),
                ));
            }
            sqlx::query(
                r#"
                UPDATE playbook_run_group
                SET cancel_requested_at = $2,
                    cancel_deadline_at = $3,
                    updated_at = $2
                WHERE id = $1
                "#,
            )
            .bind(run_group_id)
            .bind(now)
            .bind(deadline_at)
            .execute(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            sqlx::query(
                r#"
                UPDATE playbook_run
                SET cancel_requested_at = $2
                WHERE run_group_id = $1
                  AND status IN ('leased', 'dispatching', 'running')
                  AND cancel_requested_at IS NULL
                "#,
            )
            .bind(run_group_id)
            .bind(now)
            .execute(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            let refreshed = sqlx::query(&group_sql)
                .bind(run_group_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            run_group = run_group_from_row(&refreshed)?;
        }
    }

    let runs_sql = format!(
        r#"
        SELECT {RUN_COLUMNS}
        FROM playbook_run
        WHERE run_group_id = $1
        ORDER BY queued_at ASC, id ASC
        LIMIT $2
        "#
    );
    let rows = sqlx::query(&runs_sql)
        .bind(run_group_id)
        .bind(limit)
        .fetch_all(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let runs = rows
        .iter()
        .map(run_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some((
        RunGroupWithRuns { run_group, runs },
        newly_canceled_runs,
    )))
}

pub(super) async fn append_cancel_finalized_events_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    runs: &[PlaybookRun],
    outcome: PlaybookCancelOutcome,
    message: &str,
) -> Result<(), AppError> {
    let records = runs
        .iter()
        .map(|run| CreateRunGroupEventRecord {
            run_group_id: run.run_group_id,
            run_id: Some(run.id),
            host: run_event_host(run),
            event_kind: PlaybookRunEventKind::Result,
            event_type: "run_cancel_finalized".to_string(),
            message: Some(message.to_string()),
            payload: serde_json::json!({
                "status": PlaybookRunStatus::Canceled.as_str(),
                "cancel_outcome": outcome.as_str(),
            }),
        })
        .collect::<Vec<_>>();
    append_run_group_events_tx(tx, records).await?;
    Ok(())
}

pub(super) fn record_canceled_run_metrics(runs: &[PlaybookRun]) {
    for run in runs {
        metrics::record_playbook_run_outcome(
            run.status.as_str(),
            run.queued_at,
            run.started_at,
            run.finished_at,
        );
    }
}

/// Recompute run-group counters/status/lease for the given groups from their
/// current run rows. Shared by `complete_run` (one group) and
/// `timeout_stuck_runs` (many groups) so the group rollup CASE stays
/// byte-identical between the two paths. `finished_at` is the timestamp used
/// when a group transitions terminal.
pub(super) async fn rollup_run_groups_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    group_ids: &[Uuid],
    finished_at: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<PlaybookRunGroup>, AppError> {
    if group_ids.is_empty() {
        return Ok(Vec::new());
    }
    let group_sql = r#"
        WITH stats AS (
            SELECT
                run_group_id,
                COUNT(*)::int AS total_runs,
                COUNT(*) FILTER (WHERE status = 'succeeded')::int AS succeeded_runs,
                COUNT(*) FILTER (WHERE status = 'failed')::int AS failed_runs,
                COUNT(*) FILTER (WHERE status = 'canceled')::int AS canceled_runs,
                COUNT(*) FILTER (WHERE status = 'timed_out')::int AS timeout_runs,
                COUNT(*) FILTER (
                    WHERE status IN ('queued', 'leased', 'dispatching', 'running')
                )::int AS active_runs,
                COUNT(*) FILTER (
                    WHERE cancel_requested_at IS NOT NULL AND cancel_outcome IS NULL
                )::int AS unresolved_cancels
            FROM playbook_run
            WHERE run_group_id = ANY($1)
            GROUP BY run_group_id
        )
        UPDATE playbook_run_group AS group_row
        SET total_runs = stats.total_runs,
            succeeded_runs = stats.succeeded_runs,
            failed_runs = stats.failed_runs,
            canceled_runs = stats.canceled_runs,
            timeout_runs = stats.timeout_runs,
            cancel_outcome = CASE
                WHEN group_row.cancel_requested_at IS NOT NULL
                    AND group_row.cancel_outcome IS NULL
                    AND stats.active_runs = 0
                    AND stats.unresolved_cancels = 0
                THEN 'runner_confirmed'
                ELSE group_row.cancel_outcome
            END,
            status = CASE
                WHEN stats.active_runs > 0 THEN 'running'
                WHEN stats.total_runs = stats.succeeded_runs THEN 'succeeded'
                WHEN stats.failed_runs > 0 THEN 'failed'
                WHEN stats.timeout_runs > 0 THEN 'timed_out'
                WHEN stats.canceled_runs > 0 THEN 'canceled'
                ELSE 'failed'
            END,
            finished_at = CASE WHEN stats.active_runs = 0 THEN $2 ELSE NULL END,
            lease_owner = CASE WHEN stats.active_runs = 0 THEN NULL ELSE group_row.lease_owner END,
            lease_expires_at = CASE WHEN stats.active_runs = 0 THEN NULL ELSE group_row.lease_expires_at END,
            leased_at = CASE WHEN stats.active_runs = 0 THEN NULL ELSE group_row.leased_at END,
            updated_at = $2
        FROM stats
        WHERE group_row.id = stats.run_group_id
        RETURNING group_row.*
        "#;
    let rows = sqlx::query(group_sql)
        .bind(group_ids)
        .bind(finished_at)
        .fetch_all(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    rows.iter().map(run_group_from_row).collect()
}

/// Revoke callback authority for every terminal run and run group in the same
/// transaction that made them terminal. Active siblings keep their run tokens,
/// and an active group keeps its dedicated group token until the final member
/// reaches a terminal state.
pub(super) async fn revoke_terminal_callback_tokens_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    group_ids: &[Uuid],
    revoked_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), AppError> {
    if group_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE playbook_job_token AS job_token
        SET status = 'revoked',
            revoked_at = COALESCE(job_token.revoked_at, $2)
        FROM playbook_run AS run
        WHERE job_token.run_id = run.id
          AND run.run_group_id = ANY($1)
          AND run.status IN ('succeeded', 'failed', 'canceled', 'timed_out')
          AND job_token.status = 'active'
        "#,
    )
    .bind(group_ids)
    .bind(revoked_at)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    sqlx::query(
        r#"
        UPDATE playbook_run_group_callback_token AS callback_token
        SET status = 'revoked',
            revoked_at = COALESCE(callback_token.revoked_at, $2)
        FROM playbook_run_group AS run_group
        WHERE callback_token.run_group_id = run_group.id
          AND run_group.id = ANY($1)
          AND run_group.status IN ('succeeded', 'failed', 'canceled', 'timed_out')
          AND callback_token.status = 'active'
        "#,
    )
    .bind(group_ids)
    .bind(revoked_at)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}
