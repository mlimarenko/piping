//! Layout repository — canonical SQL surface for inventory layout profiles
//! (system + user). All SQL that touches `inventory_layout_*` tables lives
//! here; handlers and services must never inline SQL for these tables.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::AppError;

// ---------------------------------------------------------------------------
// Row DTOs exposed to the service layer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SystemLayoutRow {
    pub id: Uuid,
    pub entity_type: String,
    pub layout: Value,
    pub version: i32,
    pub is_active: bool,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Boxed `Send` future returned by the object-safe repository trait methods.
type RepoFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait LayoutRepository: Send + Sync {
    fn list_active_system_layouts<'a>(
        &'a self,
    ) -> RepoFuture<'a, Result<Vec<SystemLayoutRow>, AppError>>;

    fn get_active_system_layout<'a>(
        &'a self,
        entity_type: &'a str,
    ) -> RepoFuture<'a, Result<Option<SystemLayoutRow>, AppError>>;

    fn load_system_layout_json<'a>(
        &'a self,
        entity_type: &'a str,
    ) -> RepoFuture<'a, Result<Option<Value>, AppError>>;

    fn load_user_layout_json<'a>(
        &'a self,
        user_id: Uuid,
        entity_type: &'a str,
    ) -> RepoFuture<'a, Result<Option<Value>, AppError>>;

    fn upsert_system_layout<'a>(
        &'a self,
        entity_type: &'a str,
        layout: &'a Value,
        updated_by_user_id: Option<Uuid>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

    fn upsert_user_layout<'a>(
        &'a self,
        user_id: Uuid,
        entity_type: &'a str,
        layout: &'a Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

    fn delete_user_layout<'a>(
        &'a self,
        user_id: Uuid,
        entity_type: &'a str,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;
}

// ---------------------------------------------------------------------------
// Sqlx implementation
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqlxLayoutRepository {
    pool: PgPool,
}

impl SqlxLayoutRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn map_system_layout_row(row: &sqlx::postgres::PgRow) -> Result<SystemLayoutRow, AppError> {
    Ok(SystemLayoutRow {
        id: row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("Failed to parse system layout id: {e}")))?,
        entity_type: row.try_get("entity_type").map_err(|e| {
            AppError::Database(format!("Failed to parse system layout entity_type: {e}"))
        })?,
        layout: row
            .try_get("layout_json")
            .unwrap_or_else(|_| Value::Object(Map::new())),
        version: row.try_get("version").unwrap_or(1),
        is_active: row.try_get("is_active").unwrap_or(true),
        updated_by_user_id: row.try_get("updated_by_user_id").ok(),
        created_at: row.try_get("created_at").map_err(|e| {
            AppError::Database(format!("Failed to parse system layout created_at: {e}"))
        })?,
        updated_at: row.try_get("updated_at").map_err(|e| {
            AppError::Database(format!("Failed to parse system layout updated_at: {e}"))
        })?,
    })
}

impl LayoutRepository for SqlxLayoutRepository {
    fn list_active_system_layouts<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<SystemLayoutRow>, AppError>> + Send + 'a>>
    {
        Box::pin(async move {
            let rows = sqlx::query(
                r#"
                SELECT id, entity_type, layout_json, version, is_active, updated_by_user_id,
                       created_at, updated_at
                FROM inventory_layout_system_profile
                WHERE is_active = TRUE
                ORDER BY entity_type ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to list system layouts: {error}"))
            })?;

            rows.iter().map(map_system_layout_row).collect()
        })
    }

    fn get_active_system_layout<'a>(
        &'a self,
        entity_type: &'a str,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Option<SystemLayoutRow>, AppError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT id, entity_type, layout_json, version, is_active, updated_by_user_id,
                       created_at, updated_at
                FROM inventory_layout_system_profile
                WHERE entity_type = $1 AND is_active = TRUE
                "#,
            )
            .bind(entity_type)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load system layout: {error}"))
            })?;

            let Some(row) = row else {
                return Ok(None);
            };
            Ok(Some(map_system_layout_row(&row)?))
        })
    }

    fn load_system_layout_json<'a>(
        &'a self,
        entity_type: &'a str,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Option<Value>, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT layout_json
                FROM inventory_layout_system_profile
                WHERE entity_type = $1 AND is_active = TRUE
                "#,
            )
            .bind(entity_type)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load system layout: {error}"))
            })?;

            Ok(row.and_then(|row| row.try_get::<Value, _>("layout_json").ok()))
        })
    }

    fn load_user_layout_json<'a>(
        &'a self,
        user_id: Uuid,
        entity_type: &'a str,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Option<Value>, AppError>> + Send + 'a>> {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
                SELECT layout_json
                FROM inventory_layout_user_profile
                WHERE user_id = $1 AND entity_type = $2 AND is_enabled = TRUE
                "#,
            )
            .bind(user_id)
            .bind(entity_type)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to load user layout: {error}")))?;

            Ok(row.and_then(|row| row.try_get::<Value, _>("layout_json").ok()))
        })
    }

    fn upsert_system_layout<'a>(
        &'a self,
        entity_type: &'a str,
        layout: &'a Value,
        updated_by_user_id: Option<Uuid>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO inventory_layout_system_profile (
                    id,
                    entity_type,
                    layout_json,
                    version,
                    is_active,
                    updated_by_user_id
                )
                VALUES ($1, $2, $3, 1, TRUE, $4)
                ON CONFLICT (entity_type)
                DO UPDATE
                    SET layout_json = EXCLUDED.layout_json,
                        version = inventory_layout_system_profile.version + 1,
                        is_active = TRUE,
                        updated_by_user_id = EXCLUDED.updated_by_user_id,
                        updated_at = NOW()
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(entity_type)
            .bind(layout)
            .bind(updated_by_user_id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to upsert system layout: {error}"))
            })?;
            Ok(())
        })
    }

    fn upsert_user_layout<'a>(
        &'a self,
        user_id: Uuid,
        entity_type: &'a str,
        layout: &'a Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO inventory_layout_user_profile (
                    id,
                    user_id,
                    entity_type,
                    layout_json,
                    is_enabled
                )
                VALUES ($1, $2, $3, $4, TRUE)
                ON CONFLICT (user_id, entity_type)
                DO UPDATE
                    SET layout_json = EXCLUDED.layout_json,
                        is_enabled = TRUE,
                        updated_at = NOW()
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(entity_type)
            .bind(layout)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to upsert user layout: {error}"))
            })?;
            Ok(())
        })
    }

    fn delete_user_layout<'a>(
        &'a self,
        user_id: Uuid,
        entity_type: &'a str,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
        Box::pin(async move {
            sqlx::query(
                "DELETE FROM inventory_layout_user_profile WHERE user_id = $1 AND entity_type = $2",
            )
            .bind(user_id)
            .bind(entity_type)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to delete user layout: {error}"))
            })?;
            Ok(())
        })
    }
}
