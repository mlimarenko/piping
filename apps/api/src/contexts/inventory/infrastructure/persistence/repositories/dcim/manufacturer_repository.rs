use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use super::super::repository::ManufacturerRepository;
use crate::contexts::inventory::domain::Manufacturer;
use crate::contexts::shared::list_query::{ListColumn, SortSpec, push_order_by};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const MANUFACTURER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.manufacturers.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.manufacturers.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.manufacturers.columns.slug"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.manufacturers.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.manufacturers.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.manufacturers.columns.syncedAt",
    ),
];

#[derive(Clone)]
pub struct SqlxManufacturerRepository {
    pool: PgPool,
}

impl SqlxManufacturerRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_manufacturer_row(row: &PgRow) -> Result<Manufacturer, AppError> {
        Ok(Manufacturer {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read manufacturer id: {}", e))
            })?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read manufacturer external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read manufacturer name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read manufacturer slug: {}", e))
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
                        "Failed to read manufacturer piping_synced_at: {}",
                        e
                    ))
                })?,
        })
    }
}

impl ManufacturerRepository for SqlxManufacturerRepository {
    async fn get_by_id(&self, manufacturer_id: Uuid) -> Result<Option<Manufacturer>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, description, tags,
                   custom_field_data, piping_synced_at
            FROM dcim_manufacturer
            WHERE id = $1
            "#,
        )
        .bind(manufacturer_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load manufacturer: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_manufacturer_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<Manufacturer>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, description, tags,
                   custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM dcim_manufacturer
            "#,
        );
        builder.push(" WHERE (");
        builder.push_bind(search);
        builder.push("::text IS NULL OR name ILIKE ");
        builder.push_bind(search);
        builder.push(")");
        push_order_by(&mut builder, MANUFACTURER_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list manufacturers: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read manufacturer total_count: {}", e))
            })?,
            None => 0_i64,
        };
        let items = rows
            .iter()
            .map(Self::parse_manufacturer_row)
            .collect::<Result<Vec<Manufacturer>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, manufacturer: &Manufacturer) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_manufacturer (
                id,
                custom_field_data,
                name,
                slug,
                description,
                comments,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, '', $6, $7, $8)
            "#,
        )
        .bind(manufacturer.id)
        .bind(manufacturer.custom_fields.clone())
        .bind(&manufacturer.name)
        .bind(&manufacturer.slug)
        .bind(manufacturer.description.as_deref().unwrap_or(""))
        .bind(&manufacturer.tags)
        .bind(manufacturer.external_id)
        .bind(manufacturer.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create manufacturer: {}", e)))?;
        Ok(())
    }

    async fn update(&self, manufacturer: &Manufacturer) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_manufacturer
            SET name = $2,
                slug = $3,
                description = $4,
                tags = $5,
                custom_field_data = COALESCE($6, custom_field_data),
                external_id = COALESCE($7, external_id),
                piping_synced_at = COALESCE($8, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(manufacturer.id)
        .bind(&manufacturer.name)
        .bind(&manufacturer.slug)
        .bind(manufacturer.description.as_deref().unwrap_or(""))
        .bind(&manufacturer.tags)
        .bind(manufacturer.custom_fields.clone())
        .bind(manufacturer.external_id)
        .bind(manufacturer.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update manufacturer: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, manufacturer_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_manufacturer",
            manufacturer_id,
        )
        .await
        .map(|_| ())
    }
}
