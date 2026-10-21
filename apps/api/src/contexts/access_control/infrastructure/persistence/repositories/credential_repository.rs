use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::repository::UserCredentialRepository;
use crate::contexts::access_control::domain::UserCredential;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxUserCredentialRepository {
    pool: PgPool,
}

impl SqlxUserCredentialRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_credential_row(row: &PgRow) -> Result<UserCredential, AppError> {
        Ok(UserCredential {
            user_id: row
                .try_get("user_id")
                .map_err(|e| AppError::Database(format!("Failed to read user_id: {}", e)))?,
            password_hash: row
                .try_get("password_hash")
                .map_err(|e| AppError::Database(format!("Failed to read password_hash: {}", e)))?,
            password_changed_at: row.try_get("password_changed_at").map_err(|e| {
                AppError::Database(format!("Failed to read password_changed_at: {}", e))
            })?,
        })
    }
}

#[async_trait]
impl UserCredentialRepository for SqlxUserCredentialRepository {
    async fn get_by_user_id(&self, user_id: Uuid) -> Result<Option<UserCredential>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT user_id, password_hash, password_changed_at
            FROM auth_user_credential
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load user credential: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_credential_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn upsert_credential_hash(
        &self,
        user_id: Uuid,
        password_hash: &str,
        password_changed_at: DateTime<Utc>,
    ) -> Result<UserCredential, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id)
            DO UPDATE SET password_hash = EXCLUDED.password_hash,
                          password_changed_at = EXCLUDED.password_changed_at
            RETURNING user_id, password_hash, password_changed_at
            "#,
        )
        .bind(user_id)
        .bind(password_hash)
        .bind(password_changed_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to upsert user credential: {}", e)))?;

        Self::parse_credential_row(&row)
    }
}
