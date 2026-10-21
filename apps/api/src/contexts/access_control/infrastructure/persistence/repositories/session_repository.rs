use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::repository::AuthSessionRepository;
use crate::contexts::access_control::domain::AuthSession;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::audit::infrastructure::persistence::insert_audit_event;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxAuthSessionRepository {
    pool: PgPool,
}

impl SqlxAuthSessionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_session_row(row: &PgRow) -> Result<AuthSession, AppError> {
        Ok(AuthSession {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read session id: {}", e)))?,
            user_id: row.try_get("user_id").map_err(|e| {
                AppError::Database(format!("Failed to read session user_id: {}", e))
            })?,
            expires_at: row
                .try_get("expires_at")
                .map_err(|e| AppError::Database(format!("Failed to read expires_at: {}", e)))?,
            revoked_at: row
                .try_get("revoked_at")
                .map_err(|e| AppError::Database(format!("Failed to read revoked_at: {}", e)))?,
        })
    }
}

#[async_trait]
impl AuthSessionRepository for SqlxAuthSessionRepository {
    async fn create_session(
        &self,
        session: AuthSession,
        audit_event: AuditEvent,
    ) -> Result<AuthSession, AppError> {
        let mut transaction = self.pool.begin().await.map_err(|error| {
            AppError::Database(format!("Failed to begin auth session transaction: {error}"))
        })?;
        sqlx::query(
            r#"
            INSERT INTO auth_session (id, user_id, expires_at, revoked_at)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(session.id)
        .bind(session.user_id)
        .bind(session.expires_at)
        .bind(session.revoked_at)
        .execute(&mut *transaction)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create auth session: {}", e)))?;

        insert_audit_event(&mut *transaction, &audit_event).await?;

        transaction.commit().await.map_err(|error| {
            AppError::Database(format!(
                "Failed to commit auth session transaction: {error}"
            ))
        })?;

        Ok(session)
    }

    async fn get_session(&self, session_id: Uuid) -> Result<Option<AuthSession>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, user_id, expires_at, revoked_at
            FROM auth_session
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load auth session: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_session_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn extend_session(
        &self,
        session_id: Uuid,
        new_expires_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE auth_session
            SET expires_at = $2
            WHERE id = $1 AND revoked_at IS NULL AND expires_at > NOW()
            "#,
        )
        .bind(session_id)
        .bind(new_expires_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to extend auth session: {}", e)))?;

        Ok(())
    }

    async fn revoke_session(
        &self,
        session_id: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE auth_session
            SET revoked_at = $2
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .bind(revoked_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to revoke auth session: {}", e)))?;

        Ok(())
    }
}
