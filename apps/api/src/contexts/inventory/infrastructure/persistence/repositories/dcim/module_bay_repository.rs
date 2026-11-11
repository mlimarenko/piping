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
use sqlx::{Acquire, PgPool, Postgres, QueryBuilder, Row, Transaction};
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
    /// The containing module, not the module occupying the bay.
    pub module_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub enabled: bool,
    pub module_bay_types: Vec<Uuid>,
    pub occupied: bool,
    pub installed_module: Option<InstalledModule>,
    pub is_module_compatible: bool,
    pub name: String,
    pub label: String,
    /// Physical position identifier within the device (e.g. `"1"`, `"A"`).
    pub position: String,
    pub description: String,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct InstalledModule {
    pub id: Uuid,
    pub module_type_id: Uuid,
    pub status: String,
    pub serial: String,
}

macro_rules! module_bay_projection { () => { "dcim_modulebay.*, ARRAY(SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=dcim_modulebay.id ORDER BY modulebaytype_id) AS module_bay_types, (SELECT jsonb_build_object('id',m.id,'module_type_id',m.module_type_id,'status',m.status,'serial',m.serial) FROM dcim_module m WHERE m.module_bay_id=dcim_modulebay.id) AS installed_module, COALESCE((SELECT dcim_module_bay_compatible(dcim_modulebay.id,m.module_type_id) FROM dcim_module m WHERE m.module_bay_id=dcim_modulebay.id),true) AS is_module_compatible" }; }

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
    pub enabled: bool,
    pub module_bay_types: Option<Option<Vec<Uuid>>>,
}

/// Fields that may be updated on an existing `dcim_modulebay` row.
#[derive(Debug, Clone)]
pub struct UpdateModuleBay {
    pub name: Option<String>,
    pub label: Option<String>,
    pub position: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub module_bay_types: Option<Option<Vec<Uuid>>>,
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

    async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        item: &NewModuleBay,
    ) -> Result<ModuleBayRecord, AppError> {
        super::hardware_graph::lock_hardware_graph(tx).await?;
        sqlx::query("SELECT id FROM dcim_device WHERE id=$1 FOR UPDATE")
            .bind(item.device_id)
            .fetch_optional(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO dcim_modulebay (id,custom_field_data,device_id,module_id,name,label,position,description,level,lft,rght,tree_id,enabled) VALUES ($1,'{}',$2,$3,$4,$5,$6,$7,0,0,0,0,$8)")
            .bind(item.id).bind(item.device_id).bind(item.module_id).bind(&item.name)
            .bind(item.label.as_deref().unwrap_or("")).bind(item.position.as_deref().unwrap_or(""))
            .bind(item.description.as_deref().unwrap_or("")).bind(item.enabled)
            .execute(&mut **tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
        super::module_bay_membership::replace(
            tx,
            super::module_bay_membership::ModuleBayCategoryOwner::Bay,
            item.id,
            item.module_bay_types.as_ref(),
        )
        .await?;
        let row = sqlx::query(concat!(
            "SELECT ",
            module_bay_projection!(),
            " FROM dcim_modulebay WHERE id=$1"
        ))
        .bind(item.id)
        .fetch_one(&mut **tx)
        .await?;
        Self::parse_row(&row)
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
            parent_id: row.try_get("parent_id")?,
            enabled: row.try_get("enabled")?,
            module_bay_types: row.try_get("module_bay_types")?,
            occupied: row
                .try_get::<Option<serde_json::Value>, _>("installed_module")?
                .is_some(),
            installed_module: row
                .try_get::<Option<serde_json::Value>, _>("installed_module")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| {
                    AppError::Internal(format!("Invalid installed module projection: {e}"))
                })?,
            is_module_compatible: row.try_get("is_module_compatible")?,
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
                .map_err(crate::errors::classify_sqlx_error_owned)?,
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<Postgres>,
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
        let row = sqlx::query(concat!(
            "SELECT ",
            module_bay_projection!(),
            " FROM dcim_modulebay WHERE id=$1"
        ))
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

        let mut builder = QueryBuilder::new(concat!(
            "SELECT ",
            module_bay_projection!(),
            ", COUNT(*) OVER() AS total_count FROM dcim_modulebay"
        ));
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
        let mut tx = self.pool.begin().await?;
        let record = Self::insert(&mut tx, new_bay).await?;
        tx.commit().await?;
        Ok(record)
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateModuleBay,
    ) -> Result<Option<ModuleBayRecord>, AppError> {
        let mut tx = self.pool.begin().await?;
        // Device graph lock precedes owner reads; metadata patches never read a
        // stale row and overwrite an intervening update.
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        sqlx::query("SELECT d.id FROM dcim_device d JOIN dcim_modulebay b ON b.device_id=d.id WHERE b.id=$1 FOR UPDATE OF d").bind(id).fetch_optional(&mut *tx).await?;
        if sqlx::query("SELECT id FROM dcim_modulebay WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Ok(None);
        }

        sqlx::query(
            r#"
            UPDATE dcim_modulebay
            SET name = COALESCE($2,name), label = COALESCE($3,label), position = COALESCE($4,position), description = COALESCE($5,description), enabled=COALESCE($6,enabled)
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&changes.name)
        .bind(&changes.label)
        .bind(&changes.position)
        .bind(&changes.description)
        .bind(changes.enabled)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update module_bay: {}", e)))?;

        super::module_bay_membership::replace(
            &mut tx,
            super::module_bay_membership::ModuleBayCategoryOwner::Bay,
            id,
            changes.module_bay_types.as_ref(),
        )
        .await?;
        let row = sqlx::query(concat!(
            "SELECT ",
            module_bay_projection!(),
            " FROM dcim_modulebay WHERE id=$1"
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let record = Self::parse_row(&row)?;
        tx.commit().await?;
        Ok(Some(record))
    }

    async fn bulk_create(&self, items: &[NewModuleBay]) -> Result<BulkModuleBayResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let mut savepoint = tx.begin().await?;
            match Self::insert(&mut savepoint, item).await {
                Ok(record) => {
                    savepoint.commit().await?;
                    created.push(record);
                }
                Err(e) => {
                    savepoint.rollback().await?;
                    let message = match e {
                        AppError::Validation(message) | AppError::Conflict(message) => message,
                        _ => "Module bay could not be created".into(),
                    };
                    errors.push(format!(
                        "Failed to insert module_bay '{}': {}",
                        item.name, message
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
