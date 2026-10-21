use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::repository::OutboundEventRepository;
use crate::contexts::sync::domain::{OutboundEvent, OutboundEventCounters, OutboundEventStatus};
use crate::errors::AppError;

#[derive(Clone)]
pub struct SqlxOutboundEventRepository {
    pool: PgPool,
}

impl SqlxOutboundEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl OutboundEventRepository for SqlxOutboundEventRepository {
    async fn enqueue(&self, event: &OutboundEvent) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO sync_outbound_event (
                id,
                source_id,
                resource_type,
                operation,
                payload,
                status,
                attempts,
                next_attempt_at,
                processed_at,
                correlation_id,
                last_error,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(event.id)
        .bind(event.source_id)
        .bind(&event.resource_type)
        .bind(&event.operation)
        .bind(&event.payload)
        .bind(status_to_string(&event.status))
        .bind(event.attempts)
        .bind(event.next_attempt_at)
        .bind(event.processed_at)
        .bind(event.correlation_id.as_deref())
        .bind(event.last_error.as_deref())
        .bind(event.created_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list_pending(&self, limit: usize) -> Result<Vec<OutboundEvent>, AppError> {
        let limit = limit as i64;
        let rows = sqlx::query(
            r#"
            SELECT id,
                   source_id,
                   resource_type,
                   operation,
                   payload,
                   status,
                   attempts,
                   next_attempt_at,
                   processed_at,
                   correlation_id,
                   last_error,
                   created_at
            FROM sync_outbound_event
            WHERE status IN ('pending', 'failed')
              AND (next_attempt_at IS NULL OR next_attempt_at <= NOW())
            ORDER BY created_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(map_row).collect()
    }

    async fn mark_in_progress(&self, event_id: Uuid) -> Result<(), AppError> {
        mark_status(&self.pool, event_id, "in_progress", None, None).await
    }

    async fn mark_succeeded(&self, event_id: Uuid) -> Result<(), AppError> {
        mark_status(&self.pool, event_id, "succeeded", Some(Utc::now()), None).await
    }

    async fn mark_failed(
        &self,
        event_id: Uuid,
        attempts: i32,
        next_attempt_at: Option<DateTime<Utc>>,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        let status = if next_attempt_at.is_some() {
            "failed"
        } else {
            "dead_letter"
        };
        let result = sqlx::query(
            r#"
            UPDATE sync_outbound_event
            SET status = $2,
                attempts = $3,
                next_attempt_at = $4,
                last_error = $5,
                processed_at = CASE WHEN $2 = 'dead_letter' THEN NOW() ELSE processed_at END,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(event_id)
        .bind(status)
        .bind(attempts)
        .bind(next_attempt_at)
        .bind(last_error)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("Outbound event not found".to_string()));
        }
        Ok(())
    }

    async fn replay_dead_letter(&self, source_id: Uuid, limit: usize) -> Result<usize, AppError> {
        let limit = limit as i64;
        let result = sqlx::query(
            r#"
            WITH selected AS (
                SELECT id
                FROM sync_outbound_event
                WHERE source_id = $1
                  AND status = 'dead_letter'
                ORDER BY updated_at ASC
                LIMIT $2
            )
            UPDATE sync_outbound_event AS events
            SET status = 'pending',
                next_attempt_at = NULL,
                processed_at = NULL,
                updated_at = NOW()
            FROM selected
            WHERE events.id = selected.id
            "#,
        )
        .bind(source_id)
        .bind(limit)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() as usize)
    }

    async fn get_counters(&self, source_id: Uuid) -> Result<OutboundEventCounters, AppError> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE status IN ('pending', 'in_progress')) AS pending_count,
                COUNT(*) FILTER (WHERE status IN ('failed', 'dead_letter')) AS failed_count,
                MAX(processed_at) FILTER (WHERE status = 'succeeded') AS last_successful_outbound_at
            FROM sync_outbound_event
            WHERE source_id = $1
            "#,
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(OutboundEventCounters {
            pending_count: row.try_get::<i64, _>("pending_count")?,
            failed_count: row.try_get::<i64, _>("failed_count")?,
            last_successful_outbound_at: row
                .try_get::<Option<DateTime<Utc>>, _>("last_successful_outbound_at")?,
        })
    }
}

async fn mark_status(
    pool: &PgPool,
    event_id: Uuid,
    status: &str,
    processed_at: Option<DateTime<Utc>>,
    last_error: Option<&str>,
) -> Result<(), AppError> {
    let result = sqlx::query(
        r#"
        UPDATE sync_outbound_event
        SET status = $2,
            processed_at = COALESCE($3, processed_at),
            last_error = $4,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(event_id)
    .bind(status)
    .bind(processed_at)
    .bind(last_error)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Outbound event not found".to_string()));
    }
    Ok(())
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<OutboundEvent, AppError> {
    let status: String = row.try_get("status")?;
    Ok(OutboundEvent {
        id: row.try_get("id")?,
        source_id: row.try_get("source_id")?,
        resource_type: row.try_get("resource_type")?,
        operation: row.try_get("operation")?,
        payload: row.try_get("payload")?,
        status: parse_status(&status)?,
        attempts: row.try_get("attempts")?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        processed_at: row.try_get("processed_at")?,
        correlation_id: row.try_get("correlation_id")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
    })
}

fn parse_status(value: &str) -> Result<OutboundEventStatus, AppError> {
    match value {
        "pending" => Ok(OutboundEventStatus::Pending),
        "in_progress" => Ok(OutboundEventStatus::InProgress),
        "succeeded" => Ok(OutboundEventStatus::Succeeded),
        "failed" => Ok(OutboundEventStatus::Failed),
        "dead_letter" => Ok(OutboundEventStatus::DeadLetter),
        other => Err(AppError::Validation(format!(
            "Unknown outbound event status '{}'",
            other
        ))),
    }
}

fn status_to_string(status: &OutboundEventStatus) -> &'static str {
    match status {
        OutboundEventStatus::Pending => "pending",
        OutboundEventStatus::InProgress => "in_progress",
        OutboundEventStatus::Succeeded => "succeeded",
        OutboundEventStatus::Failed => "failed",
        OutboundEventStatus::DeadLetter => "dead_letter",
    }
}
