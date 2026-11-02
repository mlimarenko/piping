// Repository for `source_bootstrap_states`. Used by
// the new bootstrap lifecycle endpoints
// (`POST /api/v1/providers/{id}/bootstrap`, `GET .../bootstrap/status`,
// `POST .../bootstrap/replay`, `GET .../bootstrap/last-error`).

use chrono::Utc;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::sync::domain::{SourceBootstrapState, SourceBootstrapStateStatus};
use crate::errors::AppError;

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SourceBootstrapStateRepository: Send + Sync {
    async fn get(&self, provider_id: Uuid) -> Result<Option<SourceBootstrapState>, AppError>;
    async fn upsert(&self, state: &SourceBootstrapState) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxSourceBootstrapStateRepository {
    pool: PgPool,
}

impl SqlxSourceBootstrapStateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn parse_row(row: &sqlx::postgres::PgRow) -> Result<SourceBootstrapState, AppError> {
    let status_text: String = row.try_get("status")?;
    Ok(SourceBootstrapState {
        provider_id: row.try_get("provider_id")?,
        status: SourceBootstrapStateStatus::from_wire(&status_text).ok_or_else(|| {
            AppError::Internal(format!(
                "source_bootstrap_states row has invalid status: {status_text}"
            ))
        })?,
        attempts: row.try_get("attempts")?,
        attempted_at: row.try_get("attempted_at")?,
        completed_at: row.try_get("completed_at")?,
        last_error: row.try_get("last_error")?,
        updated_at: row.try_get("updated_at")?,
    })
}

impl SourceBootstrapStateRepository for SqlxSourceBootstrapStateRepository {
    async fn get(&self, provider_id: Uuid) -> Result<Option<SourceBootstrapState>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT provider_id, status, attempts, attempted_at, completed_at,
                   last_error, updated_at
            FROM source_bootstrap_states
            WHERE provider_id = $1
            "#,
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn upsert(&self, state: &SourceBootstrapState) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO source_bootstrap_states (
                provider_id, status, attempts,
                attempted_at, completed_at, last_error, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (provider_id) DO UPDATE SET
                status = EXCLUDED.status,
                attempts = EXCLUDED.attempts,
                attempted_at = EXCLUDED.attempted_at,
                completed_at = EXCLUDED.completed_at,
                last_error = EXCLUDED.last_error,
                updated_at = $7
            "#,
        )
        .bind(state.provider_id)
        .bind(state.status.as_str())
        .bind(state.attempts)
        .bind(state.attempted_at)
        .bind(state.completed_at)
        .bind(state.last_error.as_deref())
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
