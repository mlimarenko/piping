use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, FromRow)]
pub struct SettingValueRecord {
    pub key: String,
    pub value: Value,
    pub updated_at: DateTime<Utc>,
    pub updated_by_user_id: Option<Uuid>,
}

#[async_trait]
pub trait SettingsRepository: Send + Sync {
    async fn get_user(
        &self,
        user_id: Uuid,
        key: &str,
    ) -> Result<Option<SettingValueRecord>, AppError>;
    async fn get_instance(&self, key: &str) -> Result<Option<SettingValueRecord>, AppError>;
    async fn list_user(&self, user_id: Uuid) -> Result<Vec<SettingValueRecord>, AppError>;
    async fn list_instance(&self) -> Result<Vec<SettingValueRecord>, AppError>;
    async fn upsert_user(&self, user_id: Uuid, key: &str, value: &Value) -> Result<(), AppError>;
    async fn upsert_instance(
        &self,
        key: &str,
        value: &Value,
        updated_by_user_id: Option<Uuid>,
    ) -> Result<SettingValueRecord, AppError>;
    async fn delete_user(&self, user_id: Uuid, key: &str) -> Result<bool, AppError>;
    async fn delete_instance(&self, key: &str) -> Result<bool, AppError>;
}

#[derive(Clone)]
pub struct SqlxSettingsRepository {
    pool: PgPool,
}

impl SqlxSettingsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SettingsRepository for SqlxSettingsRepository {
    async fn get_user(
        &self,
        user_id: Uuid,
        key: &str,
    ) -> Result<Option<SettingValueRecord>, AppError> {
        sqlx::query_as::<_, SettingValueRecord>(
            r#"
            SELECT key, value, updated_at, updated_by_user_id
            FROM settings_values
            WHERE scope_type = 'user' AND user_id = $1 AND key = $2
            "#,
        )
        .bind(user_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to load user setting: {error}")))
    }

    async fn get_instance(&self, key: &str) -> Result<Option<SettingValueRecord>, AppError> {
        sqlx::query_as::<_, SettingValueRecord>(
            r#"
            SELECT key, value, updated_at, updated_by_user_id
            FROM settings_values
            WHERE scope_type = 'instance' AND key = $1
            "#,
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to load instance setting: {error}")))
    }

    async fn list_user(&self, user_id: Uuid) -> Result<Vec<SettingValueRecord>, AppError> {
        sqlx::query_as::<_, SettingValueRecord>(
            r#"
            SELECT key, value, updated_at, updated_by_user_id
            FROM settings_values
            WHERE scope_type = 'user' AND user_id = $1
            ORDER BY key ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to list user settings: {error}")))
    }

    async fn list_instance(&self) -> Result<Vec<SettingValueRecord>, AppError> {
        sqlx::query_as::<_, SettingValueRecord>(
            r#"
            SELECT key, value, updated_at, updated_by_user_id
            FROM settings_values
            WHERE scope_type = 'instance'
            ORDER BY key ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to list instance settings: {error}")))
    }

    async fn upsert_user(&self, user_id: Uuid, key: &str, value: &Value) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO settings_values (id, scope_type, user_id, key, value, updated_at)
            VALUES ($1, 'user', $2, $3, $4, NOW())
            ON CONFLICT (user_id, key) WHERE scope_type = 'user'
            DO UPDATE
                SET value = EXCLUDED.value,
                    updated_at = NOW()
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to upsert user setting: {error}")))?;
        Ok(())
    }

    async fn upsert_instance(
        &self,
        key: &str,
        value: &Value,
        updated_by_user_id: Option<Uuid>,
    ) -> Result<SettingValueRecord, AppError> {
        sqlx::query_as::<_, SettingValueRecord>(
            r#"
            INSERT INTO settings_values (
                id, scope_type, user_id, key, value, updated_at, updated_by_user_id
            )
            VALUES ($1, 'instance', NULL, $2, $3, NOW(), $4)
            ON CONFLICT (key) WHERE scope_type = 'instance'
            DO UPDATE
                SET value = EXCLUDED.value,
                    updated_at = NOW(),
                    updated_by_user_id = EXCLUDED.updated_by_user_id
            RETURNING key, value, updated_at, updated_by_user_id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(key)
        .bind(value)
        .bind(updated_by_user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to upsert instance setting: {error}")))
    }

    async fn delete_user(&self, user_id: Uuid, key: &str) -> Result<bool, AppError> {
        let result = sqlx::query(
            r#"
            DELETE FROM settings_values
            WHERE scope_type = 'user' AND user_id = $1 AND key = $2
            "#,
        )
        .bind(user_id)
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to delete user setting: {error}")))?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_instance(&self, key: &str) -> Result<bool, AppError> {
        let result = sqlx::query(
            r#"
            DELETE FROM settings_values
            WHERE scope_type = 'instance' AND key = $1
            "#,
        )
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to delete instance setting: {error}"))
        })?;
        Ok(result.rows_affected() > 0)
    }
}
