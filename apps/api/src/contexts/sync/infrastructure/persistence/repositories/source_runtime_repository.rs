use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::repository::SourceRuntimeRepository;
use crate::contexts::sync::domain::{
    SourceBootstrapStatus, SourceOperationalStatus, SourceRuntime, SourceRuntimeErrorCode,
};
use crate::errors::AppError;

#[derive(Clone)]
pub struct SqlxSourceRuntimeRepository {
    pool: PgPool,
}

impl SqlxSourceRuntimeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SourceRuntimeRepository for SqlxSourceRuntimeRepository {
    async fn get_by_source_id(&self, source_id: Uuid) -> Result<Option<SourceRuntime>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT source_id,
                   bootstrap_status,
                   operational_status,
                   external_webhook_id,
                   webhook_last_received_at,
                   last_error_code,
                   last_error_message,
                   updated_at
            FROM sync_source_runtime
            WHERE source_id = $1
            "#,
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(map_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn upsert(&self, runtime: &SourceRuntime) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO sync_source_runtime (
                source_id,
                bootstrap_status,
                operational_status,
                external_webhook_id,
                webhook_last_received_at,
                last_error_code,
                last_error_message,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (source_id)
            DO UPDATE SET bootstrap_status = EXCLUDED.bootstrap_status,
                          operational_status = EXCLUDED.operational_status,
                          external_webhook_id = EXCLUDED.external_webhook_id,
                          webhook_last_received_at = EXCLUDED.webhook_last_received_at,
                          last_error_code = EXCLUDED.last_error_code,
                          last_error_message = EXCLUDED.last_error_message,
                          updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(runtime.source_id)
        .bind(bootstrap_status_to_string(&runtime.bootstrap_status))
        .bind(operational_status_to_string(&runtime.operational_status))
        .bind(runtime.external_webhook_id.as_deref())
        .bind(runtime.webhook_last_received_at)
        .bind(
            runtime
                .last_error_code
                .as_ref()
                .map(source_runtime_error_code_to_string),
        )
        .bind(runtime.last_error_message.as_deref())
        .bind(runtime.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<SourceRuntime, AppError> {
    let bootstrap_status: String = row.try_get("bootstrap_status")?;
    let operational_status: String = row.try_get("operational_status")?;
    let last_error_code: Option<String> = row.try_get("last_error_code")?;

    Ok(SourceRuntime {
        source_id: row.try_get("source_id")?,
        bootstrap_status: parse_bootstrap_status(&bootstrap_status)?,
        operational_status: parse_operational_status(&operational_status)?,
        external_webhook_id: row.try_get("external_webhook_id")?,
        webhook_last_received_at: row.try_get("webhook_last_received_at")?,
        last_error_code: last_error_code
            .as_deref()
            .map(parse_source_runtime_error_code)
            .transpose()?,
        last_error_message: row.try_get("last_error_message")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn bootstrap_status_to_string(status: &SourceBootstrapStatus) -> &'static str {
    match status {
        SourceBootstrapStatus::Pending => "pending",
        SourceBootstrapStatus::Running => "running",
        SourceBootstrapStatus::Ready => "ready",
        SourceBootstrapStatus::Failed => "failed",
    }
}

fn operational_status_to_string(status: &SourceOperationalStatus) -> &'static str {
    match status {
        SourceOperationalStatus::Ready => "ready",
        SourceOperationalStatus::Degraded => "degraded",
        SourceOperationalStatus::BootstrapFailed => "bootstrap_failed",
        SourceOperationalStatus::Disabled => "disabled",
    }
}

fn source_runtime_error_code_to_string(code: &SourceRuntimeErrorCode) -> &'static str {
    match code {
        SourceRuntimeErrorCode::BootstrapRequestFailed => "bootstrap_request_failed",
        SourceRuntimeErrorCode::BootstrapInvalidConfig => "bootstrap_invalid_config",
        SourceRuntimeErrorCode::WebhookInvalidSignature => "webhook_invalid_signature",
        SourceRuntimeErrorCode::WebhookDedupConflict => "webhook_dedup_conflict",
        SourceRuntimeErrorCode::WebhookApplyFailed => "webhook_apply_failed",
        SourceRuntimeErrorCode::OutboundPushFailed => "outbound_push_failed",
        SourceRuntimeErrorCode::OutboundRetryExhausted => "outbound_retry_exhausted",
    }
}

fn parse_bootstrap_status(value: &str) -> Result<SourceBootstrapStatus, AppError> {
    match value {
        "pending" => Ok(SourceBootstrapStatus::Pending),
        "running" => Ok(SourceBootstrapStatus::Running),
        "ready" => Ok(SourceBootstrapStatus::Ready),
        "failed" => Ok(SourceBootstrapStatus::Failed),
        other => Err(AppError::Validation(format!(
            "Unknown source bootstrap status '{}'",
            other
        ))),
    }
}

fn parse_operational_status(value: &str) -> Result<SourceOperationalStatus, AppError> {
    match value {
        "ready" => Ok(SourceOperationalStatus::Ready),
        "degraded" => Ok(SourceOperationalStatus::Degraded),
        "bootstrap_failed" => Ok(SourceOperationalStatus::BootstrapFailed),
        "disabled" => Ok(SourceOperationalStatus::Disabled),
        other => Err(AppError::Validation(format!(
            "Unknown source operational status '{}'",
            other
        ))),
    }
}

fn parse_source_runtime_error_code(value: &str) -> Result<SourceRuntimeErrorCode, AppError> {
    match value {
        "bootstrap_request_failed" => Ok(SourceRuntimeErrorCode::BootstrapRequestFailed),
        "bootstrap_invalid_config" => Ok(SourceRuntimeErrorCode::BootstrapInvalidConfig),
        "webhook_invalid_signature" => Ok(SourceRuntimeErrorCode::WebhookInvalidSignature),
        "webhook_dedup_conflict" => Ok(SourceRuntimeErrorCode::WebhookDedupConflict),
        "webhook_apply_failed" => Ok(SourceRuntimeErrorCode::WebhookApplyFailed),
        "outbound_push_failed" => Ok(SourceRuntimeErrorCode::OutboundPushFailed),
        "outbound_retry_exhausted" => Ok(SourceRuntimeErrorCode::OutboundRetryExhausted),
        other => Err(AppError::Validation(format!(
            "Unknown source runtime error code '{}'",
            other
        ))),
    }
}
