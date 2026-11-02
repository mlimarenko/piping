use super::CreateRunGroupRecord;
use crate::contexts::playbooks::domain::{
    PlaybookScheduleConcurrencyPolicy, parse_schedule_concurrency_policy,
};
use crate::errors::{AppError, classify_sqlx_error_owned};
use sqlx::{Postgres, Row};
use uuid::Uuid;

pub(super) async fn apply_scheduled_run_concurrency_policy(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record: &CreateRunGroupRecord,
) -> Result<(), AppError> {
    let Some(schedule_id) = record.schedule_id else {
        return Ok(());
    };

    let Some(schedule_row) = sqlx::query(
        r#"
        SELECT playbook_id, concurrency_policy
        FROM playbook_schedule
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(schedule_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    else {
        return Err(AppError::NotFound(
            "Playbook schedule not found".to_string(),
        ));
    };

    let schedule_playbook_id: Uuid = schedule_row
        .try_get("playbook_id")
        .map_err(classify_sqlx_error_owned)?;
    if schedule_playbook_id != record.playbook_id {
        return Err(AppError::Validation(
            "Playbook schedule does not belong to this playbook".to_string(),
        ));
    }

    let concurrency_policy = parse_schedule_concurrency_policy(
        schedule_row
            .try_get::<String, _>("concurrency_policy")
            .map_err(classify_sqlx_error_owned)?
            .as_str(),
    )?;

    match concurrency_policy {
        PlaybookScheduleConcurrencyPolicy::SkipIfRunning => {
            let active_count: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)
                FROM playbook_run_group
                WHERE schedule_id = $1
                  AND status IN ('queued', 'leased', 'dispatching', 'running')
                "#,
            )
            .bind(schedule_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            if active_count > 0 {
                return Err(AppError::Conflict(
                    "Playbook schedule concurrency policy skip_if_running suppressed this fire because a prior scheduled run is still active".to_string(),
                ));
            }
        }
        PlaybookScheduleConcurrencyPolicy::QueuePending => {}
        PlaybookScheduleConcurrencyPolicy::ReplacePending => {
            sqlx::query(
                r#"
                WITH canceled_runs AS (
                    UPDATE playbook_run AS run
                    SET status = 'canceled',
                        cancel_requested_at = COALESCE(run.cancel_requested_at, NOW()),
                        cancel_outcome = 'not_dispatched',
                        finished_at = COALESCE(run.finished_at, NOW()),
                        error_message = COALESCE(
                            run.error_message,
                            'Canceled by replace_pending schedule concurrency policy'
                        )
                    FROM playbook_run_group AS run_group
                    WHERE run.run_group_id = run_group.id
                      AND run_group.schedule_id = $1
                      AND run_group.status = 'queued'
                      AND run.status = 'queued'
                    RETURNING run.run_group_id
                ), affected_groups AS (
                    SELECT DISTINCT run_group_id AS id
                    FROM canceled_runs
                )
                UPDATE playbook_run_group AS run_group
                SET canceled_runs = (
                        SELECT COUNT(*)::int
                        FROM playbook_run AS run
                        WHERE run.run_group_id = run_group.id
                          AND run.status = 'canceled'
                    ),
                    cancel_requested_at = COALESCE(run_group.cancel_requested_at, NOW()),
                    cancel_outcome = 'not_dispatched',
                    status = CASE
                        WHEN NOT EXISTS (
                            SELECT 1
                            FROM playbook_run AS run
                            WHERE run.run_group_id = run_group.id
                              AND run.status IN ('queued', 'leased', 'dispatching', 'running')
                        ) THEN 'canceled'
                        ELSE run_group.status
                    END,
                    finished_at = CASE
                        WHEN NOT EXISTS (
                            SELECT 1
                            FROM playbook_run AS run
                            WHERE run.run_group_id = run_group.id
                              AND run.status IN ('queued', 'leased', 'dispatching', 'running')
                        ) THEN COALESCE(run_group.finished_at, NOW())
                        ELSE run_group.finished_at
                    END,
                    updated_at = NOW()
                FROM affected_groups
                WHERE run_group.id = affected_groups.id
                "#,
            )
            .bind(schedule_id)
            .execute(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        }
    }

    Ok(())
}
