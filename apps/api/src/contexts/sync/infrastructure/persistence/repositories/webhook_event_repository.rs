use chrono::Utc;
use sqlx::{Error as SqlxError, PgPool, Row};
use uuid::Uuid;

use super::repository::WebhookEventRepository;
use crate::contexts::sync::domain::{InboundWebhookEvent, InboundWebhookEventStatus};
use crate::errors::AppError;

#[derive(Clone)]
pub struct SqlxWebhookEventRepository {
    pool: PgPool,
}

impl SqlxWebhookEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl WebhookEventRepository for SqlxWebhookEventRepository {
    async fn get_by_dedup_key(
        &self,
        source_id: Uuid,
        dedup_key: &str,
    ) -> Result<Option<InboundWebhookEvent>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id,
                   source_id,
                   dedup_key,
                   event_type,
                   model,
                   request_id,
                   status,
                   payload_hash,
                   received_at,
                   processed_at,
                   last_error
            FROM sync_webhook_event
            WHERE source_id = $1 AND dedup_key = $2
            "#,
        )
        .bind(source_id)
        .bind(dedup_key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(map_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn create(&self, event: &InboundWebhookEvent) -> Result<(), AppError> {
        // NOTE: `payload` (the raw JSON body) is intentionally NOT inserted here.
        // Only the SHA-256 hash is persisted.
        sqlx::query(
            r#"
            INSERT INTO sync_webhook_event (
                id,
                source_id,
                dedup_key,
                event_type,
                model,
                request_id,
                status,
                payload_hash,
                received_at,
                processed_at,
                last_error
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(event.id)
        .bind(event.source_id)
        .bind(&event.dedup_key)
        .bind(&event.event_type)
        .bind(&event.model)
        .bind(event.request_id.as_deref())
        .bind(status_to_string(&event.status))
        .bind(&event.payload_hash)
        .bind(event.received_at)
        .bind(event.processed_at)
        .bind(event.last_error.as_deref())
        .execute(&self.pool)
        .await
        .map_err(map_webhook_write_error)?;

        Ok(())
    }

    async fn update_status(
        &self,
        event_id: Uuid,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        let result = sqlx::query(
            r#"
            UPDATE sync_webhook_event
            SET status = $2,
                processed_at = CASE WHEN $2 IN ('applied', 'failed', 'ignored', 'deduplicated') THEN NOW() ELSE processed_at END,
                last_error = $3,
                updated_at = $4
            WHERE id = $1
            "#,
        )
        .bind(event_id)
        .bind(status)
        .bind(last_error)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("Webhook event not found".to_string()));
        }
        Ok(())
    }

    async fn prune_older_than(&self, days: i64) -> Result<u64, AppError> {
        let result = sqlx::query(
            r#"
            DELETE FROM sync_webhook_event
            WHERE received_at < NOW() - ($1 * INTERVAL '1 day')
            "#,
        )
        .bind(days)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

fn map_webhook_write_error(error: SqlxError) -> AppError {
    if let SqlxError::Database(db_error) = &error {
        let is_unique = db_error.code().map(|code| code == "23505").unwrap_or(false);
        let constraint = db_error.constraint().unwrap_or_default();
        if is_unique && constraint.contains("sync_webhook_event_source_id_dedup_key_key") {
            return AppError::Conflict("Webhook event already exists".to_string());
        }
    }
    AppError::DatabaseSource(error)
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<InboundWebhookEvent, AppError> {
    let status: String = row.try_get("status")?;
    Ok(InboundWebhookEvent {
        id: row.try_get("id")?,
        source_id: row.try_get("source_id")?,
        dedup_key: row.try_get("dedup_key")?,
        event_type: row.try_get("event_type")?,
        model: row.try_get("model")?,
        request_id: row.try_get("request_id")?,
        status: parse_status(&status)?,
        payload_hash: row.try_get("payload_hash")?,
        // payload is in-memory only; never round-tripped from the DB.
        payload: serde_json::Value::Null,
        received_at: row.try_get("received_at")?,
        processed_at: row.try_get("processed_at")?,
        last_error: row.try_get("last_error")?,
    })
}

fn parse_status(status: &str) -> Result<InboundWebhookEventStatus, AppError> {
    match status {
        "accepted" => Ok(InboundWebhookEventStatus::Accepted),
        "deduplicated" => Ok(InboundWebhookEventStatus::Deduplicated),
        "applied" => Ok(InboundWebhookEventStatus::Applied),
        "ignored" => Ok(InboundWebhookEventStatus::Ignored),
        "failed" => Ok(InboundWebhookEventStatus::Failed),
        other => Err(AppError::Validation(format!(
            "Unknown inbound webhook event status '{}'",
            other
        ))),
    }
}

fn status_to_string(status: &InboundWebhookEventStatus) -> &'static str {
    match status {
        InboundWebhookEventStatus::Accepted => "accepted",
        InboundWebhookEventStatus::Deduplicated => "deduplicated",
        InboundWebhookEventStatus::Applied => "applied",
        InboundWebhookEventStatus::Ignored => "ignored",
        InboundWebhookEventStatus::Failed => "failed",
    }
}
