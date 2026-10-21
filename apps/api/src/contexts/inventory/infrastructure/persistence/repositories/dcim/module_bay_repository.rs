//! Module bay repository — full CRUD over `dcim_modulebay`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_modulebay` HAS the piping envelope columns (`external_id`,
//! `piping_synced_at`, `tags`), but this repository neither reads nor
//! writes them — as with the other half of the inventory repositories.
//! Nothing populates `external_id` anywhere today, so this is an
//! unfinished envelope rather than a data-loss path; wire it up here when
//! the sync engine actually starts writing it.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const MODULE_BAY_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.moduleBays.columns.device",
    ),
    ListColumn::fk_filter(
        "module_id",
        "module_id",
        "module_id",
        "module_id",
        "modules",
        "inventory.moduleBays.columns.module",
    ),
    ListColumn::sortable("name", "name", "inventory.moduleBays.columns.name"),
    ListColumn::sortable("label", "label", "inventory.moduleBays.columns.label"),
    ListColumn::sortable(
        "position",
        "position",
        "inventory.moduleBays.columns.position",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.moduleBays.columns.description",
    ),
];

/// Representation of a `dcim_modulebay` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ModuleBayRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    /// The installed module occupying this bay, if any.
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    /// Physical position identifier within the device (e.g. `"1"`, `"A"`).
    pub position: String,
    pub description: String,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_modulebay` row.
#[derive(Debug, Clone)]
pub struct NewModuleBay {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: Option<String>,
    pub position: Option<String>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `dcim_modulebay` row.
#[derive(Debug, Clone)]
pub struct UpdateModuleBay {
    pub name: Option<String>,
    pub label: Option<String>,
    pub position: Option<String>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkModuleBayResult {
    pub created: Vec<ModuleBayRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleBayFilter {
    pub device_id: Vec<Uuid>,
    pub module_id: Vec<Uuid>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ModuleBayRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleBayRecord>, AppError>;
    async fn list(
        &self,
        filter: ModuleBayFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ModuleBayRecord>, i64), AppError>;
    async fn create(&self, new_bay: &NewModuleBay) -> Result<ModuleBayRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateModuleBay,
    ) -> Result<Option<ModuleBayRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewModuleBay]) -> Result<BulkModuleBayResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxModuleBayRepository {
    pool: PgPool,
}

impl SqlxModuleBayRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ModuleBayRecord, AppError> {
        Ok(ModuleBayRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read module_bay id: {}", e)))?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay device_id: {}", e))
            })?,
            module_id: row.try_get::<Option<Uuid>, _>("module_id").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay module_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay name: {}", e))
            })?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay label: {}", e))
            })?,
            position: row.try_get("position").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay position: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay description: {}", e))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &ModuleBayFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(MODULE_BAY_COLUMNS, "module bay", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(MODULE_BAY_COLUMNS, "module bay", "module_id")?.sql,
            &filter.module_id,
        );
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "label", "position", "description"],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &ModuleBayFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_modulebay");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count module_bays: {}", e)))
    }
}

impl ModuleBayRepository for SqlxModuleBayRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleBayRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, module_id, name, label, position, description, custom_field_data
            FROM dcim_modulebay
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load module_bay: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: ModuleBayFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ModuleBayRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, module_id, name, label, position, description, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_modulebay
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            MODULE_BAY_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list module_bays: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read module_bay total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<ModuleBayRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_bay: &NewModuleBay) -> Result<ModuleBayRecord, AppError> {
        let label = new_bay.label.clone().unwrap_or_default();
        let position = new_bay.position.clone().unwrap_or_default();
        let description = new_bay.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO dcim_modulebay (
                id, custom_field_data, device_id, module_id, name, label,
                position, description,
                level, lft, rght, tree_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, 0, 0, 0, 0)
            "#,
        )
        .bind(new_bay.id)
        .bind(new_bay.device_id)
        .bind(new_bay.module_id)
        .bind(&new_bay.name)
        .bind(&label)
        .bind(&position)
        .bind(&description)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create module_bay: {}", e)))?;

        Ok(ModuleBayRecord {
            id: new_bay.id,
            device_id: new_bay.device_id,
            module_id: new_bay.module_id,
            name: new_bay.name.clone(),
            label,
            position,
            description,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateModuleBay,
    ) -> Result<Option<ModuleBayRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let label = changes.label.clone().unwrap_or(existing.label);
        let position = changes.position.clone().unwrap_or(existing.position);
        let description = changes.description.clone().unwrap_or(existing.description);

        sqlx::query(
            r#"
            UPDATE dcim_modulebay
            SET name = $2, label = $3, position = $4, description = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&label)
        .bind(&position)
        .bind(&description)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update module_bay: {}", e)))?;

        Ok(Some(ModuleBayRecord {
            id,
            device_id: existing.device_id,
            module_id: existing.module_id,
            name,
            label,
            position,
            description,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(&self, items: &[NewModuleBay]) -> Result<BulkModuleBayResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let label = item.label.clone().unwrap_or_default();
            let position = item.position.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_modulebay (
                    id, custom_field_data, device_id, module_id, name, label,
                    position, description,
                    level, lft, rght, tree_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, 0, 0, 0, 0)
                "#,
                )
                .bind(item.id)
                .bind(item.device_id)
                .bind(item.module_id)
                .bind(&item.name)
                .bind(&label)
                .bind(&position)
                .bind(&description),
            )
            .await?;

            match result {
                Ok(_) => created.push(ModuleBayRecord {
                    id: item.id,
                    device_id: item.device_id,
                    module_id: item.module_id,
                    name: item.name.clone(),
                    label,
                    position,
                    description,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!(
                        "Failed to insert module_bay '{}': {}",
                        item.name, e
                    ));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk module_bay insert: {}", e))
        })?;

        Ok(BulkModuleBayResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_modulebay", id)
            .await
            .map(|_| ())
    }
}
