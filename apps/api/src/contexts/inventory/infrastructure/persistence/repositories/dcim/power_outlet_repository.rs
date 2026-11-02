//! Power outlet repository — full CRUD over `dcim_poweroutlet`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! NOTE: `dcim_poweroutlet` HAS the piping envelope columns (`external_id`,
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

pub const POWER_OUTLET_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.powerOutlets.columns.device",
    ),
    ListColumn::fk_filter(
        "module_id",
        "module_id",
        "module_id",
        "module_id",
        "modules",
        "inventory.powerOutlets.columns.module",
    ),
    ListColumn::sortable("name", "name", "inventory.powerOutlets.columns.name"),
    ListColumn::sortable("label", "label", "inventory.powerOutlets.columns.label"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.powerOutlets.columns.description",
    ),
    ListColumn::enum_filter(
        "type",
        "type",
        "type",
        "inventory.powerOutlets.columns.type",
    ),
    ListColumn::enum_filter(
        "feed_leg",
        "feed_leg",
        "feed_leg",
        "inventory.powerOutlets.columns.feedLeg",
    ),
    ListColumn::fk_filter(
        "power_port_id",
        "power_port_id",
        "power_port_id",
        "power_port_id",
        "powerPorts",
        "inventory.powerOutlets.columns.powerPort",
    ),
    ListColumn::enum_filter(
        "mark_connected",
        "mark_connected",
        "mark_connected::text",
        "inventory.powerOutlets.columns.markConnected",
    ),
    ListColumn::fk_filter(
        "cable_id",
        "cable_id",
        "cable_id",
        "cable_id",
        "cables",
        "inventory.powerOutlets.columns.cable",
    ),
];

/// Representation of a `dcim_poweroutlet` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct PowerOutletRecord {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// Stored as `type` in the DB; trailing underscore avoids Rust keyword clash.
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<Uuid>,
    pub mark_connected: bool,
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_poweroutlet` row.
#[derive(Debug, Clone)]
pub struct NewPowerOutlet {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<Uuid>,
    pub mark_connected: Option<bool>,
}

/// Fields that may be updated on an existing `dcim_poweroutlet` row.
#[derive(Debug, Clone)]
pub struct UpdatePowerOutlet {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub type_: Option<String>,
    pub feed_leg: Option<String>,
    pub power_port_id: Option<Uuid>,
    pub mark_connected: Option<bool>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkPowerOutletResult {
    pub created: Vec<PowerOutletRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PowerOutletFilter {
    pub device_id: Vec<Uuid>,
    pub module_id: Vec<Uuid>,
    pub type_: Vec<String>,
    pub feed_leg: Vec<String>,
    pub power_port_id: Vec<Uuid>,
    pub mark_connected: Vec<String>,
    pub cable_id: Vec<Uuid>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait PowerOutletRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerOutletRecord>, AppError>;
    async fn list(
        &self,
        filter: PowerOutletFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PowerOutletRecord>, i64), AppError>;
    async fn create(&self, new_outlet: &NewPowerOutlet) -> Result<PowerOutletRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePowerOutlet,
    ) -> Result<Option<PowerOutletRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewPowerOutlet],
    ) -> Result<BulkPowerOutletResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxPowerOutletRepository {
    pool: PgPool,
}

impl SqlxPowerOutletRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<PowerOutletRecord, AppError> {
        Ok(PowerOutletRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet id: {}", e))
            })?,
            device_id: row.try_get("device_id").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet device_id: {}", e))
            })?,
            module_id: row.try_get::<Option<Uuid>, _>("module_id").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet module_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet name: {}", e))
            })?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet description: {}", e))
            })?,
            type_: row.try_get::<Option<String>, _>("type").ok().flatten(),
            feed_leg: row.try_get::<Option<String>, _>("feed_leg").ok().flatten(),
            power_port_id: row
                .try_get::<Option<Uuid>, _>("power_port_id")
                .ok()
                .flatten(),
            mark_connected: row.try_get("mark_connected").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet mark_connected: {}", e))
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
        filter: &PowerOutletFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "device_id")?.sql,
            &filter.device_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "module_id")?.sql,
            &filter.module_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "type")?.sql,
            &filter.type_,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "feed_leg")?.sql,
            &filter.feed_leg,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "power_port_id")?.sql,
            &filter.power_port_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "mark_connected")?.sql,
            &filter.mark_connected,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_OUTLET_COLUMNS, "power outlet", "cable_id")?.sql,
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
                "feed_leg",
                "mark_connected",
            ],
            filter.search.as_deref(),
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &PowerOutletFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_poweroutlet");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count power_outlets: {}", e)))
    }
}

impl PowerOutletRepository for SqlxPowerOutletRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerOutletRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, feed_leg, power_port_id, mark_connected, cable_id, custom_field_data
            FROM dcim_poweroutlet
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load power_outlet: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: PowerOutletFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PowerOutletRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, device_id, module_id, name, label, description,
                   type, feed_leg, power_port_id, mark_connected, cable_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_poweroutlet
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            POWER_OUTLET_COLUMNS,
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
            .map_err(|e| AppError::Database(format!("Failed to list power_outlets: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read power_outlet total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<PowerOutletRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_outlet: &NewPowerOutlet) -> Result<PowerOutletRecord, AppError> {
        let label = new_outlet.label.clone().unwrap_or_default();
        let description = new_outlet.description.clone().unwrap_or_default();
        let mark_connected = new_outlet.mark_connected.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO dcim_poweroutlet (
                id, custom_field_data, device_id, module_id, name, label,
                description, type, feed_leg, power_port_id, mark_connected,
                color, status
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, '', '')
            "#,
        )
        .bind(new_outlet.id)
        .bind(new_outlet.device_id)
        .bind(new_outlet.module_id)
        .bind(&new_outlet.name)
        .bind(&label)
        .bind(&description)
        .bind(&new_outlet.type_)
        .bind(&new_outlet.feed_leg)
        .bind(new_outlet.power_port_id)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create power_outlet: {}", e)))?;

        Ok(PowerOutletRecord {
            id: new_outlet.id,
            device_id: new_outlet.device_id,
            module_id: new_outlet.module_id,
            name: new_outlet.name.clone(),
            label,
            description,
            type_: new_outlet.type_.clone(),
            feed_leg: new_outlet.feed_leg.clone(),
            power_port_id: new_outlet.power_port_id,
            mark_connected,
            cable_id: None,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePowerOutlet,
    ) -> Result<Option<PowerOutletRecord>, AppError> {
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
        let feed_leg = if changes.feed_leg.is_some() {
            changes.feed_leg.clone()
        } else {
            existing.feed_leg
        };
        let power_port_id = if changes.power_port_id.is_some() {
            changes.power_port_id
        } else {
            existing.power_port_id
        };
        let mark_connected = changes.mark_connected.unwrap_or(existing.mark_connected);

        sqlx::query(
            r#"
            UPDATE dcim_poweroutlet
            SET name = $2, label = $3, description = $4, type = $5,
                feed_leg = $6, power_port_id = $7, mark_connected = $8
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&label)
        .bind(&description)
        .bind(&type_)
        .bind(&feed_leg)
        .bind(power_port_id)
        .bind(mark_connected)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update power_outlet: {}", e)))?;

        Ok(Some(PowerOutletRecord {
            id,
            device_id: existing.device_id,
            module_id: existing.module_id,
            name,
            label,
            description,
            type_,
            feed_leg,
            power_port_id,
            mark_connected,
            cable_id: existing.cable_id,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewPowerOutlet],
    ) -> Result<BulkPowerOutletResult, AppError> {
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
                INSERT INTO dcim_poweroutlet (
                    id, custom_field_data, device_id, module_id, name, label,
                    description, type, feed_leg, power_port_id, mark_connected,
                    color, status
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, '', '')
                "#,
                )
                .bind(item.id)
                .bind(item.device_id)
                .bind(item.module_id)
                .bind(&item.name)
                .bind(&label)
                .bind(&description)
                .bind(&item.type_)
                .bind(&item.feed_leg)
                .bind(item.power_port_id)
                .bind(mark_connected),
            )
            .await?;

            match result {
                Ok(_) => created.push(PowerOutletRecord {
                    id: item.id,
                    device_id: item.device_id,
                    module_id: item.module_id,
                    name: item.name.clone(),
                    label,
                    description,
                    type_: item.type_.clone(),
                    feed_leg: item.feed_leg.clone(),
                    power_port_id: item.power_port_id,
                    mark_connected,
                    cable_id: None,
                    custom_fields: None,
                }),
                Err(e) => {
                    errors.push(format!(
                        "Failed to insert power_outlet '{}': {}",
                        item.name, e
                    ));
                }
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk power_outlet insert: {}", e))
        })?;

        Ok(BulkPowerOutletResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_poweroutlet", id)
            .await
            .map(|_| ())
    }
}
