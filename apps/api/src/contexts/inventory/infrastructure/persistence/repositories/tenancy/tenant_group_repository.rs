use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::helpers::derive_slug;
use super::super::repository::TenantGroupRepository;
use crate::contexts::inventory::domain::TenantGroup;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const TENANT_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.tenantGroups.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.tenantGroups.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.tenantGroups.columns.slug"),
    ListColumn::fk_filter(
        "parent_id",
        "parent_id",
        "parent_id",
        "parent_id",
        "tenantGroups",
        "inventory.tenantGroups.columns.parent",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.tenantGroups.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.tenantGroups.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.tenantGroups.columns.syncedAt",
    ),
];

#[derive(Clone)]
pub struct SqlxTenantGroupRepository {
    pool: PgPool,
}

impl SqlxTenantGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_group_row(row: &PgRow) -> Result<TenantGroup, AppError> {
        Ok(TenantGroup {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read tenant group id: {}", e))
            })?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read tenant group external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read tenant group name: {}", e))
            })?,
            slug: row.try_get("slug").ok(),
            parent_id: row.try_get("parent_id").map_err(|e| {
                AppError::Database(format!("Failed to read tenant group parent_id: {}", e))
            })?,
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
                    AppError::Database(format!(
                        "Failed to read tenant group piping_synced_at: {}",
                        e
                    ))
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

impl TenantGroupRepository for SqlxTenantGroupRepository {
    async fn get_by_id(&self, tenant_group_id: Uuid) -> Result<Option<TenantGroup>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, parent_id, description, tags,
                   custom_field_data, piping_synced_at
            FROM tenancy_tenantgroup
            WHERE id = $1
            "#,
        )
        .bind(tenant_group_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load tenant group: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_group_row(&row)?)),
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
    ) -> Result<(Vec<TenantGroup>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, parent_id, description, tags,
                   custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM tenancy_tenantgroup
            "#,
        );
        Self::push_list_filters(&mut builder, search, parent_id);
        push_order_by(&mut builder, TENANT_GROUP_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list tenant groups: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read tenant group total_count: {}", e))
            })?,
            None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tenancy_tenantgroup")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to count tenant groups: {}", e)))?,
        };
        let items = rows
            .iter()
            .map(Self::parse_group_row)
            .collect::<Result<Vec<TenantGroup>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, tenant_group: &TenantGroup) -> Result<(), AppError> {
        let slug = tenant_group
            .slug
            .clone()
            .unwrap_or_else(|| derive_slug(&tenant_group.name));
        sqlx::query(
            r#"
            INSERT INTO tenancy_tenantgroup (
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
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5,
                1, 2, 1, 0, $6, '', $7, $8, $9
            )
            "#,
        )
        .bind(tenant_group.id)
        .bind(tenant_group.custom_fields.clone())
        .bind(&tenant_group.name)
        .bind(slug.as_str())
        .bind(tenant_group.description.as_deref().unwrap_or(""))
        .bind(tenant_group.parent_id)
        .bind(&tenant_group.tags)
        .bind(tenant_group.external_id)
        .bind(tenant_group.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create tenant group: {}", e)))?;
        Ok(())
    }

    async fn update(&self, tenant_group: &TenantGroup) -> Result<(), AppError> {
        let slug = tenant_group
            .slug
            .clone()
            .unwrap_or_else(|| derive_slug(&tenant_group.name));
        sqlx::query(
            r#"
            UPDATE tenancy_tenantgroup
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
        .bind(tenant_group.id)
        .bind(&tenant_group.name)
        .bind(slug.as_str())
        .bind(tenant_group.parent_id)
        .bind(tenant_group.description.as_deref().unwrap_or(""))
        .bind(&tenant_group.tags)
        .bind(tenant_group.custom_fields.clone())
        .bind(tenant_group.external_id)
        .bind(tenant_group.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update tenant group: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, tenant_group_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "tenancy_tenantgroup",
            tenant_group_id,
        )
        .await
        .map(|_| ())
    }
}
