use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::repository::SiteGroupRepository;
use crate::contexts::inventory::domain::SiteGroup;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const SITE_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.siteGroups.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.siteGroups.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.siteGroups.columns.slug"),
    ListColumn::fk_filter(
        "parent_id",
        "parent_id",
        "parent_id",
        "parent_id",
        "siteGroups",
        "inventory.siteGroups.columns.parent",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.siteGroups.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.siteGroups.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.siteGroups.columns.syncedAt",
    ),
];

#[derive(Clone)]
pub struct SqlxSiteGroupRepository {
    pool: PgPool,
}

impl SqlxSiteGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_site_group_row(row: &PgRow) -> Result<SiteGroup, AppError> {
        Ok(SiteGroup {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read site group id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read site group external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read site group name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read site group slug: {}", e))
            })?,
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
                    AppError::Database(format!("Failed to read site group piping_synced_at: {}", e))
                })?,
        })
    }

    fn push_list_filters<'a>(
        builder: &mut QueryBuilder<'a, Postgres>,
        search: Option<&'a str>,
        parent_id: &[Uuid],
    ) {
        let mut where_state = WhereState::new();
        if let Some(search) = search {
            where_state.and(builder);
            builder.push("(");
            builder.push_bind(search);
            builder.push("::text IS NULL OR name ILIKE ");
            builder.push_bind(search);
            builder.push(")");
        }
        push_uuid_in(builder, &mut where_state, "parent_id", parent_id);
    }
}

impl SiteGroupRepository for SqlxSiteGroupRepository {
    async fn get_by_id(&self, group_id: Uuid) -> Result<Option<SiteGroup>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, parent_id, description, tags,
                   custom_field_data, piping_synced_at
            FROM dcim_sitegroup
            WHERE id = $1
            "#,
        )
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load site group: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_site_group_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        parent_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<SiteGroup>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, parent_id, description, tags,
                   custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM dcim_sitegroup
            "#,
        );
        Self::push_list_filters(&mut builder, search, parent_id);
        push_order_by(&mut builder, SITE_GROUP_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list site groups: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read site group total_count: {}", e))
            })?,
            None => 0_i64,
        };
        let items = rows
            .iter()
            .map(Self::parse_site_group_row)
            .collect::<Result<Vec<SiteGroup>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, group: &SiteGroup) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_sitegroup (
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
        .bind(group.id)
        .bind(group.custom_fields.clone())
        .bind(&group.name)
        .bind(&group.slug)
        .bind(group.description.as_deref().unwrap_or(""))
        .bind(group.parent_id)
        .bind(&group.tags)
        .bind(group.external_id)
        .bind(group.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create site group: {}", e)))?;
        Ok(())
    }

    async fn update(&self, group: &SiteGroup) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_sitegroup
            SET name = $2,
                slug = $3,
                description = $4,
                parent_id = $5,
                tags = $6,
                custom_field_data = COALESCE($7, custom_field_data),
                external_id = COALESCE($8, external_id),
                piping_synced_at = COALESCE($9, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(group.id)
        .bind(&group.name)
        .bind(&group.slug)
        .bind(group.description.as_deref().unwrap_or(""))
        .bind(group.parent_id)
        .bind(&group.tags)
        .bind(group.custom_fields.clone())
        .bind(group.external_id)
        .bind(group.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update site group: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, group_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_sitegroup", group_id)
            .await
            .map(|_| ())
    }
}
