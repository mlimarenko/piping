//! Device bay repository — full CRUD over `dcim_devicebay`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_devicebay` HAS the piping envelope columns (`external_id`,
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
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const DEVICE_BAY_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.deviceBays.columns.device",
    ),
    ListColumn::sortable("name", "name", "inventory.deviceBays.columns.name"),
    ListColumn::sortable("label", "label", "inventory.deviceBays.columns.label"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.deviceBays.columns.description",
    ),
    ListColumn::fk_filter(
        "installed_device_id",
        "installed_device_id",
        "installed_device_id",
        "installed_device_id",
        "devices",
        "inventory.deviceBays.columns.installedDevice",
    ),
];

/// Representation of a `dcim_devicebay` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct DeviceBayRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub name: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub installed_device_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_devicebay` row.
#[derive(Debug, Clone)]
pub struct NewDeviceBay {
    pub id: Uuid,
    pub device_id: Uuid,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub installed_device_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `dcim_devicebay` row.
#[derive(Debug, Clone)]
pub struct UpdateDeviceBay {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub installed_device_id: Option<Option<Uuid>>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkDeviceBayResult {
    pub created: Vec<DeviceBayRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceBayFilter {
    pub device_id: Vec<Uuid>,
    pub installed_device_id: Vec<Uuid>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait DeviceBayRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<DeviceBayRecord>, AppError>;
    async fn list(
        &self,
        filter: DeviceBayFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<DeviceBayRecord>, i64), AppError>;
    async fn create(&self, new_bay: &NewDeviceBay) -> Result<DeviceBayRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateDeviceBay,
    ) -> Result<Option<DeviceBayRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewDeviceBay]) -> Result<BulkDeviceBayResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxDeviceBayRepository {
    pool: PgPool,
}

impl SqlxDeviceBayRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<DeviceBayRecord, AppError> {
        Ok(DeviceBayRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read device_bay id: {}", e)))?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read device_bay device_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read device_bay name: {}", e))
            })?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read device_bay label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read device_bay description: {}", e))
            })?,
            enabled: row.try_get("enabled").map_err(|e| {
                AppError::Database(format!("Failed to read device_bay enabled: {}", e))
            })?,
            installed_device_id: row
                .try_get::<Option<Uuid>, _>("installed_device_id")
                .ok()
                .flatten(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<Postgres>,
        filter: &DeviceBayFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(DEVICE_BAY_COLUMNS, "device bay", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(DEVICE_BAY_COLUMNS, "device bay", "installed_device_id")?.sql,
            &filter.installed_device_id,
        );
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "label", "description"],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &DeviceBayFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_devicebay");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count device_bays: {}", e)))
    }
}

impl DeviceBayRepository for SqlxDeviceBayRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<DeviceBayRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, name, label, description, enabled,
                   installed_device_id, custom_field_data
            FROM dcim_devicebay
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load device_bay: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: DeviceBayFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<DeviceBayRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, name, label, description, enabled,
                   installed_device_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_devicebay
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            DEVICE_BAY_COLUMNS,
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
            .map_err(|e| AppError::Database(format!("Failed to list device_bays: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read device_bay total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<DeviceBayRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_bay: &NewDeviceBay) -> Result<DeviceBayRecord, AppError> {
        let label = new_bay.label.clone().unwrap_or_default();
        let description = new_bay.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO dcim_devicebay (
                id, custom_field_data, device_id, name, label,
                description, installed_device_id, enabled
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(new_bay.id)
        .bind(new_bay.device_id)
        .bind(&new_bay.name)
        .bind(&label)
        .bind(&description)
        .bind(new_bay.installed_device_id)
        .bind(new_bay.enabled)
        .execute(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

        Ok(DeviceBayRecord {
            id: new_bay.id,
            device_id: new_bay.device_id,
            name: new_bay.name.clone(),
            label,
            description,
            enabled: new_bay.enabled,
            installed_device_id: new_bay.installed_device_id,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateDeviceBay,
    ) -> Result<Option<DeviceBayRecord>, AppError> {
        // Merge in the locked UPDATE, not in a preceding pool read: omission
        // must not overwrite an installation or enablement committed meanwhile.
        let row = sqlx::query(
            r#"
            UPDATE dcim_devicebay
            SET name = COALESCE($2, name), label = COALESCE($3, label),
                description = COALESCE($4, description),
                installed_device_id = CASE WHEN $5 THEN $6 ELSE installed_device_id END,
                enabled = COALESCE($7, enabled)
            WHERE id = $1
            RETURNING id, device_id, name, label, description, enabled,
                      installed_device_id, custom_field_data
            "#,
        )
        .bind(id)
        .bind(&changes.name)
        .bind(&changes.label)
        .bind(&changes.description)
        .bind(changes.installed_device_id.is_some())
        .bind(changes.installed_device_id.flatten())
        .bind(changes.enabled)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

        row.as_ref().map(Self::parse_row).transpose()
    }

    async fn bulk_create(&self, items: &[NewDeviceBay]) -> Result<BulkDeviceBayResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let label = item.label.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_devicebay (
                    id, custom_field_data, device_id, name, label,
                    description, installed_device_id, enabled
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7)
                "#,
                )
                .bind(item.id)
                .bind(item.device_id)
                .bind(&item.name)
                .bind(&label)
                .bind(&description)
                .bind(item.installed_device_id)
                .bind(item.enabled),
            )
            .await?;

            match result {
                Ok(_) => created.push(DeviceBayRecord {
                    id: item.id,
                    device_id: item.device_id,
                    name: item.name.clone(),
                    label,
                    description,
                    enabled: item.enabled,
                    installed_device_id: item.installed_device_id,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!(
                        "Failed to insert device_bay '{}': {}",
                        item.name, e
                    ));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk device_bay insert: {}", e))
        })?;

        Ok(BulkDeviceBayResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_devicebay", id)
            .await
            .map(|_| ())
    }
}
