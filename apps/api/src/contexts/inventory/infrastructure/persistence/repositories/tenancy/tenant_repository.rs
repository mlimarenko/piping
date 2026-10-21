use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::list_query::{ListColumn, SortSpec, push_order_by, push_uuid_in};
use super::super::helpers::derive_slug;
use super::super::repository::TenantRepository;
use crate::contexts::inventory::domain::{Tenant, TenantListView};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const TENANT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "t.name", "inventory.tenants.columns.name"),
    ListColumn::fk_filter(
        "tenant_group",
        "tg.name",
        "group_id",
        "t.group_id",
        "tenantGroups",
        "inventory.tenants.columns.tenantGroup",
    ),
];

#[derive(Clone)]
pub struct SqlxTenantRepository {
    pool: PgPool,
}

impl SqlxTenantRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_tenant_row(row: &PgRow) -> Result<Tenant, AppError> {
        Ok(Tenant {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read tenant id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read tenant external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read tenant name: {}", e)))?,
            slug: row.try_get("slug").ok(),
            group_id: row.try_get("group_id").map_err(|e| {
                AppError::Database(format!("Failed to read tenant group_id: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            comments: row.try_get::<Option<String>, _>("comments").ok().flatten(),
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read tenant piping_synced_at: {}", e))
                })?,
        })
    }
}

impl TenantRepository for SqlxTenantRepository {
    async fn get_by_id(&self, tenant_id: Uuid) -> Result<Option<Tenant>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, group_id, description, comments, tags,
                   custom_field_data, piping_synced_at
            FROM tenancy_tenant
            WHERE id = $1
            "#,
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load tenant: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_tenant_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Tenant>, AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let rows = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, group_id, description, comments, tags,
                   custom_field_data, piping_synced_at
            FROM tenancy_tenant
            ORDER BY name
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list tenants: {}", e)))?;

        rows.iter()
            .map(Self::parse_tenant_row)
            .collect::<Result<Vec<Tenant>, AppError>>()
    }

    async fn list_hydrated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        group_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<TenantListView>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT t.id, t.name, t.slug, t.group_id, t.description,
                   tg.name AS group_name,
                   COUNT(*) OVER() AS total_count
            FROM tenancy_tenant t
            LEFT JOIN tenancy_tenantgroup tg ON tg.id = t.group_id
            "#,
        );

        let mut where_state = WhereState::new();
        if let Some(search_term) = search {
            where_state.and(&mut builder);
            builder.push("t.name ILIKE ");
            builder.push_bind(search_term.to_owned());
        }
        push_uuid_in(&mut builder, &mut where_state, "t.group_id", group_id);

        push_order_by(
            &mut builder,
            TENANT_COLUMNS,
            sort.as_ref(),
            "t.name ASC, t.id",
        );
        if sort.is_some() {
            builder.push(", t.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list tenants hydrated: {}", e))
            })?;

        let total: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("total_count").unwrap_or(0))
            .unwrap_or(0);

        let items = rows
            .iter()
            .map(|row| {
                Ok(TenantListView {
                    id: row.try_get("id").map_err(|e| {
                        AppError::Database(format!("Failed to read tenant id: {}", e))
                    })?,
                    name: row.try_get("name").map_err(|e| {
                        AppError::Database(format!("Failed to read tenant name: {}", e))
                    })?,
                    slug: row.try_get("slug").ok(),
                    group_id: row.try_get("group_id").ok(),
                    group_name: row.try_get("group_name").ok(),
                    description: row
                        .try_get::<Option<String>, _>("description")
                        .ok()
                        .flatten(),
                })
            })
            .collect::<Result<Vec<TenantListView>, AppError>>()?;

        Ok((items, total))
    }

    async fn create(&self, tenant: &Tenant) -> Result<(), AppError> {
        let slug = tenant
            .slug
            .clone()
            .unwrap_or_else(|| derive_slug(&tenant.name));
        sqlx::query(
            r#"
            INSERT INTO tenancy_tenant (
                id,
                custom_field_data,
                name,
                slug,
                description,
                comments,
                group_id,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(tenant.id)
        .bind(tenant.custom_fields.clone())
        .bind(&tenant.name)
        .bind(slug.as_str())
        .bind(tenant.description.as_deref().unwrap_or(""))
        .bind(tenant.comments.as_deref().unwrap_or(""))
        .bind(tenant.group_id)
        .bind(&tenant.tags)
        .bind(tenant.external_id)
        .bind(tenant.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create tenant: {}", e)))?;
        Ok(())
    }

    async fn update(&self, tenant: &Tenant) -> Result<(), AppError> {
        let slug = tenant
            .slug
            .clone()
            .unwrap_or_else(|| derive_slug(&tenant.name));
        sqlx::query(
            r#"
            UPDATE tenancy_tenant
            SET name = $2,
                slug = $3,
                group_id = $4,
                description = $5,
                comments = $6,
                tags = $7,
                custom_field_data = COALESCE($8, custom_field_data),
                external_id = COALESCE($9, external_id),
                piping_synced_at = COALESCE($10, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(tenant.id)
        .bind(&tenant.name)
        .bind(slug.as_str())
        .bind(tenant.group_id)
        .bind(tenant.description.as_deref().unwrap_or(""))
        .bind(tenant.comments.as_deref().unwrap_or(""))
        .bind(&tenant.tags)
        .bind(tenant.custom_fields.clone())
        .bind(tenant.external_id)
        .bind(tenant.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update tenant: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, tenant_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "tenancy_tenant", tenant_id)
            .await
            .map(|_| ())
    }
}
