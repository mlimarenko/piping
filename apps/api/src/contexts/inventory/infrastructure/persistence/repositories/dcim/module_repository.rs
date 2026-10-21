//! Module repository — full CRUD over `dcim_module`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_module` HAS the piping envelope columns (`external_id`,
//! `piping_synced_at`, `tags`), but this repository neither reads nor
//! writes them — as with the other half of the inventory repositories.
//! Nothing populates `external_id` anywhere today, so this is an
//! unfinished envelope rather than a data-loss path; wire it up here when
//! the sync engine actually starts writing it.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const MODULE_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.modules.columns.device",
    ),
    ListColumn::fk_filter(
        "module_bay_id",
        "module_bay_id",
        "module_bay_id",
        "module_bay_id",
        "moduleBays",
        "inventory.modules.columns.moduleBay",
    ),
    ListColumn::fk_filter(
        "module_type_id",
        "module_type_id",
        "module_type_id",
        "module_type_id",
        "moduleTypes",
        "inventory.modules.columns.moduleType",
    ),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.modules.columns.status",
    ),
    ListColumn::sortable("serial", "serial", "inventory.modules.columns.serial"),
    ListColumn::sortable(
        "asset_tag",
        "asset_tag",
        "inventory.modules.columns.assetTag",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.modules.columns.description",
    ),
];

/// Representation of a `dcim_module` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ModuleRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_bay_id: Uuid,
    pub module_bay_name: Option<String>,
    pub module_type_id: Uuid,
    pub module_type_name: Option<String>,
    pub status: String,
    pub serial: String,
    pub asset_tag: Option<String>,
    pub description: String,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_module` row.
#[derive(Debug, Clone)]
pub struct NewModule {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_bay_id: Uuid,
    pub module_type_id: Uuid,
    pub status: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `dcim_module` row.
#[derive(Debug, Clone)]
pub struct UpdateModule {
    pub status: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<Option<String>>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkModuleResult {
    pub created: Vec<ModuleRecord>,
    pub errors: Vec<String>,
}

/// Module list filter. Empty vectors mean "no filter"; non-empty vectors emit
/// parameterized `IN` predicates against the repository-local `MODULE_COLUMNS`
/// metadata.
#[derive(Debug, Clone, Default)]
pub struct ModuleFilter {
    pub search: Option<String>,
    pub device_id: Vec<Uuid>,
    pub module_bay_id: Vec<Uuid>,
    pub module_type_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ModuleRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleRecord>, AppError>;
    async fn list(
        &self,
        filter: ModuleFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ModuleRecord>, i64), AppError>;
    async fn create(&self, new_module: &NewModule) -> Result<ModuleRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateModule,
    ) -> Result<Option<ModuleRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewModule]) -> Result<BulkModuleResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxModuleRepository {
    pool: PgPool,
}

impl SqlxModuleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ModuleRecord, AppError> {
        Ok(ModuleRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read module id: {}", e)))?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read module device_id: {}", e))
            })?,
            module_bay_id: row.try_get("module_bay_id").map_err(|e| {
                AppError::Database(format!("Failed to read module module_bay_id: {}", e))
            })?,
            module_bay_name: row
                .try_get::<Option<String>, _>("module_bay_name")
                .ok()
                .flatten(),
            module_type_id: row.try_get("module_type_id").map_err(|e| {
                AppError::Database(format!("Failed to read module module_type_id: {}", e))
            })?,
            module_type_name: row
                .try_get::<Option<String>, _>("module_type_name")
                .ok()
                .flatten(),
            status: row
                .try_get("status")
                .map_err(|e| AppError::Database(format!("Failed to read module status: {}", e)))?,
            serial: row
                .try_get("serial")
                .map_err(|e| AppError::Database(format!("Failed to read module serial: {}", e)))?,
            asset_tag: row.try_get::<Option<String>, _>("asset_tag").ok().flatten(),
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read module description: {}", e))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, sqlx::Postgres>,
        filter: &ModuleFilter,
    ) -> Result<(), AppError> {
        let has_filter = filter.search.is_some()
            || !filter.device_id.is_empty()
            || !filter.module_bay_id.is_empty()
            || !filter.module_type_id.is_empty()
            || !filter.status.is_empty();
        if !has_filter {
            return Ok(());
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["serial", "asset_tag", "status", "description"],
            filter.search.as_deref(),
        );

        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(MODULE_COLUMNS, "module", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(MODULE_COLUMNS, "module", "module_bay_id")?.sql,
            &filter.module_bay_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(MODULE_COLUMNS, "module", "module_type_id")?.sql,
            &filter.module_type_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(MODULE_COLUMNS, "module", "status")?.sql,
            &filter.status,
        );

        Ok(())
    }

    async fn count_filtered(&self, filter: &ModuleFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_module");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count modules: {}", e)))
    }
}

impl ModuleRepository for SqlxModuleRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, module_bay_id, NULL::text AS module_bay_name,
                   module_type_id, NULL::text AS module_type_name, status,
                   serial, asset_tag, description, custom_field_data
            FROM dcim_module
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load module: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: ModuleFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ModuleRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            "
            SELECT id, device_id, module_bay_id, module_bay_name, module_type_id,
                   module_type_name, status, serial, asset_tag, description, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT m.id, m.device_id, m.module_bay_id, mb.name AS module_bay_name,
                       m.module_type_id, mt.model AS module_type_name, m.status,
                       m.serial, m.asset_tag, m.description, m.custom_field_data
                FROM dcim_module m
                JOIN dcim_modulebay mb ON mb.id = m.module_bay_id
                JOIN dcim_moduletype mt ON mt.id = m.module_type_id
            ) module_rows
            ",
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(&mut builder, MODULE_COLUMNS, filter.sort.as_ref(), "id");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list modules: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read module total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<ModuleRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_module: &NewModule) -> Result<ModuleRecord, AppError> {
        let status = new_module
            .status
            .clone()
            .unwrap_or_else(|| "active".to_string());
        let serial = new_module.serial.clone().unwrap_or_default();
        let description = new_module.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO dcim_module (
                id, custom_field_data, device_id, module_bay_id, module_type_id,
                status, serial, asset_tag, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, '')
            "#,
        )
        .bind(new_module.id)
        .bind(new_module.device_id)
        .bind(new_module.module_bay_id)
        .bind(new_module.module_type_id)
        .bind(&status)
        .bind(&serial)
        .bind(&new_module.asset_tag)
        .bind(&description)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create module: {}", e)))?;

        Ok(ModuleRecord {
            id: new_module.id,
            device_id: new_module.device_id,
            module_bay_id: new_module.module_bay_id,
            module_bay_name: None,
            module_type_id: new_module.module_type_id,
            module_type_name: None,
            status,
            serial,
            asset_tag: new_module.asset_tag.clone(),
            description,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateModule,
    ) -> Result<Option<ModuleRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let status = changes.status.clone().unwrap_or(existing.status);
        let serial = changes.serial.clone().unwrap_or(existing.serial);
        let asset_tag = match &changes.asset_tag {
            Some(v) => v.clone(),
            None => existing.asset_tag,
        };
        let description = changes.description.clone().unwrap_or(existing.description);

        sqlx::query(
            r#"
            UPDATE dcim_module
            SET status = $2, serial = $3, asset_tag = $4, description = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&status)
        .bind(&serial)
        .bind(&asset_tag)
        .bind(&description)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update module: {}", e)))?;

        Ok(Some(ModuleRecord {
            id,
            device_id: existing.device_id,
            module_bay_id: existing.module_bay_id,
            module_bay_name: existing.module_bay_name,
            module_type_id: existing.module_type_id,
            module_type_name: existing.module_type_name,
            status,
            serial,
            asset_tag,
            description,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(&self, items: &[NewModule]) -> Result<BulkModuleResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let status = item.status.clone().unwrap_or_else(|| "active".to_string());
            let serial = item.serial.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_module (
                    id, custom_field_data, device_id, module_bay_id, module_type_id,
                    status, serial, asset_tag, description, comments
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, '')
                "#,
                )
                .bind(item.id)
                .bind(item.device_id)
                .bind(item.module_bay_id)
                .bind(item.module_type_id)
                .bind(&status)
                .bind(&serial)
                .bind(&item.asset_tag)
                .bind(&description),
            )
            .await?;

            match result {
                Ok(_) => created.push(ModuleRecord {
                    id: item.id,
                    device_id: item.device_id,
                    module_bay_id: item.module_bay_id,
                    module_bay_name: None,
                    module_type_id: item.module_type_id,
                    module_type_name: None,
                    status,
                    serial,
                    asset_tag: item.asset_tag.clone(),
                    description,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!("Failed to insert module '{}': {}", item.id, e));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk module insert: {}", e))
        })?;

        Ok(BulkModuleResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_module", id)
            .await
            .map(|_| ())
    }
}
