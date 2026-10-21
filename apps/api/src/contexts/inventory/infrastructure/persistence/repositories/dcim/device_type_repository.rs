use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use super::super::super::list_query::{ListColumn, SortSpec, push_order_by, push_uuid_in};
use super::super::repository::DeviceTypeRepository;
use crate::contexts::inventory::domain::{DeviceType, DeviceTypeListView};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const DEVICE_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("model", "dt.model", "inventory.deviceTypes.columns.model"),
    ListColumn::fk_filter(
        "manufacturer",
        "m.name",
        "manufacturer_id",
        "dt.manufacturer_id",
        "manufacturers",
        "inventory.deviceTypes.columns.manufacturer",
    ),
];

#[derive(Clone)]
pub struct SqlxDeviceTypeRepository {
    pool: PgPool,
}

/// The one projection `parse_device_type_row` understands; the float8 casts
/// are load-bearing (NUMERIC does not decode into f64).
const DEVICE_TYPE_ROW_COLUMNS: &str = "id, external_id, manufacturer_id, model, slug, \
     default_platform_id, part_number, u_height::float8 AS u_height, is_full_depth, \
     subdevice_role, airflow, weight::float8 AS weight, weight_unit, front_image, \
     rear_image, description, comments, tags, custom_field_data, piping_synced_at";

impl SqlxDeviceTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Parse one device-type row. Every SELECT feeding this parser carries the
    /// same column set with `u_height`/`weight` cast to float8 — NUMERIC does
    /// not decode into f64, and under the old `.ok()` reads that meant every
    /// device type answered u_height 1.0 and weight None regardless of the
    /// stored values. Typed reads make that class of drift loud instead.
    fn parse_device_type_row(row: &PgRow) -> Result<DeviceType, AppError> {
        Ok(DeviceType {
            id: col(row, "id")?,
            external_id: col(row, "external_id")?,
            manufacturer_id: col(row, "manufacturer_id")?,
            model: col(row, "model")?,
            slug: col(row, "slug")?,
            default_platform_id: col(row, "default_platform_id")?,
            part_number: col(row, "part_number")?,
            u_height: col(row, "u_height")?,
            is_full_depth: col(row, "is_full_depth")?,
            subdevice_role: col(row, "subdevice_role")?,
            airflow: col(row, "airflow")?,
            weight: col(row, "weight")?,
            weight_unit: col(row, "weight_unit")?,
            front_image: col(row, "front_image")?,
            rear_image: col(row, "rear_image")?,
            description: col(row, "description")?,
            comments: col(row, "comments")?,
            tags: col(row, "tags")?,
            custom_fields: col(row, "custom_field_data")?,
            piping_synced_at: col(row, "piping_synced_at")?,
        })
    }
}

impl DeviceTypeRepository for SqlxDeviceTypeRepository {
    async fn get_by_id(&self, device_type_id: Uuid) -> Result<Option<DeviceType>, AppError> {
        let row = sqlx::query(&format!(
            "SELECT {DEVICE_TYPE_ROW_COLUMNS} FROM dcim_devicetype WHERE id = $1"
        ))
        .bind(device_type_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load device type: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_device_type_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<DeviceType>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM dcim_devicetype")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count device types: {}", e)))?;

        let rows = sqlx::query(&format!(
            "SELECT {DEVICE_TYPE_ROW_COLUMNS} FROM dcim_devicetype ORDER BY model LIMIT $1 OFFSET $2"
        ))
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list device types: {}", e)))?;

        let items = rows
            .iter()
            .map(Self::parse_device_type_row)
            .collect::<Result<Vec<DeviceType>, AppError>>()?;
        Ok((items, total))
    }

    async fn list_hydrated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        manufacturer_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<DeviceTypeListView>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT dt.id, dt.external_id, dt.manufacturer_id, dt.model, dt.slug,
                   dt.default_platform_id, dt.part_number,
                   dt.u_height::float8 AS u_height, dt.is_full_depth,
                   dt.subdevice_role, dt.airflow, dt.weight::float8 AS weight,
                   dt.weight_unit, dt.front_image,
                   dt.rear_image, dt.description, dt.comments, dt.tags, dt.custom_field_data,
                   dt.piping_synced_at,
                   m.name AS manufacturer_name,
                   COUNT(*) OVER() AS total_count
            FROM dcim_devicetype dt
            LEFT JOIN dcim_manufacturer m ON m.id = dt.manufacturer_id
            "#,
        );

        let mut where_state = WhereState::new();
        if let Some(search_term) = search {
            where_state.and(&mut builder);
            builder.push("dt.model ILIKE ");
            builder.push_bind(search_term.to_owned());
        }
        push_uuid_in(
            &mut builder,
            &mut where_state,
            "dt.manufacturer_id",
            manufacturer_id,
        );

        push_order_by(
            &mut builder,
            DEVICE_TYPE_COLUMNS,
            sort.as_ref(),
            "dt.model ASC, dt.id",
        );
        if sort.is_some() {
            builder.push(", dt.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list device types hydrated: {}", e))
        })?;

        let total: i64 = match rows.first() {
            Some(row) => col(row, "total_count")?,
            None => 0,
        };

        let items = rows
            .iter()
            .map(|row| {
                Ok(DeviceTypeListView {
                    record: Self::parse_device_type_row(row)?,
                    manufacturer_name: col(row, "manufacturer_name")?,
                })
            })
            .collect::<Result<Vec<DeviceTypeListView>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, device_type: &DeviceType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_devicetype (
                id,
                custom_field_data,
                manufacturer_id,
                model,
                slug,
                part_number,
                u_height,
                is_full_depth,
                subdevice_role,
                airflow,
                weight,
                weight_unit,
                front_image,
                rear_image,
                description,
                comments,
                default_platform_id,
                console_port_template_count,
                console_server_port_template_count,
                power_port_template_count,
                power_outlet_template_count,
                interface_template_count,
                front_port_template_count,
                rear_port_template_count,
                device_bay_template_count,
                module_bay_template_count,
                inventory_item_template_count,
                exclude_from_utilization,
                device_count,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5,
                $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15,
                $16, $17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, 0,
                $18, $19, $20
            )
            "#,
        )
        .bind(device_type.id)
        .bind(device_type.custom_fields.clone())
        .bind(device_type.manufacturer_id)
        .bind(&device_type.model)
        .bind(&device_type.slug)
        .bind(device_type.part_number.as_deref().unwrap_or(""))
        .bind(device_type.u_height)
        .bind(device_type.is_full_depth)
        .bind(device_type.subdevice_role.as_deref())
        .bind(device_type.airflow.as_deref())
        .bind(device_type.weight)
        .bind(device_type.weight_unit.as_deref())
        .bind(device_type.front_image.as_deref().unwrap_or(""))
        .bind(device_type.rear_image.as_deref().unwrap_or(""))
        .bind(device_type.description.as_deref().unwrap_or(""))
        .bind(device_type.comments.as_deref().unwrap_or(""))
        .bind(device_type.default_platform_id)
        .bind(&device_type.tags)
        .bind(device_type.external_id)
        .bind(device_type.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create device type: {}", e)))?;
        Ok(())
    }

    async fn update(&self, device_type: &DeviceType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_devicetype
            SET manufacturer_id = $2,
                model = $3,
                slug = $4,
                part_number = $5,
                u_height = $6,
                is_full_depth = $7,
                subdevice_role = $8,
                airflow = $9,
                weight = $10,
                weight_unit = $11,
                front_image = $12,
                rear_image = $13,
                description = $14,
                comments = $15,
                default_platform_id = $16,
                tags = $17,
                custom_field_data = COALESCE($18, custom_field_data),
                external_id = COALESCE($19, external_id),
                piping_synced_at = COALESCE($20, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(device_type.id)
        .bind(device_type.manufacturer_id)
        .bind(&device_type.model)
        .bind(&device_type.slug)
        .bind(device_type.part_number.as_deref().unwrap_or(""))
        .bind(device_type.u_height)
        .bind(device_type.is_full_depth)
        .bind(device_type.subdevice_role.as_deref())
        .bind(device_type.airflow.as_deref())
        .bind(device_type.weight)
        .bind(device_type.weight_unit.as_deref())
        .bind(device_type.front_image.as_deref().unwrap_or(""))
        .bind(device_type.rear_image.as_deref().unwrap_or(""))
        .bind(device_type.description.as_deref().unwrap_or(""))
        .bind(device_type.comments.as_deref().unwrap_or(""))
        .bind(device_type.default_platform_id)
        .bind(&device_type.tags)
        .bind(device_type.custom_fields.clone())
        .bind(device_type.external_id)
        .bind(device_type.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update device type: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, device_type_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_devicetype",
            device_type_id,
        )
        .await
        .map(|_| ())
    }
}
