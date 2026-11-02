use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::PowerFeed;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike,
    push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const POWER_FEED_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.powerFeeds.columns.name"),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "inventory.powerFeeds.columns.status",
    ),
    ListColumn::enum_filter("type", "type", "type", "inventory.powerFeeds.columns.type"),
    ListColumn::enum_filter(
        "supply",
        "supply",
        "supply",
        "inventory.powerFeeds.columns.supply",
    ),
    ListColumn::enum_filter(
        "phase",
        "phase",
        "phase",
        "inventory.powerFeeds.columns.phase",
    ),
    ListColumn::sortable("voltage", "voltage", "inventory.powerFeeds.columns.voltage"),
    ListColumn::sortable(
        "amperage",
        "amperage",
        "inventory.powerFeeds.columns.amperage",
    ),
    ListColumn::sortable(
        "max_utilization",
        "max_utilization",
        "inventory.powerFeeds.columns.maxUtilization",
    ),
    ListColumn::sortable(
        "available_power",
        "available_power",
        "inventory.powerFeeds.columns.availablePower",
    ),
    ListColumn::fk_filter(
        "power_panel_id",
        "power_panel_id",
        "power_panel_id",
        "power_panel_id",
        "powerPanels",
        "inventory.powerFeeds.columns.powerPanel",
    ),
    ListColumn::fk_filter(
        "rack_id",
        "rack_id",
        "rack_id",
        "rack_id",
        "racks",
        "inventory.powerFeeds.columns.rack",
    ),
    ListColumn::enum_filter(
        "mark_connected",
        "mark_connected",
        "mark_connected::text",
        "inventory.powerFeeds.columns.markConnected",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.powerFeeds.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.powerFeeds.columns.comments",
    ),
    ListColumn::fk_filter(
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.powerFeeds.columns.tenant",
    ),
];

#[derive(Debug, Default)]
pub struct PowerFeedFilter {
    pub search: Option<String>,
    pub status: Vec<String>,
    pub r#type: Vec<String>,
    pub supply: Vec<String>,
    pub phase: Vec<String>,
    pub power_panel_id: Vec<Uuid>,
    pub rack_id: Vec<Uuid>,
    pub mark_connected: Vec<String>,
    pub tenant_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait PowerFeedRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerFeed>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: PowerFeedFilter,
    ) -> Result<(Vec<PowerFeed>, i64), AppError>;
    async fn create(&self, item: &PowerFeed) -> Result<(), AppError>;
    async fn update(&self, item: &PowerFeed) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxPowerFeedRepository {
    pool: PgPool,
}

impl SqlxPowerFeedRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<PowerFeed, AppError> {
        Ok(PowerFeed {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed name: {}", e))
            })?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed status: {}", e))
            })?,
            r#type: row.try_get("type").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed type: {}", e))
            })?,
            supply: row.try_get("supply").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed supply: {}", e))
            })?,
            phase: row.try_get("phase").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed phase: {}", e))
            })?,
            voltage: row.try_get("voltage").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed voltage: {}", e))
            })?,
            amperage: row.try_get("amperage").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed amperage: {}", e))
            })?,
            max_utilization: row.try_get("max_utilization").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_powerfeed max_utilization: {}",
                    e
                ))
            })?,
            available_power: row.try_get("available_power").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_powerfeed available_power: {}",
                    e
                ))
            })?,
            power_panel_id: row.try_get("power_panel_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_powerfeed power_panel_id: {}",
                    e
                ))
            })?,
            rack_id: row.try_get("rack_id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed rack_id: {}", e))
            })?,
            mark_connected: row.try_get("mark_connected").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read dcim_powerfeed mark_connected: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed comments: {}", e))
            })?,
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed tenant_id: {}", e))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &PowerFeedFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "name",
                "status",
                "type",
                "supply",
                "phase",
                "description",
                "comments",
            ],
            filter.search.as_deref(),
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "status")?.sql,
            &filter.status,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "type")?.sql,
            &filter.r#type,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "supply")?.sql,
            &filter.supply,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "phase")?.sql,
            &filter.phase,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "power_panel_id")?.sql,
            &filter.power_panel_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "rack_id")?.sql,
            &filter.rack_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "mark_connected")?.sql,
            &filter.mark_connected,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_FEED_COLUMNS, "power feed", "tenant_id")?.sql,
            &filter.tenant_id,
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &PowerFeedFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_powerfeed");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count dcim_powerfeeds: {}", e)))
    }
}

impl PowerFeedRepository for SqlxPowerFeedRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerFeed>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, status, type, supply, phase, voltage, amperage, max_utilization,
                   available_power, power_panel_id, rack_id, mark_connected, description,
                   comments, tenant_id, custom_field_data
            FROM dcim_powerfeed
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load dcim_powerfeed: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: PowerFeedFilter,
    ) -> Result<(Vec<PowerFeed>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, status, type, supply, phase, voltage, amperage, max_utilization,
                   available_power, power_panel_id, rack_id, mark_connected, description,
                   comments, tenant_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_powerfeed
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            POWER_FEED_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list dcim_powerfeeds: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerfeed total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<PowerFeed>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &PowerFeed) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_powerfeed (
                id, custom_field_data, name, status, type, supply, phase, voltage, amperage,
                max_utilization, available_power, power_panel_id, rack_id, mark_connected,
                description, comments, tenant_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.status)
        .bind(&item.r#type)
        .bind(&item.supply)
        .bind(&item.phase)
        .bind(item.voltage)
        .bind(item.amperage)
        .bind(item.max_utilization)
        .bind(item.available_power)
        .bind(item.power_panel_id)
        .bind(item.rack_id)
        .bind(item.mark_connected)
        .bind(&item.description)
        .bind(&item.comments)
        .bind(item.tenant_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create dcim_powerfeed: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &PowerFeed) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_powerfeed
            SET name = $2, status = $3, type = $4, supply = $5, phase = $6, voltage = $7,
                amperage = $8, max_utilization = $9, available_power = $10, power_panel_id = $11,
                rack_id = $12, mark_connected = $13, description = $14, comments = $15,
                tenant_id = $16
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.status)
        .bind(&item.r#type)
        .bind(&item.supply)
        .bind(&item.phase)
        .bind(item.voltage)
        .bind(item.amperage)
        .bind(item.max_utilization)
        .bind(item.available_power)
        .bind(item.power_panel_id)
        .bind(item.rack_id)
        .bind(item.mark_connected)
        .bind(&item.description)
        .bind(&item.comments)
        .bind(item.tenant_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update dcim_powerfeed: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_powerfeed", id)
            .await
            .map(|_| ())
    }
}
