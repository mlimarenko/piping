use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::helpers::derive_slug;
use super::super::repository::DeviceRoleRepository;
use crate::contexts::inventory::domain::DeviceRole;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_string_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const DEVICE_ROLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.deviceRoles.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.deviceRoles.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.deviceRoles.columns.slug"),
    ListColumn::sortable("color", "color", "inventory.deviceRoles.columns.color"),
    ListColumn::enum_filter(
        "vm_role",
        "vm_role",
        "vm_role::text",
        "inventory.deviceRoles.columns.vmRole",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.deviceRoles.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.deviceRoles.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.deviceRoles.columns.syncedAt",
    ),
];

#[derive(Clone)]
pub struct SqlxDeviceRoleRepository {
    pool: PgPool,
}

impl SqlxDeviceRoleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_role_row(row: &PgRow) -> Result<DeviceRole, AppError> {
        Ok(DeviceRole {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read device role id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read device role external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read device role name: {}", e))
            })?,
            slug: row.try_get("slug").ok(),
            color: row.try_get("color").ok(),
            vm_role: row.try_get("vm_role").ok(),
            description: row.try_get("description").ok(),
            config_template_id: row.try_get("config_template_id").ok(),
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!(
                        "Failed to read device role piping_synced_at: {}",
                        e
                    ))
                })?,
        })
    }

    fn push_list_filters<'a>(
        builder: &mut QueryBuilder<'a, Postgres>,
        search: Option<&'a str>,
        vm_role: &[String],
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
        push_string_in(builder, &mut where_state, "vm_role::text", vm_role);
    }
}

impl DeviceRoleRepository for SqlxDeviceRoleRepository {
    async fn get_by_id(&self, role_id: Uuid) -> Result<Option<DeviceRole>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, color, vm_role, description,
                   config_template_id, tags, custom_field_data, piping_synced_at
            FROM dcim_devicerole
            WHERE id = $1
            "#,
        )
        .bind(role_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load device role: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_role_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        vm_role: &[String],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<DeviceRole>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, color, vm_role, description,
                   config_template_id, tags, custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM dcim_devicerole
            "#,
        );
        Self::push_list_filters(&mut builder, search, vm_role);
        push_order_by(&mut builder, DEVICE_ROLE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list device roles: {}", e)))?;

        let total: i64 = rows
            .first()
            .map(|r| r.try_get::<i64, _>("total_count").unwrap_or(0))
            .unwrap_or(0);

        let items = rows
            .iter()
            .map(Self::parse_role_row)
            .collect::<Result<Vec<DeviceRole>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, role: &DeviceRole) -> Result<(), AppError> {
        let slug = role.slug.clone().unwrap_or_else(|| derive_slug(&role.name));
        sqlx::query(
            r#"
            INSERT INTO dcim_devicerole (
                id,
                custom_field_data,
                name,
                slug,
                color,
                vm_role,
                description,
                config_template_id,
                level,
                lft,
                rght,
                tree_id,
                comments,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7, $8,
                0, 0, 0, 0, '', $9, $10, $11
            )
            "#,
        )
        .bind(role.id)
        .bind(role.custom_fields.clone())
        .bind(&role.name)
        .bind(slug.as_str())
        .bind(role.color.as_deref().unwrap_or("ffffff"))
        .bind(role.vm_role.unwrap_or(false))
        .bind(role.description.as_deref().unwrap_or(""))
        .bind(role.config_template_id)
        .bind(&role.tags)
        .bind(role.external_id)
        .bind(role.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create device role: {}", e)))?;
        Ok(())
    }

    async fn update(&self, role: &DeviceRole) -> Result<(), AppError> {
        let slug = role.slug.clone().unwrap_or_else(|| derive_slug(&role.name));
        sqlx::query(
            r#"
            UPDATE dcim_devicerole
            SET name = $2,
                slug = $3,
                color = $4,
                vm_role = $5,
                description = $6,
                config_template_id = $7,
                tags = $8,
                custom_field_data = COALESCE($9, custom_field_data),
                external_id = COALESCE($10, external_id),
                piping_synced_at = COALESCE($11, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(role.id)
        .bind(&role.name)
        .bind(slug.as_str())
        .bind(role.color.as_deref().unwrap_or("ffffff"))
        .bind(role.vm_role.unwrap_or(false))
        .bind(role.description.as_deref().unwrap_or(""))
        .bind(role.config_template_id)
        .bind(&role.tags)
        .bind(role.custom_fields.clone())
        .bind(role.external_id)
        .bind(role.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update device role: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, role_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_devicerole", role_id)
            .await
            .map(|_| ())
    }
}
