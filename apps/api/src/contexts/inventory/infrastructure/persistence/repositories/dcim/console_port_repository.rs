//! Console port repository — full CRUD over `dcim_consoleport`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_consoleport` HAS the piping envelope columns (`external_id`,
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

pub const CONSOLE_PORT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.consolePorts.columns.device",
    ),
    ListColumn::fk_filter(
        "module_id",
        "module_id",
        "module_id",
        "module_id",
        "modules",
        "inventory.consolePorts.columns.module",
    ),
    ListColumn::sortable("name", "name", "inventory.consolePorts.columns.name"),
    ListColumn::sortable("label", "label", "inventory.consolePorts.columns.label"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.consolePorts.columns.description",
    ),
    ListColumn::enum_filter(
        "type",
        "type",
        "type",
        "inventory.consolePorts.columns.type",
    ),
    ListColumn::sortable("speed", "speed", "inventory.consolePorts.columns.speed"),
    ListColumn::enum_filter(
        "mark_connected",
        "mark_connected",
        "mark_connected::text",
        "inventory.consolePorts.columns.markConnected",
    ),
    ListColumn::fk_filter(
        "cable_id",
        "cable_id",
        "cable_id",
        "cable_id",
        "cables",
        "inventory.consolePorts.columns.cable",
    ),
];

/// Representation of a `dcim_consoleport` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ConsolePortRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// Stored as `type` in the DB; trailing underscore avoids Rust keyword clash.
    pub type_: Option<String>,
    pub speed: Option<i32>,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_consoleport` row.
#[derive(Debug, Clone)]
pub struct NewConsolePort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub speed: Option<i32>,
    pub mark_connected: Option<bool>,
}

/// Fields that may be updated on an existing `dcim_consoleport` row.
#[derive(Debug, Clone)]
pub struct UpdateConsolePort {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub speed: Option<i32>,
    pub mark_connected: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct ConsolePortFilter {
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
pub trait ConsolePortRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ConsolePortRecord>, AppError>;
    async fn list(
        &self,
        filter: ConsolePortFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ConsolePortRecord>, i64), AppError>;
    async fn create(&self, new_port: &NewConsolePort) -> Result<ConsolePortRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateConsolePort,
    ) -> Result<Option<ConsolePortRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewConsolePort],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ConsolePortRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxConsolePortRepository {
    pool: PgPool,
}

impl SqlxConsolePortRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ConsolePortRecord, AppError> {
        Ok(ConsolePortRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read console_port id: {}", e))
            })?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read console_port device_id: {}", e))
            })?,
            module_id: row.try_get::<Option<Uuid>, _>("module_id").map_err(|e| {
                AppError::Database(format!("Failed to read console_port module_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read console_port name: {}", e))
            })?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read console_port label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read console_port description: {}", e))
            })?,
            type_: row.try_get::<Option<String>, _>("type").ok().flatten(),
            speed: row.try_get::<Option<i32>, _>("speed").ok().flatten(),
            mark_connected: row.try_get("mark_connected").map_err(|e| {
                AppError::Database(format!("Failed to read console_port mark_connected: {}", e))
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
        filter: &ConsolePortFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(CONSOLE_PORT_COLUMNS, "console port", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(CONSOLE_PORT_COLUMNS, "console port", "module_id")?.sql,
            &filter.module_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(CONSOLE_PORT_COLUMNS, "console port", "type")?.sql,
            &filter.type_,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(CONSOLE_PORT_COLUMNS, "console port", "mark_connected")?.sql,
            &filter.mark_connected,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(CONSOLE_PORT_COLUMNS, "console port", "cable_id")?.sql,
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
                "speed",
                "mark_connected",
            ],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &ConsolePortFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_consoleport");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count console_ports: {}", e)))
    }
}

impl ConsolePortRepository for SqlxConsolePortRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ConsolePortRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, speed, mark_connected, cable_id, custom_field_data
            FROM dcim_consoleport
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load console_port: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: ConsolePortFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ConsolePortRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, speed, mark_connected, cable_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_consoleport
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            CONSOLE_PORT_COLUMNS,
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
            .map_err(|e| AppError::Database(format!("Failed to list console_ports: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read console_port total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<ConsolePortRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_port: &NewConsolePort) -> Result<ConsolePortRecord, AppError> {
        let label = new_port.label.clone().unwrap_or_default();
        let description = new_port.description.clone().unwrap_or_default();
        let mark_connected = new_port.mark_connected.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO dcim_consoleport (
                id, custom_field_data, device_id, module_id, name, label,
                description, type, speed, mark_connected
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(new_port.id)
        .bind(new_port.device_id)
        .bind(new_port.module_id)
        .bind(&new_port.name)
        .bind(&label)
        .bind(&description)
        .bind(&new_port.type_)
        .bind(new_port.speed)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create console_port: {}", e)))?;

        Ok(ConsolePortRecord {
            id: new_port.id,
            device_id: new_port.device_id,
            module_id: new_port.module_id,
            name: new_port.name.clone(),
            label,
            description,
            type_: new_port.type_.clone(),
            speed: new_port.speed,
            mark_connected,
            cable_id: None,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateConsolePort,
    ) -> Result<Option<ConsolePortRecord>, AppError> {
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
        let speed = if changes.speed.is_some() {
            changes.speed
        } else {
            existing.speed
        };
        let mark_connected = changes.mark_connected.unwrap_or(existing.mark_connected);

        sqlx::query(
            r#"
            UPDATE dcim_consoleport
            SET name = $2, label = $3, description = $4, type = $5,
                speed = $6, mark_connected = $7
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&label)
        .bind(&description)
        .bind(&type_)
        .bind(speed)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update console_port: {}", e)))?;

        Ok(Some(ConsolePortRecord {
            id,
            device_id: existing.device_id,
            module_id: existing.module_id,
            name,
            label,
            description,
            type_,
            speed,
            mark_connected,
            cable_id: existing.cable_id,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewConsolePort],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ConsolePortRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk console_port insert",
            |connection, item| {
                Box::pin(async move {
                    let label = item.label.clone().unwrap_or_default();
                    let description = item.description.clone().unwrap_or_default();
                    let mark_connected = item.mark_connected.unwrap_or(false);

                    sqlx::query(
                        r#"
                INSERT INTO dcim_consoleport (
                    id, custom_field_data, device_id, module_id, name, label,
                    description, type, speed, mark_connected
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9)
                "#,
                    )
                    .bind(item.id)
                    .bind(item.device_id)
                    .bind(item.module_id)
                    .bind(&item.name)
                    .bind(&label)
                    .bind(&description)
                    .bind(&item.type_)
                    .bind(item.speed)
                    .bind(mark_connected)
                    .execute(connection)
                    .await
                    .map_err(|error| {
                        format!("Failed to insert console_port '{}': {error}", item.name)
                    })?;

                    Ok(ConsolePortRecord {
                        id: item.id,
                        device_id: item.device_id,
                        module_id: item.module_id,
                        name: item.name.clone(),
                        label,
                        description,
                        type_: item.type_.clone(),
                        speed: item.speed,
                        mark_connected,
                        cable_id: None,
                        custom_fields: None,
                    })
                })
            },
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_consoleport", id)
            .await
            .map(|_| ())
    }
}
