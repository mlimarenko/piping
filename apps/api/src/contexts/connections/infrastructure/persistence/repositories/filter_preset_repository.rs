use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct ConnectionFilterPreset {
    pub id: Uuid,
    pub name: String,
    pub filters: Value,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SqlxConnectionFilterPresetRepository {
    pool: PgPool,
}

impl SqlxConnectionFilterPresetRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, limit: i64) -> Result<Vec<ConnectionFilterPreset>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, filters, created_by, created_at, updated_at
            FROM connection_filter_presets
            ORDER BY updated_at DESC, lower(name) ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to list connection filter presets: {error}"))
        })?;

        rows.into_iter().map(row_to_preset).collect()
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<ConnectionFilterPreset>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, filters, created_by, created_at, updated_at
            FROM connection_filter_presets
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to load connection filter preset: {error}"))
        })?;

        row.map(row_to_preset).transpose()
    }

    pub async fn create(
        &self,
        id: Uuid,
        name: &str,
        filters: &Value,
        created_by: Uuid,
    ) -> Result<ConnectionFilterPreset, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO connection_filter_presets (id, name, filters, created_by, created_at, updated_at)
            VALUES ($1, $2, $3, $4, NOW(), NOW())
            RETURNING id, name, filters, created_by, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(filters)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(map_write_error)?;

        row_to_preset(row)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        filters: &Value,
    ) -> Result<Option<ConnectionFilterPreset>, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE connection_filter_presets
            SET name = $2, filters = $3, updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, filters, created_by, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(filters)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_write_error)?;

        row.map(row_to_preset).transpose()
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM connection_filter_presets WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!(
                    "Failed to delete connection filter preset: {error}"
                ))
            })?;
        Ok(result.rows_affected() > 0)
    }
}

fn row_to_preset(row: sqlx::postgres::PgRow) -> Result<ConnectionFilterPreset, AppError> {
    Ok(ConnectionFilterPreset {
        id: row.try_get("id").map_err(map_read_error)?,
        name: row.try_get("name").map_err(map_read_error)?,
        filters: row.try_get("filters").map_err(map_read_error)?,
        created_by: row.try_get("created_by").map_err(map_read_error)?,
        created_at: row.try_get("created_at").map_err(map_read_error)?,
        updated_at: row.try_get("updated_at").map_err(map_read_error)?,
    })
}

fn map_read_error(error: sqlx::Error) -> AppError {
    AppError::Database(format!(
        "Failed to decode connection filter preset: {error}"
    ))
}

fn map_write_error(error: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(db_error) = &error
        && db_error.constraint() == Some("ux_connection_filter_presets_name_lower")
    {
        return AppError::Conflict("connection filter preset name already exists".to_string());
    }
    AppError::Database(format!("Failed to write connection filter preset: {error}"))
}
