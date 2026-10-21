use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::PowerPanel;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, filter_column, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const POWER_PANEL_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.powerPanels.columns.name"),
    ListColumn::fk_filter(
        "site_id",
        "site_id",
        "site_id",
        "site_id",
        "sites",
        "inventory.powerPanels.columns.site",
    ),
    ListColumn::fk_filter(
        "location_id",
        "location_id",
        "location_id",
        "location_id",
        "locations",
        "inventory.powerPanels.columns.location",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.powerPanels.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.powerPanels.columns.comments",
    ),
];

#[derive(Debug, Default)]
pub struct PowerPanelFilter {
    pub search: Option<String>,
    pub site_id: Vec<Uuid>,
    pub location_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait PowerPanelRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerPanel>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: PowerPanelFilter,
    ) -> Result<(Vec<PowerPanel>, i64), AppError>;
    async fn create(&self, item: &PowerPanel) -> Result<(), AppError>;
    async fn update(&self, item: &PowerPanel) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxPowerPanelRepository {
    pool: PgPool,
}

impl SqlxPowerPanelRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<PowerPanel, AppError> {
        Ok(PowerPanel {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel name: {}", e))
            })?,
            site_id: row.try_get("site_id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel site_id: {}", e))
            })?,
            location_id: row.try_get("location_id").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel location_id: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel comments: {}", e))
            })?,
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &PowerPanelFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "description", "comments"],
            filter.search.as_deref(),
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_PANEL_COLUMNS, "power panel", "site_id")?.sql,
            &filter.site_id,
        );
        push_uuid_in(
            builder,
            &mut where_state,
            filter_column(POWER_PANEL_COLUMNS, "power panel", "location_id")?.sql,
            &filter.location_id,
        );
        Ok(())
    }

    async fn count_filtered(&self, filter: &PowerPanelFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_powerpanel");
        Self::push_list_filters(&mut builder, filter)?;
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count dcim_powerpanels: {}", e)))
    }
}

impl PowerPanelRepository for SqlxPowerPanelRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PowerPanel>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, site_id, location_id, description, comments, custom_field_data
            FROM dcim_powerpanel
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load dcim_powerpanel: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: PowerPanelFilter,
    ) -> Result<(Vec<PowerPanel>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, site_id, location_id, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_powerpanel
            "#,
        );
        Self::push_list_filters(&mut builder, &filter)?;
        push_order_by(
            &mut builder,
            POWER_PANEL_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list dcim_powerpanels: {}", e))
            })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read dcim_powerpanel total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<PowerPanel>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &PowerPanel) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_powerpanel (
                id, custom_field_data, name, site_id, location_id, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.site_id)
        .bind(item.location_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create dcim_powerpanel: {}", e)))?;
        Ok(())
    }

    async fn update(&self, item: &PowerPanel) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_powerpanel
            SET name = $2, site_id = $3, location_id = $4, description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(item.site_id)
        .bind(item.location_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update dcim_powerpanel: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_powerpanel", id)
            .await
            .map(|_| ())
    }
}
