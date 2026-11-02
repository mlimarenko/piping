//! Power port repository — full CRUD over `dcim_powerport`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_powerport` HAS the piping envelope columns (`external_id`,
//! `piping_synced_at`, `tags`), but this repository neither reads nor
//! writes them — as with the other half of the inventory repositories.
//! Nothing populates `external_id` anywhere today, so this is an
//! unfinished envelope rather than a data-loss path; wire it up here when
//! the sync engine actually starts writing it.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const POWER_PORT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.powerPorts.columns.device",
    ),
    ListColumn::fk_filter(
        "module_id",
        "module_id",
        "module_id",
        "module_id",
        "modules",
        "inventory.powerPorts.columns.module",
    ),
    ListColumn::sortable("name", "name", "inventory.powerPorts.columns.name"),
    ListColumn::sortable("label", "label", "inventory.powerPorts.columns.label"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.powerPorts.columns.description",
    ),
    ListColumn::enum_filter("type", "type", "type", "inventory.powerPorts.columns.type"),
    ListColumn::sortable(
        "maximum_draw",
        "maximum_draw",
        "inventory.powerPorts.columns.maximumDraw",
    ),
    ListColumn::sortable(
        "allocated_draw",
        "allocated_draw",
        "inventory.powerPorts.columns.allocatedDraw",
    ),
    ListColumn::enum_filter(
        "mark_connected",
        "mark_connected",
        "mark_connected::text",
        "inventory.powerPorts.columns.markConnected",
    ),
    ListColumn::fk_filter(
        "cable_id",
        "cable_id",
        "cable_id",
        "cable_id",
        "cables",
        "inventory.powerPorts.columns.cable",
    ),
];

/// Representation of a `dcim_powerport` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct PowerPortRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// Stored as `type` in the DB; trailing underscore avoids Rust keyword clash.
    pub type_: Option<String>,
    /// Maximum power draw in watts.
    pub maximum_draw: Option<i32>,
    /// Allocated power draw in watts.
    pub allocated_draw: Option<i32>,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_powerport` row.
#[derive(Debug, Clone)]
pub struct NewPowerPort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub maximum_draw: Option<i32>,
    pub allocated_draw: Option<i32>,
    pub mark_connected: Option<bool>,
}

/// Fields that may be updated on an existing `dcim_powerport` row.
#[derive(Debug, Clone)]
pub struct UpdatePowerPort {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub maximum_draw: Option<i32>,
    pub allocated_draw: Option<i32>,
    pub mark_connected: Option<bool>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkPowerPortResult {
    pub created: Vec<PowerPortRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PowerPortFilter {
    pub device_id: Vec<Uuid>,
    pub module_id: Vec<Uuid>,
    pub type_: Vec<String>,
    pub mark_connected: Vec<String>,
    pub cable_id: Vec<Uuid>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait PowerPortRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerPortRecord>, AppError>;
    async fn list(
        &self,
        filter: PowerPortFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PowerPortRecord>, i64), AppError>;
    async fn create(&self, new_port: &NewPowerPort) -> Result<PowerPortRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePowerPort,
    ) -> Result<Option<PowerPortRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewPowerPort]) -> Result<BulkPowerPortResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxPowerPortRepository {
    pool: PgPool,
}

impl SqlxPowerPortRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<PowerPortRecord, AppError> {
        Ok(PowerPortRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read power_port id: {}", e)))?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read power_port device_id: {}", e))
            })?,
            module_id: row.try_get::<Option<Uuid>, _>("module_id").map_err(|e| {
                AppError::Database(format!("Failed to read power_port module_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read power_port name: {}", e))
            })?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read power_port label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read power_port description: {}", e))
            })?,
            type_: row.try_get::<Option<String>, _>("type").ok().flatten(),
            maximum_draw: row.try_get::<Option<i32>, _>("maximum_draw").ok().flatten(),
            allocated_draw: row
                .try_get::<Option<i32>, _>("allocated_draw")
                .ok()
                .flatten(),
            mark_connected: row.try_get("mark_connected").map_err(|e| {
                AppError::Database(format!("Failed to read power_port mark_connected: {}", e))
            })?,
            cable_id: row.try_get::<Option<Uuid>, _>("cable_id").ok().flatten(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &PowerPortFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_PORT_COLUMNS, "power port", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_PORT_COLUMNS, "power port", "module_id")?.sql,
            &filter.module_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_PORT_COLUMNS, "power port", "type")?.sql,
            &filter.type_,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_PORT_COLUMNS, "power port", "mark_connected")?.sql,
            &filter.mark_connected,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_PORT_COLUMNS, "power port", "cable_id")?.sql,
            &filter.cable_id,
        );
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "name",
                "label",
                "description",
                "type",
                "maximum_draw",
                "allocated_draw",
                "mark_connected",
            ],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &PowerPortFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_powerport");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count power_ports: {}", e)))
    }
}

impl PowerPortRepository for SqlxPowerPortRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerPortRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, maximum_draw, allocated_draw, mark_connected, cable_id, custom_field_data
            FROM dcim_powerport
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load power_port: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: PowerPortFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PowerPortRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, maximum_draw, allocated_draw, mark_connected, cable_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_powerport
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            POWER_PORT_COLUMNS,
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
            .map_err(|e| AppError::Database(format!("Failed to list power_ports: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read power_port total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<PowerPortRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_port: &NewPowerPort) -> Result<PowerPortRecord, AppError> {
        let label = new_port.label.clone().unwrap_or_default();
        let description = new_port.description.clone().unwrap_or_default();
        let mark_connected = new_port.mark_connected.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO dcim_powerport (
                id, custom_field_data, device_id, module_id, name, label,
                description, type, maximum_draw, allocated_draw, mark_connected
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(new_port.id)
        .bind(new_port.device_id)
        .bind(new_port.module_id)
        .bind(&new_port.name)
        .bind(&label)
        .bind(&description)
        .bind(&new_port.type_)
        .bind(new_port.maximum_draw)
        .bind(new_port.allocated_draw)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create power_port: {}", e)))?;

        Ok(PowerPortRecord {
            id: new_port.id,
            device_id: new_port.device_id,
            module_id: new_port.module_id,
            name: new_port.name.clone(),
            label,
            description,
            type_: new_port.type_.clone(),
            maximum_draw: new_port.maximum_draw,
            allocated_draw: new_port.allocated_draw,
            mark_connected,
            cable_id: None,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePowerPort,
    ) -> Result<Option<PowerPortRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let label = changes.label.clone().unwrap_or(existing.label);
        let description = changes.description.clone().unwrap_or(existing.description);
        let type_ = if changes.type_.is_some() {
            changes.type_.clone()
        } else {
            existing.type_
        };
        let maximum_draw = if changes.maximum_draw.is_some() {
            changes.maximum_draw
        } else {
            existing.maximum_draw
        };
        let allocated_draw = if changes.allocated_draw.is_some() {
            changes.allocated_draw
        } else {
            existing.allocated_draw
        };
        let mark_connected = changes.mark_connected.unwrap_or(existing.mark_connected);

        sqlx::query(
            r#"
            UPDATE dcim_powerport
            SET name = $2, label = $3, description = $4, type = $5,
                maximum_draw = $6, allocated_draw = $7, mark_connected = $8
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&label)
        .bind(&description)
        .bind(&type_)
        .bind(maximum_draw)
        .bind(allocated_draw)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update power_port: {}", e)))?;

        Ok(Some(PowerPortRecord {
            id,
            device_id: existing.device_id,
            module_id: existing.module_id,
            name,
            label,
            description,
            type_,
            maximum_draw,
            allocated_draw,
            mark_connected,
            cable_id: existing.cable_id,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(&self, items: &[NewPowerPort]) -> Result<BulkPowerPortResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let label = item.label.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();
            let mark_connected = item.mark_connected.unwrap_or(false);

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_powerport (
                    id, custom_field_data, device_id, module_id, name, label,
                    description, type, maximum_draw, allocated_draw, mark_connected
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
                )
                .bind(item.id)
                .bind(item.device_id)
                .bind(item.module_id)
                .bind(&item.name)
                .bind(&label)
                .bind(&description)
                .bind(&item.type_)
                .bind(item.maximum_draw)
                .bind(item.allocated_draw)
                .bind(mark_connected),
            )
            .await?;

            match result {
                Ok(_) => created.push(PowerPortRecord {
                    id: item.id,
                    device_id: item.device_id,
                    module_id: item.module_id,
                    name: item.name.clone(),
                    label,
                    description,
                    type_: item.type_.clone(),
                    maximum_draw: item.maximum_draw,
                    allocated_draw: item.allocated_draw,
                    mark_connected,
                    cable_id: None,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!(
                        "Failed to insert power_port '{}': {}",
                        item.name, e
                    ));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk power_port insert: {}", e))
        })?;

        Ok(BulkPowerPortResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_powerport", id)
            .await
            .map(|_| ())
    }
}
