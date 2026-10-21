use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use super::super::super::list_query::{
    ListColumn, push_id_filter, push_order_by, push_string_in, push_uuid_in,
};
use super::super::repository::{
    VirtualMachineFilter, VirtualMachineListProjection, VirtualMachineRepository,
};
use super::super::visibility_sql::{VisibilityColumns, push_visibility_expr};
use crate::contexts::inventory::domain::{VirtualMachine, VirtualMachineListView};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

/// Facet columns of `virtualization_virtualmachine` (aliased `v`) for the
/// row-visibility predicate. VMs expose the same facets as devices.
const VM_VISIBILITY_COLUMNS: VisibilityColumns = VisibilityColumns {
    id: "v.id",
    role_id: "v.role_id",
    site_id: "v.site_id",
    region_id: "(SELECT region_site.region_id FROM dcim_site region_site WHERE region_site.id = v.site_id)",
    tenant_id: "v.tenant_id",
    platform_id: "v.platform_id",
    cluster_id: "v.cluster_id",
    status: "v.status",
};

pub const VIRTUAL_MACHINE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "v.name", "inventory.virtualMachines.columns.name"),
    ListColumn::enum_filter(
        "status",
        "v.status",
        "v.status",
        "inventory.virtualMachines.columns.status",
    ),
    ListColumn::fk_filter(
        "cluster",
        "c.name",
        "cluster_id",
        "v.cluster_id",
        "clusters",
        "inventory.virtualMachines.columns.cluster",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "v.tenant_id",
        "tenants",
        "inventory.virtualMachines.columns.tenant",
    ),
    ListColumn::fk_filter(
        "role",
        "r.name",
        "role_id",
        "v.role_id",
        "device-roles",
        "inventory.virtualMachines.columns.role",
    ),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "v.site_id",
        "sites",
        "inventory.virtualMachines.columns.site",
    ),
    ListColumn::fk_filter(
        "platform",
        "p.name",
        "platform_id",
        "v.platform_id",
        "platforms",
        "inventory.virtualMachines.columns.platform",
    ),
    ListColumn::fk_filter(
        "virtual_machine_type",
        "vt.name",
        "virtual_machine_type_id",
        "v.virtual_machine_type_id",
        "virtual-machine-types",
        "inventory.resources.fields.virtualMachineType",
    ),
    ListColumn::sortable(
        "primary_ip",
        "COALESCE(v.primary_ip4_id::text, v.primary_ip6_id::text)",
        "inventory.virtualMachines.columns.primaryIp",
    ),
    ListColumn::sortable(
        "vcpus",
        "v.vcpus",
        "inventory.virtualMachines.columns.vcpus",
    ),
    ListColumn::sortable(
        "memory",
        "v.memory",
        "inventory.virtualMachines.columns.memory",
    ),
    ListColumn::sortable("disk", "v.disk", "inventory.virtualMachines.columns.disk"),
];

fn push_virtual_machine_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    filter: &VirtualMachineFilter,
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_id_filter(builder, &mut where_state, "v.id", filter.ids.as_deref());
    push_uuid_in(
        builder,
        &mut where_state,
        "v.cluster_id",
        &filter.cluster_id,
    );
    push_uuid_in(builder, &mut where_state, "v.site_id", &filter.site_id);
    push_uuid_in(builder, &mut where_state, "v.tenant_id", &filter.tenant_id);
    push_uuid_in(builder, &mut where_state, "v.role_id", &filter.role_id);
    push_uuid_in(
        builder,
        &mut where_state,
        "v.platform_id",
        &filter.platform_id,
    );
    push_string_in(builder, &mut where_state, "v.status", &filter.status);
    push_uuid_in(
        builder,
        &mut where_state,
        "v.virtual_machine_type_id",
        &filter.virtual_machine_type_id,
    );
    if let Some(search_term) = search {
        where_state.and(builder);
        builder.push("v.name ILIKE ");
        builder.push_bind(search_term.to_owned());
    }
    if let Some(predicate) = filter.visibility.as_ref().filter(|p| !p.is_always()) {
        where_state.and(builder);
        push_visibility_expr(builder, predicate, &VM_VISIBILITY_COLUMNS);
    }
}

#[derive(Clone)]
pub struct SqlxVirtualMachineRepository {
    pool: PgPool,
}

impl SqlxVirtualMachineRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn selected_columns(projection: VirtualMachineListProjection) -> Vec<&'static str> {
        let mut columns = vec![
            "id",
            "external_id",
            "name",
            "cluster_id",
            "site_id",
            "tenant_id",
            "role_id",
            "platform_id",
            "virtual_machine_type_id",
            // NUMERIC does not decode into f64 — without this cast the read
            // fails (and used to silently decay to None under `.ok()`).
            "vcpus::float8 AS vcpus",
            "memory",
            "disk",
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
        projection: VirtualMachineListProjection,
    ) {
        builder.push("SELECT ");
        builder.push(Self::selected_columns(projection).join(", "));
        builder.push(" FROM virtualization_virtualmachine");
    }

    /// The projection every fixed-SELECT caller of `parse_vm_row` uses: all
    /// three optional columns present.
    const FULL_PROJECTION: VirtualMachineListProjection = VirtualMachineListProjection {
        include_status: true,
        include_primary_ip4_id: true,
        include_primary_ip6_id: true,
    };

    /// Parse a VM row against the projection that produced it — the same
    /// contract as `parse_device_row`: optional columns are skipped by
    /// construction when unselected, everything selected reads through `col`
    /// and fails with the column name instead of decaying to `None`.
    fn parse_vm_row(
        row: &PgRow,
        projection: VirtualMachineListProjection,
    ) -> Result<VirtualMachine, AppError> {
        Ok(VirtualMachine {
            id: col(row, "id")?,
            external_id: col(row, "external_id")?,
            name: col(row, "name")?,
            cluster_id: col(row, "cluster_id")?,
            site_id: col(row, "site_id")?,
            tenant_id: col(row, "tenant_id")?,
            role_id: col(row, "role_id")?,
            platform_id: col(row, "platform_id")?,
            virtual_machine_type_id: col(row, "virtual_machine_type_id")?,
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
            vcpus: col(row, "vcpus")?,
            memory: col(row, "memory")?,
            disk: col(row, "disk")?,
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

impl VirtualMachineRepository for SqlxVirtualMachineRepository {
    async fn get_by_id(&self, vm_id: Uuid) -> Result<Option<VirtualMachine>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, cluster_id, site_id, tenant_id, role_id, status,
                   primary_ip4_id, primary_ip6_id, platform_id, virtual_machine_type_id,
                   vcpus::float8 AS vcpus, memory, disk,
                   description, comments, config_template_id, local_context_data, tags,
                   custom_field_data, piping_synced_at
            FROM virtualization_virtualmachine
            WHERE id = $1
            "#,
        )
        .bind(vm_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load virtual machine: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_vm_row(&row, Self::FULL_PROJECTION)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<VirtualMachine>, AppError> {
        self.list_paginated_projected(
            VirtualMachineListProjection {
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
        projection: VirtualMachineListProjection,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<VirtualMachine>, AppError> {
        let resolved_limit = clamp_limit(limit);
        let resolved_offset = clamp_offset(offset);
        tracing::debug!(
            operation = "inventory.virtual_machines.list_paginated_projected",
            columns = %Self::selected_columns(projection).join(","),
            limit = resolved_limit,
            offset = resolved_offset,
            "Inventory virtual machine list projection prepared"
        );

        let mut builder = QueryBuilder::<Postgres>::new("");
        Self::push_select_columns(&mut builder, projection);
        builder.push(" ORDER BY name");
        builder.push(" LIMIT ");
        builder.push_bind(resolved_limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(resolved_offset as i64);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list virtual machines: {}", e))
            })?;

        rows.iter()
            .map(|row| Self::parse_vm_row(row, projection))
            .collect::<Result<Vec<VirtualMachine>, AppError>>()
    }

    async fn list_hydrated(
        &self,
        filter: VirtualMachineFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<VirtualMachineListView>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT v.id, v.name, v.status,
                   v.site_id, s.name AS site_name,
                   v.cluster_id, c.name AS cluster_name,
                   v.tenant_id, t.name AS tenant_name,
                   v.role_id, r.name AS role_name,
                   v.platform_id, p.name AS platform_name,
                   v.virtual_machine_type_id, vt.name AS virtual_machine_type_name,
                   v.primary_ip4_id, v.primary_ip6_id,
                   v.vcpus::float8 AS vcpus, v.memory, v.disk,
                   COUNT(*) OVER() AS total_count
            FROM virtualization_virtualmachine v
            LEFT JOIN dcim_site s ON s.id = v.site_id
            LEFT JOIN virtualization_cluster c ON c.id = v.cluster_id
            LEFT JOIN tenancy_tenant t ON t.id = v.tenant_id
            LEFT JOIN dcim_devicerole r ON r.id = v.role_id
            LEFT JOIN dcim_platform p ON p.id = v.platform_id
            LEFT JOIN virtualization_virtualmachinetype vt ON vt.id = v.virtual_machine_type_id
            "#,
        );

        push_virtual_machine_filters(&mut builder, &filter, search);

        push_order_by(
            &mut builder,
            VIRTUAL_MACHINE_COLUMNS,
            filter.sort.as_ref(),
            "v.name ASC, v.id",
        );
        if filter.sort.is_some() {
            builder.push(", v.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list virtual machines hydrated: {}", e))
        })?;

        // `unwrap_or(0)` on the window count would report an empty first page
        // on a decode error instead of failing; read it like every other column.
        let total: i64 = match rows.first() {
            Some(row) => col(row, "total_count")?,
            None => {
                let mut count = QueryBuilder::<Postgres>::new(
                    "SELECT COUNT(*) FROM virtualization_virtualmachine v",
                );
                push_virtual_machine_filters(&mut count, &filter, search);
                count
                    .build_query_scalar::<i64>()
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| {
                        AppError::Database(format!(
                            "Failed to count virtual machines hydrated: {e}"
                        ))
                    })?
            }
        };

        let items = rows
            .iter()
            .map(|row| {
                Ok(VirtualMachineListView {
                    id: col(row, "id")?,
                    name: col(row, "name")?,
                    status: col(row, "status")?,
                    site_id: col(row, "site_id")?,
                    site_name: col(row, "site_name")?,
                    cluster_id: col(row, "cluster_id")?,
                    cluster_name: col(row, "cluster_name")?,
                    tenant_id: col(row, "tenant_id")?,
                    tenant_name: col(row, "tenant_name")?,
                    role_id: col(row, "role_id")?,
                    role_name: col(row, "role_name")?,
                    platform_id: col(row, "platform_id")?,
                    platform_name: col(row, "platform_name")?,
                    virtual_machine_type_id: col(row, "virtual_machine_type_id")?,
                    virtual_machine_type_name: col(row, "virtual_machine_type_name")?,
                    primary_ip4_id: col(row, "primary_ip4_id")?,
                    primary_ip6_id: col(row, "primary_ip6_id")?,
                    vcpus: col(row, "vcpus")?,
                    memory: col(row, "memory")?,
                    disk: col(row, "disk")?,
                })
            })
            .collect::<Result<Vec<VirtualMachineListView>, AppError>>()?;

        Ok((items, total))
    }

    async fn create(&self, vm: &VirtualMachine) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO virtualization_virtualmachine (
                id,
                custom_field_data,
                name,
                status,
                comments,
                description,
                interface_count,
                virtual_disk_count,
                serial,
                start_on_boot,
                cluster_id,
                tenant_id,
                role_id,
                site_id,
                platform_id,
                primary_ip4_id,
                primary_ip6_id,
                vcpus,
                memory,
                disk,
                config_template_id,
                local_context_data,
                tags,
                external_id,
                piping_synced_at,
                virtual_machine_type_id
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6,
                0, 0, '', 'false',
                $7, $8, $9, $10, $11, $12, $13,
                $14, $15, $16, $17, $18, $19, $20, $21, $22
            )
            "#,
        )
        .bind(vm.id)
        .bind(vm.custom_fields.clone())
        .bind(&vm.name)
        .bind(vm.status.as_deref().unwrap_or("active"))
        .bind(vm.comments.as_deref().unwrap_or(""))
        .bind(vm.description.as_deref().unwrap_or(""))
        .bind(vm.cluster_id)
        .bind(vm.tenant_id)
        .bind(vm.role_id)
        .bind(vm.site_id)
        .bind(vm.platform_id)
        .bind(vm.primary_ip4_id)
        .bind(vm.primary_ip6_id)
        .bind(vm.vcpus)
        .bind(vm.memory)
        .bind(vm.disk)
        .bind(vm.config_template_id)
        .bind(vm.local_context_data.clone())
        .bind(&vm.tags)
        .bind(vm.external_id)
        .bind(vm.piping_synced_at)
        .bind(vm.virtual_machine_type_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create virtual machine: {}", e)))?;
        Ok(())
    }

    async fn update(&self, vm: &VirtualMachine) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE virtualization_virtualmachine
            SET name = $2,
                cluster_id = $3,
                tenant_id = $4,
                role_id = $5,
                site_id = $6,
                status = $7,
                primary_ip4_id = $8,
                primary_ip6_id = $9,
                platform_id = $10,
                vcpus = $11,
                memory = $12,
                disk = $13,
                description = $14,
                comments = $15,
                config_template_id = $16,
                local_context_data = $17,
                tags = $18,
                custom_field_data = COALESCE($19, custom_field_data),
                external_id = COALESCE($20, external_id),
                piping_synced_at = COALESCE($21, piping_synced_at),
                virtual_machine_type_id = $22
            WHERE id = $1
            "#,
        )
        .bind(vm.id)
        .bind(&vm.name)
        .bind(vm.cluster_id)
        .bind(vm.tenant_id)
        .bind(vm.role_id)
        .bind(vm.site_id)
        .bind(vm.status.as_deref().unwrap_or("active"))
        .bind(vm.primary_ip4_id)
        .bind(vm.primary_ip6_id)
        .bind(vm.platform_id)
        .bind(vm.vcpus)
        .bind(vm.memory)
        .bind(vm.disk)
        .bind(vm.description.as_deref().unwrap_or(""))
        .bind(vm.comments.as_deref().unwrap_or(""))
        .bind(vm.config_template_id)
        .bind(vm.local_context_data.clone())
        .bind(&vm.tags)
        .bind(vm.custom_fields.clone())
        .bind(vm.external_id)
        .bind(vm.piping_synced_at)
        .bind(vm.virtual_machine_type_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update virtual machine: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, vm_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "virtualization_virtualmachine",
            vm_id,
        )
        .await
        .map(|_| ())
    }
}
