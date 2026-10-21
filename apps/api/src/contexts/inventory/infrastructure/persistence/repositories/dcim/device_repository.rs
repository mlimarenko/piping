use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use super::super::super::list_query::{
    ListColumn, push_custom_field_filters, push_id_filter, push_order_by, push_string_in,
    push_uuid_in,
};
use super::super::repository::{DeviceFilter, DeviceListProjection, DeviceRepository};
use super::super::visibility_sql::{VisibilityColumns, push_visibility_expr};
use crate::contexts::inventory::domain::{Device, DeviceListView};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

/// Facet columns of `dcim_device` (aliased `d`) for the row-visibility predicate.
const DEVICE_VISIBILITY_COLUMNS: VisibilityColumns = VisibilityColumns {
    id: "d.id",
    role_id: "d.role_id",
    site_id: "d.site_id",
    region_id: "(SELECT region_site.region_id FROM dcim_site region_site WHERE region_site.id = d.site_id)",
    tenant_id: "d.tenant_id",
    platform_id: "d.platform_id",
    cluster_id: "d.cluster_id",
    status: "d.status",
};

pub const DEVICE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "d.name", "inventory.devices.columns.name"),
    ListColumn::enum_filter(
        "status",
        "d.status",
        "d.status",
        "inventory.devices.columns.status",
    ),
    ListColumn::sortable("serial", "d.serial", "inventory.devices.columns.serial"),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "d.site_id",
        "sites",
        "inventory.devices.columns.site",
    ),
    ListColumn::fk_filter(
        "role",
        "r.name",
        "role_id",
        "d.role_id",
        "device-roles",
        "inventory.devices.columns.role",
    ),
    ListColumn::fk_filter(
        "device_type",
        "dt.model",
        "device_type_id",
        "d.device_type_id",
        "device-types",
        "inventory.devices.columns.deviceType",
    ),
    ListColumn::fk_filter(
        "platform",
        "pl.name",
        "platform_id",
        "d.platform_id",
        "platforms",
        "inventory.devices.columns.platform",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "d.tenant_id",
        "tenants",
        "inventory.devices.columns.tenant",
    ),
    ListColumn::fk_filter(
        "rack",
        "rack.name",
        "rack_id",
        "d.rack_id",
        "racks",
        "inventory.devices.columns.rack",
    ),
    ListColumn::sortable(
        "primary_ip",
        "COALESCE(d.primary_ip4_id::text, d.primary_ip6_id::text)",
        "inventory.devices.columns.primaryIp",
    ),
    ListColumn::sortable(
        "asset_tag",
        "d.asset_tag",
        "inventory.devices.columns.assetTag",
    ),
    ListColumn::sortable(
        "position",
        "d.position",
        "inventory.devices.columns.position",
    ),
    ListColumn::sortable("face", "d.face", "inventory.devices.columns.face"),
    ListColumn::sortable("airflow", "d.airflow", "inventory.devices.columns.airflow"),
];

#[derive(Clone)]
pub struct SqlxDeviceRepository {
    pool: PgPool,
}

impl SqlxDeviceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Build the REST-facing update without overwriting collected facts.
    ///
    /// `DevicePayload` can update every column emitted below (nullable payload
    /// fields intentionally keep their existing PUT semantics, including
    /// clearing a value). Inventory-only facts absent from that payload are not
    /// emitted at all, so editing a device cannot erase them. Primary IP is
    /// likewise omitted because `IpamService::set_primary_ip_for_device` owns
    /// that update.
    fn build_update_query(device: &Device) -> Result<QueryBuilder<'static, Postgres>, AppError> {
        let site_id = device
            .site_id
            .ok_or_else(|| AppError::Validation("Site is required for device".to_string()))?;
        let role_id = device
            .role_id
            .ok_or_else(|| AppError::Validation("Device role is required".to_string()))?;
        let device_type_id = device
            .device_type_id
            .ok_or_else(|| AppError::Validation("Device type is required".to_string()))?;

        let mut builder = QueryBuilder::<Postgres>::new("UPDATE dcim_device SET name = ");
        builder.push_bind(device.name.clone());
        builder.push(", site_id = ");
        builder.push_bind(site_id);
        builder.push(", tenant_id = ");
        builder.push_bind(device.tenant_id);
        builder.push(", role_id = ");
        builder.push_bind(role_id);
        builder.push(", device_type_id = ");
        builder.push_bind(device_type_id);
        builder.push(", status = ");
        builder.push_bind(
            device
                .status
                .clone()
                .unwrap_or_else(|| "active".to_string()),
        );
        builder.push(", platform_id = ");
        builder.push_bind(device.platform_id);
        builder.push(", rack_id = ");
        builder.push_bind(device.rack_id);
        builder.push(", serial = ");
        builder.push_bind(device.serial.clone().unwrap_or_default());
        builder.push(", asset_tag = ");
        builder.push_bind(device.asset_tag.clone());
        builder.push(", \"position\" = ");
        builder.push_bind(device.position);
        builder.push(", face = ");
        builder.push_bind(device.face.clone());
        builder.push(", airflow = ");
        builder.push_bind(device.airflow.clone());
        builder.push(", description = ");
        builder.push_bind(device.description.clone().unwrap_or_default());
        builder.push(", comments = ");
        builder.push_bind(device.comments.clone().unwrap_or_default());
        builder.push(", local_context_data = ");
        builder.push_bind(device.local_context_data.clone());
        builder.push(", custom_field_data = COALESCE(");
        builder.push_bind(device.custom_fields.clone());
        builder.push(", custom_field_data)");

        builder.push(" WHERE id = ");
        builder.push_bind(device.id);
        Ok(builder)
    }

    fn selected_columns(projection: DeviceListProjection) -> Vec<&'static str> {
        let mut columns = vec![
            "id",
            "external_id",
            "name",
            "site_id",
            "tenant_id",
            "role_id",
            "device_type_id",
            "platform_id",
            "rack_id",
            "cluster_id",
            "virtual_chassis_id",
            "serial",
            "asset_tag",
            // NUMERIC does not decode into f64 — without this cast the read
            // fails (and used to silently decay to None under `.ok()`).
            "\"position\"::float8 AS \"position\"",
            "face",
            "vc_position",
            "vc_priority",
            "airflow",
            "description",
            "comments",
            "config_template_id",
            "local_context_data",
            "tags",
            "custom_field_data",
            "piping_synced_at",
        ];
        if projection.include_status {
            columns.push("status");
        }
        if projection.include_primary_ip4_id {
            columns.push("primary_ip4_id");
        }
        if projection.include_primary_ip6_id {
            columns.push("primary_ip6_id");
        }
        columns
    }

    fn push_select_columns(
        builder: &mut QueryBuilder<'_, Postgres>,
        projection: DeviceListProjection,
    ) {
        builder.push("SELECT ");
        builder.push(Self::selected_columns(projection).join(", "));
        builder.push(" FROM dcim_device");
    }

    /// The projection every fixed-SELECT caller of `parse_device_row` uses:
    /// all three optional columns present.
    const FULL_PROJECTION: DeviceListProjection = DeviceListProjection {
        include_status: true,
        include_primary_ip4_id: true,
        include_primary_ip6_id: true,
    };

    /// Parse a device row against the projection that produced it.
    ///
    /// `status` / `primary_ip4_id` / `primary_ip6_id` are only selected when
    /// the projection asks for them, so their absence is a stated fact the
    /// parser handles by construction — not a swallowed decode error. Every
    /// column the projection did select reads through `col`, which fails with
    /// the column name on a mismatch instead of decaying to `None`.
    fn parse_device_row(row: &PgRow, projection: DeviceListProjection) -> Result<Device, AppError> {
        let id: Uuid = col(row, "id")?;
        let name: Option<String> = col(row, "name")?;
        Ok(Device {
            id,
            external_id: col(row, "external_id")?,
            name: name.unwrap_or_else(|| id.to_string()),
            site_id: Some(col(row, "site_id")?),
            tenant_id: col(row, "tenant_id")?,
            role_id: Some(col(row, "role_id")?),
            device_type_id: Some(col(row, "device_type_id")?),
            status: if projection.include_status {
                col(row, "status")?
            } else {
                None
            },
            primary_ip4_id: if projection.include_primary_ip4_id {
                col(row, "primary_ip4_id")?
            } else {
                None
            },
            primary_ip6_id: if projection.include_primary_ip6_id {
                col(row, "primary_ip6_id")?
            } else {
                None
            },
            platform_id: col(row, "platform_id")?,
            rack_id: col(row, "rack_id")?,
            cluster_id: col(row, "cluster_id")?,
            virtual_chassis_id: col(row, "virtual_chassis_id")?,
            serial: col(row, "serial")?,
            asset_tag: col(row, "asset_tag")?,
            position: col(row, "position")?,
            face: col(row, "face")?,
            vc_position: col(row, "vc_position")?,
            vc_priority: col(row, "vc_priority")?,
            airflow: col(row, "airflow")?,
            description: col(row, "description")?,
            comments: col(row, "comments")?,
            config_template_id: col(row, "config_template_id")?,
            local_context_data: col(row, "local_context_data")?,
            tags: col(row, "tags")?,
            custom_fields: col(row, "custom_field_data")?,
            piping_synced_at: col(row, "piping_synced_at")?,
            piping_connection_profile_ids: Vec::new(),
        })
    }
}

impl DeviceRepository for SqlxDeviceRepository {
    async fn get_by_id(&self, device_id: Uuid) -> Result<Option<Device>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, site_id, tenant_id, role_id, device_type_id, status,
                   primary_ip4_id, primary_ip6_id, platform_id, rack_id, cluster_id,
                   virtual_chassis_id, serial, asset_tag, "position"::float8 AS "position", face,
                   vc_position, vc_priority, airflow, description, comments,
                   config_template_id, local_context_data, tags, custom_field_data,
                   piping_synced_at
            FROM dcim_device
            WHERE id = $1
            "#,
        )
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load device: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_device_row(&row, Self::FULL_PROJECTION)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        filter: DeviceFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Device>, AppError> {
        self.list_paginated_projected(
            filter,
            DeviceListProjection {
                include_status: true,
                include_primary_ip4_id: true,
                include_primary_ip6_id: true,
            },
            limit,
            offset,
        )
        .await
    }

    async fn list_paginated_projected(
        &self,
        filter: DeviceFilter,
        projection: DeviceListProjection,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Device>, AppError> {
        let resolved_limit = clamp_limit(limit);
        let resolved_offset = clamp_offset(offset);
        let selected_columns = Self::selected_columns(projection);
        tracing::debug!(
            operation = "inventory.devices.list_paginated_projected",
            columns = %selected_columns.join(","),
            filter_tenant = !filter.tenant_id.is_empty(),
            filter_site = !filter.site_id.is_empty(),
            filter_region = filter.region_id.is_some(),
            filter_role = !filter.role_id.is_empty(),
            filter_cluster = !filter.cluster_id.is_empty(),
            filter_rack = !filter.rack_id.is_empty(),
            filter_location = !filter.location_id.is_empty(),
            filter_status = !filter.status.is_empty(),
            limit = resolved_limit,
            offset = resolved_offset,
            "Inventory device list projection prepared"
        );

        let mut builder = QueryBuilder::<Postgres>::new("");
        Self::push_select_columns(&mut builder, projection);
        let has_filter = filter.ids.is_some()
            || !filter.tenant_id.is_empty()
            || !filter.site_id.is_empty()
            || filter.region_id.is_some()
            || !filter.role_id.is_empty()
            || !filter.cluster_id.is_empty()
            || !filter.rack_id.is_empty()
            || !filter.location_id.is_empty()
            || !filter.status.is_empty()
            || !filter.custom_fields.is_empty();
        if has_filter {
            builder.push(" WHERE ");
            let mut where_state = WhereState::after_where();

            push_id_filter(&mut builder, &mut where_state, "id", filter.ids.as_deref());

            push_uuid_in(
                &mut builder,
                &mut where_state,
                "tenant_id",
                &filter.tenant_id,
            );
            push_uuid_in(&mut builder, &mut where_state, "site_id", &filter.site_id);

            if let Some(region_id) = filter.region_id {
                where_state.and(&mut builder);
                builder.push("site_id IN (SELECT id FROM dcim_site WHERE region_id = ");
                builder.push_bind(region_id);
                builder.push(")");
            }

            push_uuid_in(&mut builder, &mut where_state, "role_id", &filter.role_id);
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "cluster_id",
                &filter.cluster_id,
            );
            push_uuid_in(&mut builder, &mut where_state, "rack_id", &filter.rack_id);
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "location_id",
                &filter.location_id,
            );
            push_string_in(&mut builder, &mut where_state, "status", &filter.status);
            push_custom_field_filters(
                &mut builder,
                &mut where_state,
                "custom_field_data",
                "id",
                &filter.custom_fields,
            );
        }

        builder.push(" ORDER BY name, id");
        builder.push(" LIMIT ");
        builder.push_bind(resolved_limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(resolved_offset as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list devices: {}", e)))?;

        rows.iter()
            .map(|row| Self::parse_device_row(row, projection))
            .collect::<Result<Vec<Device>, AppError>>()
    }

    async fn list_hydrated(
        &self,
        filter: DeviceFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<DeviceListView>, i64), AppError> {
        let resolved_limit = clamp_limit(limit) as i64;
        let resolved_offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT d.id, d.name, d.status,
                   d.site_id, s.name AS site_name,
                   d.tenant_id, t.name AS tenant_name,
                   d.role_id, r.name AS role_name,
                   d.device_type_id, dt.model AS device_type_name,
                   d.platform_id, pl.name AS platform_name,
                   d.rack_id, rack.name AS rack_name,
                   d.primary_ip4_id, d.primary_ip6_id,
                   d.serial, d.asset_tag, d.position::float8 AS position,
                   d.face, d.airflow, d.custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_device d
            LEFT JOIN dcim_site s ON s.id = d.site_id
            LEFT JOIN tenancy_tenant t ON t.id = d.tenant_id
            LEFT JOIN dcim_devicerole r ON r.id = d.role_id
            LEFT JOIN dcim_devicetype dt ON dt.id = d.device_type_id
            LEFT JOIN dcim_platform pl ON pl.id = d.platform_id
            LEFT JOIN dcim_rack rack ON rack.id = d.rack_id
            "#,
        );

        push_device_list_filters(&mut builder, &filter, search);

        push_order_by(
            &mut builder,
            DEVICE_COLUMNS,
            filter.sort.as_ref(),
            "d.name ASC, d.id",
        );
        if filter.sort.is_some() {
            builder.push(", d.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(resolved_limit);
        builder.push(" OFFSET ");
        builder.push_bind(resolved_offset);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list devices hydrated: {}", e))
            })?;

        // `unwrap_or(0)` on the window count would report an empty first page
        // on a decode error instead of failing; read it like every other column.
        let total: i64 = match rows.first() {
            Some(row) => col(row, "total_count")?,
            None => 0,
        };

        let items = rows
            .iter()
            .map(|row| {
                let name: Option<String> = col(row, "name")?;
                Ok(DeviceListView {
                    id: col(row, "id")?,
                    // NULL names stay empty in the list view — the id fallback
                    // is parse_device_row's policy, not this projection's.
                    name: name.unwrap_or_default(),
                    status: col(row, "status")?,
                    site_id: col(row, "site_id")?,
                    site_name: col(row, "site_name")?,
                    tenant_id: col(row, "tenant_id")?,
                    tenant_name: col(row, "tenant_name")?,
                    role_id: col(row, "role_id")?,
                    role_name: col(row, "role_name")?,
                    device_type_id: col(row, "device_type_id")?,
                    device_type_name: col(row, "device_type_name")?,
                    platform_id: col(row, "platform_id")?,
                    platform_name: col(row, "platform_name")?,
                    rack_id: col(row, "rack_id")?,
                    rack_name: col(row, "rack_name")?,
                    primary_ip4_id: col(row, "primary_ip4_id")?,
                    primary_ip6_id: col(row, "primary_ip6_id")?,
                    serial: col(row, "serial")?,
                    asset_tag: col(row, "asset_tag")?,
                    position: col(row, "position")?,
                    face: col(row, "face")?,
                    airflow: col(row, "airflow")?,
                    custom_fields: col(row, "custom_field_data")?,
                })
            })
            .collect::<Result<Vec<DeviceListView>, AppError>>()?;

        Ok((items, total))
    }

    async fn create(&self, device: &Device) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_device (
                id,
                custom_field_data,
                name,
                serial,
                asset_tag,
                "position",
                face,
                status,
                vc_position,
                vc_priority,
                comments,
                cluster_id,
                role_id,
                device_type_id,
                platform_id,
                primary_ip4_id,
                primary_ip6_id,
                rack_id,
                site_id,
                tenant_id,
                virtual_chassis_id,
                airflow,
                description,
                config_template_id,
                local_context_data,
                console_port_count,
                console_server_port_count,
                power_port_count,
                power_outlet_count,
                interface_count,
                front_port_count,
                rear_port_count,
                device_bay_count,
                module_bay_count,
                inventory_item_count,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                $21, $22, $23, $24, $25,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                $26, $27, $28
            )
            "#,
        )
        .bind(device.id)
        .bind(device.custom_fields.clone())
        .bind(&device.name)
        .bind(device.serial.as_deref().unwrap_or(""))
        .bind(device.asset_tag.as_deref())
        .bind(device.position)
        .bind(device.face.as_deref())
        .bind(device.status.as_deref().unwrap_or("active"))
        .bind(device.vc_position)
        .bind(device.vc_priority)
        .bind(device.comments.as_deref().unwrap_or(""))
        .bind(device.cluster_id)
        .bind(
            device
                .role_id
                .ok_or_else(|| AppError::Validation("Device role is required".to_string()))?,
        )
        .bind(
            device
                .device_type_id
                .ok_or_else(|| AppError::Validation("Device type is required".to_string()))?,
        )
        .bind(device.platform_id)
        .bind(device.primary_ip4_id)
        .bind(device.primary_ip6_id)
        .bind(device.rack_id)
        .bind(
            device
                .site_id
                .ok_or_else(|| AppError::Validation("Site is required for device".to_string()))?,
        )
        .bind(device.tenant_id)
        .bind(device.virtual_chassis_id)
        .bind(device.airflow.as_deref())
        .bind(device.description.as_deref().unwrap_or(""))
        .bind(device.config_template_id)
        .bind(device.local_context_data.clone())
        .bind(&device.tags)
        .bind(device.external_id)
        .bind(device.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create device: {}", e)))?;
        Ok(())
    }

    async fn update(&self, device: &Device) -> Result<(), AppError> {
        let mut query = Self::build_update_query(device)?;
        query
            .build()
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to update device: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, device_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_device", device_id)
            .await
            .map(|_| ())
    }
}

/// Append the `WHERE` clause for [`DeviceRepository::list_hydrated`] to `builder`.
///
/// No-op when no facet, search term, or visibility predicate is active.
fn push_device_list_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    filter: &DeviceFilter,
    search: Option<&str>,
) {
    let has_visibility = filter
        .visibility
        .as_ref()
        .is_some_and(|predicate| !predicate.is_always());
    let has_filter = filter.ids.is_some()
        || !filter.tenant_id.is_empty()
        || !filter.site_id.is_empty()
        || filter.region_id.is_some()
        || !filter.role_id.is_empty()
        || !filter.device_type_id.is_empty()
        || !filter.platform_id.is_empty()
        || !filter.cluster_id.is_empty()
        || !filter.rack_id.is_empty()
        || !filter.location_id.is_empty()
        || !filter.status.is_empty()
        || !filter.custom_fields.is_empty()
        || search.is_some()
        || has_visibility;
    if !has_filter {
        return;
    }

    builder.push(" WHERE ");
    let mut where_state = WhereState::after_where();
    push_id_filter(builder, &mut where_state, "d.id", filter.ids.as_deref());
    push_uuid_in(builder, &mut where_state, "d.tenant_id", &filter.tenant_id);
    push_uuid_in(builder, &mut where_state, "d.site_id", &filter.site_id);
    if let Some(region_id) = filter.region_id {
        where_state.and(builder);
        builder.push("d.site_id IN (SELECT id FROM dcim_site WHERE region_id = ");
        builder.push_bind(region_id);
        builder.push(")");
    }
    push_uuid_in(builder, &mut where_state, "d.role_id", &filter.role_id);
    push_uuid_in(
        builder,
        &mut where_state,
        "d.device_type_id",
        &filter.device_type_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "d.platform_id",
        &filter.platform_id,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "d.cluster_id",
        &filter.cluster_id,
    );
    push_uuid_in(builder, &mut where_state, "d.rack_id", &filter.rack_id);
    push_uuid_in(
        builder,
        &mut where_state,
        "d.location_id",
        &filter.location_id,
    );
    push_string_in(builder, &mut where_state, "d.status", &filter.status);
    push_custom_field_filters(
        builder,
        &mut where_state,
        "d.custom_field_data",
        "d.id",
        &filter.custom_fields,
    );
    if let Some(search_term) = search {
        where_state.and(builder);
        builder.push("d.name ILIKE ");
        builder.push_bind(search_term.to_owned());
    }
    // Row-level visibility predicate. `COUNT(*) OVER()` above
    // sees this WHERE, so total stays consistent with the returned rows.
    if has_visibility && let Some(predicate) = filter.visibility.as_ref() {
        where_state.and(builder);
        push_visibility_expr(builder, predicate, &DEVICE_VISIBILITY_COLUMNS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_sql_preserves_collected_facts_and_updates_payload_fields() {
        let update = Device {
            id: Uuid::from_u128(1),
            name: "renamed-device".to_string(),
            site_id: Some(Uuid::from_u128(2)),
            role_id: Some(Uuid::from_u128(3)),
            device_type_id: Some(Uuid::from_u128(4)),
            ..Device::default()
        };

        let builder = SqlxDeviceRepository::build_update_query(&update).expect("build update");
        let sql = builder.sql();

        for payload_column in [
            "name = ",
            "site_id = ",
            "tenant_id = ",
            "role_id = ",
            "device_type_id = ",
            "status = ",
            "platform_id = ",
            "rack_id = ",
            "serial = ",
            "asset_tag = ",
            "\"position\" = ",
            "face = ",
            "airflow = ",
            "description = ",
            "comments = ",
            "local_context_data = ",
            "custom_field_data = COALESCE(",
        ] {
            assert!(
                sql.contains(payload_column),
                "missing payload assignment `{payload_column}`: {sql}"
            );
        }
        for collected_fact in [
            "primary_ip4_id = ",
            "primary_ip6_id = ",
            "cluster_id = ",
            "virtual_chassis_id = ",
            "vc_position = ",
            "vc_priority = ",
            "config_template_id = ",
            "tags = ",
            "external_id = ",
            "piping_synced_at = ",
        ] {
            assert!(
                !sql.contains(collected_fact),
                "collected fact produced assignment `{collected_fact}`: {sql}"
            );
        }
        assert!(
            !sql.contains("renamed-device"),
            "values must stay bound: {sql}"
        );
    }
}
