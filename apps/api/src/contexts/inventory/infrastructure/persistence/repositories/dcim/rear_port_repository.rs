//! Rear port repository — full CRUD over `dcim_rearport`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_rearport` HAS the piping envelope columns (`external_id`,
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

pub const REAR_PORT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.rearPorts.columns.device",
    ),
    ListColumn::fk_filter(
        "module_id",
        "module_id",
        "module_id",
        "module_id",
        "modules",
        "inventory.rearPorts.columns.module",
    ),
    ListColumn::sortable("name", "name", "inventory.rearPorts.columns.name"),
    ListColumn::sortable("label", "label", "inventory.rearPorts.columns.label"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.rearPorts.columns.description",
    ),
    ListColumn::enum_filter("type", "type", "type", "inventory.rearPorts.columns.type"),
    ListColumn::sortable("color", "color", "inventory.rearPorts.columns.color"),
    ListColumn::sortable(
        "positions",
        "positions",
        "inventory.rearPorts.columns.positions",
    ),
    ListColumn::enum_filter(
        "mark_connected",
        "mark_connected",
        "mark_connected::text",
        "inventory.rearPorts.columns.markConnected",
    ),
    ListColumn::fk_filter(
        "cable_id",
        "cable_id",
        "cable_id",
        "cable_id",
        "cables",
        "inventory.rearPorts.columns.cable",
    ),
];

/// Representation of a `dcim_rearport` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct RearPortRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// Stored as `type` in the DB; trailing underscore avoids Rust keyword clash.
    pub type_: String,
    /// HTML hex color (6 characters, no `#`); empty string when unset.
    pub color: String,
    /// Number of front ports that can be mapped to this rear port.
    pub positions: i16,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_rearport` row.
#[derive(Debug, Clone)]
pub struct NewRearPort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: String,
    pub color: Option<String>,
    pub positions: i16,
    pub mark_connected: Option<bool>,
}

/// Fields that may be updated on an existing `dcim_rearport` row.
#[derive(Debug, Clone)]
pub struct UpdateRearPort {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub color: Option<String>,
    pub positions: Option<i16>,
    pub mark_connected: Option<bool>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkRearPortResult {
    pub created: Vec<RearPortRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RearPortFilter {
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
pub trait RearPortRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RearPortRecord>, AppError>;
    async fn list(
        &self,
        filter: RearPortFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RearPortRecord>, i64), AppError>;
    async fn create(&self, new_port: &NewRearPort) -> Result<RearPortRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRearPort,
    ) -> Result<Option<RearPortRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewRearPort]) -> Result<BulkRearPortResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxRearPortRepository {
    pool: PgPool,
}

impl SqlxRearPortRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<RearPortRecord, AppError> {
        Ok(RearPortRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read rear_port id: {}", e)))?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port device_id: {}", e))
            })?,
            module_id: row.try_get::<Option<Uuid>, _>("module_id").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port module_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read rear_port name: {}", e)))?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port description: {}", e))
            })?,
            type_: row
                .try_get::<String, _>("type")
                .map_err(|e| AppError::Database(format!("Failed to read rear_port type: {}", e)))?,
            color: row.try_get("color").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port color: {}", e))
            })?,
            positions: row.try_get("positions").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port positions: {}", e))
            })?,
            mark_connected: row.try_get("mark_connected").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port mark_connected: {}", e))
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
        filter: &RearPortFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(REAR_PORT_COLUMNS, "rear port", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(REAR_PORT_COLUMNS, "rear port", "module_id")?.sql,
            &filter.module_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(REAR_PORT_COLUMNS, "rear port", "type")?.sql,
            &filter.type_,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(REAR_PORT_COLUMNS, "rear port", "mark_connected")?.sql,
            &filter.mark_connected,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(REAR_PORT_COLUMNS, "rear port", "cable_id")?.sql,
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
                "color",
                "positions",
                "mark_connected",
            ],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &RearPortFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_rearport");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count rear_ports: {}", e)))
    }
}

impl RearPortRepository for SqlxRearPortRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RearPortRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, color, positions, mark_connected, cable_id, custom_field_data
            FROM dcim_rearport
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load rear_port: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: RearPortFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RearPortRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, color, positions, mark_connected, cable_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_rearport
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            REAR_PORT_COLUMNS,
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
            .map_err(|e| AppError::Database(format!("Failed to list rear_ports: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read rear_port total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<RearPortRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_port: &NewRearPort) -> Result<RearPortRecord, AppError> {
        let label = new_port.label.clone().unwrap_or_default();
        let description = new_port.description.clone().unwrap_or_default();
        let color = new_port.color.clone().unwrap_or_default();
        let mark_connected = new_port.mark_connected.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO dcim_rearport (
                id, custom_field_data, device_id, module_id, name, label,
                description, type, color, positions, mark_connected
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
        .bind(&color)
        .bind(new_port.positions)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create rear_port: {}", e)))?;

        Ok(RearPortRecord {
            id: new_port.id,
            device_id: new_port.device_id,
            module_id: new_port.module_id,
            name: new_port.name.clone(),
            label,
            description,
            type_: new_port.type_.clone(),
            color,
            positions: new_port.positions,
            mark_connected,
            cable_id: None,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRearPort,
    ) -> Result<Option<RearPortRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let label = changes.label.clone().unwrap_or(existing.label);
        let description = changes.description.clone().unwrap_or(existing.description);
        let type_ = changes.type_.clone().unwrap_or(existing.type_);
        let color = changes.color.clone().unwrap_or(existing.color);
        let positions = changes.positions.unwrap_or(existing.positions);
        let mark_connected = changes.mark_connected.unwrap_or(existing.mark_connected);

        sqlx::query(
            r#"
            UPDATE dcim_rearport
            SET name = $2, label = $3, description = $4, type = $5,
                color = $6, positions = $7, mark_connected = $8
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&label)
        .bind(&description)
        .bind(&type_)
        .bind(&color)
        .bind(positions)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update rear_port: {}", e)))?;

        Ok(Some(RearPortRecord {
            id,
            device_id: existing.device_id,
            module_id: existing.module_id,
            name,
            label,
            description,
            type_,
            color,
            positions,
            mark_connected,
            cable_id: existing.cable_id,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(&self, items: &[NewRearPort]) -> Result<BulkRearPortResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let label = item.label.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();
            let color = item.color.clone().unwrap_or_default();
            let mark_connected = item.mark_connected.unwrap_or(false);

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_rearport (
                    id, custom_field_data, device_id, module_id, name, label,
                    description, type, color, positions, mark_connected
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
                .bind(&color)
                .bind(item.positions)
                .bind(mark_connected),
            )
            .await?;

            match result {
                Ok(_) => created.push(RearPortRecord {
                    id: item.id,
                    device_id: item.device_id,
                    module_id: item.module_id,
                    name: item.name.clone(),
                    label,
                    description,
                    type_: item.type_.clone(),
                    color,
                    positions: item.positions,
                    mark_connected,
                    cable_id: None,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!("Failed to insert rear_port '{}': {}", item.name, e));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk rear_port insert: {}", e))
        })?;

        Ok(BulkRearPortResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_rearport", id)
            .await
            .map(|_| ())
    }
}
