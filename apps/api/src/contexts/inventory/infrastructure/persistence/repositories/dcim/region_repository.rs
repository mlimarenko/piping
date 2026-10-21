use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::list_query::{ListColumn, SortSpec, push_order_by, push_uuid_in};
use super::super::repository::RegionRepository;
use crate::contexts::inventory::domain::{Region, RegionListView};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const REGION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "r.name", "inventory.regions.columns.name"),
    ListColumn::fk_filter(
        "parent",
        "p.name",
        "parent_id",
        "r.parent_id",
        "regions",
        "inventory.regions.columns.parent",
    ),
];

#[derive(Clone)]
pub struct SqlxRegionRepository {
    pool: PgPool,
}

impl SqlxRegionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_region_row(row: &PgRow) -> Result<Region, AppError> {
        Ok(Region {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read region id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read region external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read region name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read region slug: {}", e)))?,
            parent_id: row.try_get("parent_id").ok(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read region piping_synced_at: {}", e))
                })?,
        })
    }
}

impl RegionRepository for SqlxRegionRepository {
    async fn get_by_id(&self, region_id: Uuid) -> Result<Option<Region>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, parent_id, description, tags,
                   custom_field_data, piping_synced_at
            FROM dcim_region
            WHERE id = $1
            "#,
        )
        .bind(region_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load region: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_region_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Region>, AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let rows = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, parent_id, description, tags,
                   custom_field_data, piping_synced_at
            FROM dcim_region
            ORDER BY name
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list regions: {}", e)))?;

        rows.iter()
            .map(Self::parse_region_row)
            .collect::<Result<Vec<Region>, AppError>>()
    }

    async fn list_hydrated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        parent_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<RegionListView>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT r.id, r.name, r.slug, r.parent_id, r.description,
                   p.name AS parent_name,
                   COUNT(*) OVER() AS total_count
            FROM dcim_region r
            LEFT JOIN dcim_region p ON p.id = r.parent_id
            "#,
        );

        let mut where_state = WhereState::new();
        if let Some(search_term) = search {
            where_state.and(&mut builder);
            builder.push("r.name ILIKE ");
            builder.push_bind(search_term.to_owned());
        }
        push_uuid_in(&mut builder, &mut where_state, "r.parent_id", parent_id);

        push_order_by(
            &mut builder,
            REGION_COLUMNS,
            sort.as_ref(),
            "r.name ASC, r.id",
        );
        if sort.is_some() {
            builder.push(", r.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list regions hydrated: {}", e))
            })?;

        let total: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("total_count").unwrap_or(0))
            .unwrap_or(0);

        let items = rows
            .iter()
            .map(|row| {
                Ok(RegionListView {
                    id: row.try_get("id").map_err(|e| {
                        AppError::Database(format!("Failed to read region id: {}", e))
                    })?,
                    name: row.try_get("name").map_err(|e| {
                        AppError::Database(format!("Failed to read region name: {}", e))
                    })?,
                    slug: row.try_get("slug").map_err(|e| {
                        AppError::Database(format!("Failed to read region slug: {}", e))
                    })?,
                    parent_id: row.try_get("parent_id").ok(),
                    parent_name: row.try_get("parent_name").ok(),
                    description: row
                        .try_get::<Option<String>, _>("description")
                        .ok()
                        .flatten(),
                })
            })
            .collect::<Result<Vec<RegionListView>, AppError>>()?;

        Ok((items, total))
    }

    async fn create(&self, region: &Region) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_region (
                id,
                custom_field_data,
                name,
                slug,
                description,
                lft,
                rght,
                tree_id,
                level,
                parent_id,
                comments,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, 0, 0, 0, 0, $6, '', $7, $8, $9)
            "#,
        )
        .bind(region.id)
        .bind(region.custom_fields.clone())
        .bind(&region.name)
        .bind(&region.slug)
        .bind(region.description.as_deref().unwrap_or(""))
        .bind(region.parent_id)
        .bind(&region.tags)
        .bind(region.external_id)
        .bind(region.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create region: {}", e)))?;
        Ok(())
    }

    async fn update(&self, region: &Region) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_region
            SET name = $2,
                slug = $3,
                parent_id = $4,
                description = $5,
                tags = $6,
                custom_field_data = COALESCE($7, custom_field_data),
                external_id = COALESCE($8, external_id),
                piping_synced_at = COALESCE($9, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(region.id)
        .bind(&region.name)
        .bind(&region.slug)
        .bind(region.parent_id)
        .bind(region.description.as_deref().unwrap_or(""))
        .bind(&region.tags)
        .bind(region.custom_fields.clone())
        .bind(region.external_id)
        .bind(region.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update region: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, region_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_region", region_id)
            .await
            .map(|_| ())
    }
}
