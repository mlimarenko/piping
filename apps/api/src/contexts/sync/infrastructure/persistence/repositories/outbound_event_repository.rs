use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::super::local_inventory_target::{
    begin_provider_transaction, remove_binding, write_binding,
};
use super::repository::OutboundEventRepository;
use crate::contexts::sync::domain::{InventoryCommitAuthorisation, OutboundBindingChange};
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

    async fn claim(&self, event_id: Uuid) -> Result<Option<OutboundEvent>, AppError> {
        let row = sqlx::query("UPDATE sync_outbound_event SET status='in_progress', updated_at=now() WHERE id=$1 AND status IN ('pending','failed') AND (next_attempt_at IS NULL OR next_attempt_at <= now()) RETURNING *")
            .bind(event_id).fetch_optional(&self.pool).await?;
        row.as_ref().map(map_row).transpose()
    }

    async fn begin_create_attempt(
        &self,
        event: &OutboundEvent,
        local_id: Uuid,
    ) -> Result<bool, AppError> {
        // The provider lease serialises this check with other creates. Attempts survive replay,
        // process failure and duplicate producers; only this owner records a possible POST.
        let result = sqlx::query("UPDATE sync_outbound_event SET attempts=attempts+1, updated_at=now() WHERE id=$1 AND status='in_progress' AND attempts=0 AND NOT EXISTS (SELECT 1 FROM sync_outbound_event prior WHERE prior.source_id=$2 AND prior.resource_type=$3 AND lower(prior.operation)='create' AND prior.payload->>'local_id'=$4 AND prior.id<>$1 AND prior.attempts>0 AND prior.status<>'succeeded')")
            .bind(event.id).bind(event.source_id).bind(&event.resource_type).bind(local_id.to_string())
            .execute(&self.pool).await?;
        Ok(result.rows_affected() == 1)
    }

    async fn complete(
        &self,
        event: &OutboundEvent,
        change: OutboundBindingChange,
        authorisation: &dyn InventoryCommitAuthorisation,
    ) -> Result<(), AppError> {
        let mut transaction = begin_provider_transaction(&self.pool, event.source_id).await?;
        let row = sqlx::query("SELECT status FROM sync_outbound_event WHERE id=$1 FOR UPDATE")
            .bind(event.id)
            .fetch_one(&mut *transaction)
            .await?;
        if row.try_get::<String, _>("status")? != "in_progress" {
            return Err(AppError::Conflict(
                "Outbound event is no longer claimed".into(),
            ));
        }
        match change {
            OutboundBindingChange::None => {}
            OutboundBindingChange::Bind(binding) => {
                validate_event_binding(event, &binding)?;
                write_binding(&mut transaction, &binding, None, None).await?;
            }
            OutboundBindingChange::Remove(binding) => {
                validate_event_binding(event, &binding)?;
                remove_binding(&mut transaction, &binding).await?;
            }
        }
        sqlx::query("UPDATE sync_outbound_event SET status='succeeded', processed_at=now(), last_error=NULL, updated_at=now() WHERE id=$1")
            .bind(event.id).execute(&mut *transaction).await?;
        authorisation.ensure_owned().await?;
        transaction.commit().await?;
        Ok(())
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
                attempts = GREATEST(attempts, $3),
                next_attempt_at = $4,
                last_error = $5,
                processed_at = CASE WHEN $2 = 'dead_letter' THEN NOW() ELSE processed_at END,
                updated_at = NOW()
            WHERE id = $1 AND status = 'in_progress'
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
            return Err(AppError::Conflict(
                "Outbound event is no longer claimed".to_string(),
            ));
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
            WHERE events.id = selected.id AND events.status = 'dead_letter'
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

fn validate_event_binding(
    event: &OutboundEvent,
    binding: &crate::contexts::sync::domain::InventoryBinding,
) -> Result<(), AppError> {
    let local_id = event
        .payload
        .get("local_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    if binding.identity.provider_id != event.source_id
        || binding.identity.entity.source_key() != event.resource_type
        || Some(binding.local_id) != local_id
    {
        return Err(AppError::Validation(
            "Outbound binding does not match event identity".into(),
        ));
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
