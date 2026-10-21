//! VMInterface repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const VM_INTERFACE_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "virtual_machine_id",
        "virtual_machine_id",
        "virtual_machine_id",
        "virtual_machine_id",
        "virtualMachines",
        "inventory.vmInterfaces.columns.virtualMachine",
    ),
    ListColumn::sortable("name", "name", "inventory.vmInterfaces.columns.name"),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "inventory.vmInterfaces.columns.enabled",
    ),
    ListColumn::sortable("mtu", "mtu", "inventory.vmInterfaces.columns.mtu"),
    ListColumn::enum_filter(
        "mode",
        "mode",
        "mode",
        "inventory.vmInterfaces.columns.mode",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.vmInterfaces.columns.description",
    ),
];

/// Minimal representation of a `virtualization_vminterface` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct VmInterfaceRecord {
    pub id: Uuid,
    pub virtual_machine_id: Uuid,
    pub virtual_machine_name: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub description: Option<String>,
}

/// Input for creating a new `virtualization_vminterface` row.
#[derive(Debug, Clone)]
pub struct NewVmInterface {
    pub id: Uuid,
    pub virtual_machine_id: Uuid,
    pub name: String,
    pub enabled: Option<bool>,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `virtualization_vminterface` row.
#[derive(Debug, Clone)]
pub struct UpdateVmInterface {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub description: Option<String>,
}

/// Multi-value filters for listing `virtualization_vminterface` rows.
#[derive(Debug, Clone, Default)]
pub struct VmInterfaceListFilter {
    pub search: Option<String>,
    pub virtual_machine_id: Vec<Uuid>,
    pub enabled: Vec<String>,
    pub mode: Vec<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkVmInterfaceResult {
    pub created: Vec<VmInterfaceRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VmInterfaceRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VmInterfaceRecord>, AppError>;
    async fn list(
        &self,
        filter: VmInterfaceListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VmInterfaceRecord>, i64), AppError>;
    async fn create(&self, new_iface: &NewVmInterface) -> Result<VmInterfaceRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVmInterface,
    ) -> Result<Option<VmInterfaceRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewVmInterface],
    ) -> Result<BulkVmInterfaceResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVmInterfaceRepository {
    pool: PgPool,
}

impl SqlxVmInterfaceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VmInterfaceRecord, AppError> {
        Ok(VmInterfaceRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read vm_interface id: {}", e))
            })?,
            virtual_machine_id: row.try_get("virtual_machine_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read vm_interface virtual_machine_id: {}",
                    e
                ))
            })?,
            virtual_machine_name: row.try_get("virtual_machine_name").ok(),
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read vm_interface name: {}", e))
            })?,
            enabled: row.try_get("enabled").map_err(|e| {
                AppError::Database(format!("Failed to read vm_interface enabled: {}", e))
            })?,
            mtu: row.try_get::<Option<i32>, _>("mtu").map_err(|e| {
                AppError::Database(format!("Failed to read vminterface mtu: {}", e))
            })?,
            mode: row.try_get::<Option<String>, _>("mode").map_err(|e| {
                AppError::Database(format!("Failed to read vminterface mode: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read vminterface description: {}", e))
                })?,
        })
    }

    fn filter_sql(param: &str, kind: FilterKind) -> Option<&'static str> {
        VM_INTERFACE_COLUMNS.iter().find_map(|column| {
            let filter = column.filter?;
            (filter.param == param && filter.kind == kind).then_some(filter.sql)
        })
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &VmInterfaceListFilter) {
        let has_filter = filter.search.is_some()
            || !filter.virtual_machine_id.is_empty()
            || !filter.enabled.is_empty()
            || !filter.mode.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["iface.name", "iface.mode", "iface.description"],
            filter.search.as_deref(),
        );

        if let Some(sql) = Self::filter_sql("virtual_machine_id", FilterKind::Uuid) {
            push_uuid_in(builder, &mut where_state, sql, &filter.virtual_machine_id);
        }
        if let Some(sql) = Self::filter_sql("enabled", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.enabled);
        }
        if let Some(sql) = Self::filter_sql("mode", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.mode);
        }
    }

    async fn count_filtered(&self, filter: &VmInterfaceListFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM virtualization_vminterface iface");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count vm_interfaces: {}", e)))
    }
}

impl VmInterfaceRepository for SqlxVmInterfaceRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VmInterfaceRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT iface.id,
                   iface.virtual_machine_id,
                   vm.name AS virtual_machine_name,
                   iface.name,
                   iface.enabled,
                   iface.mtu,
                   iface.mode,
                   iface.description
            FROM virtualization_vminterface iface
            LEFT JOIN virtualization_virtualmachine vm
              ON vm.id = iface.virtual_machine_id
            WHERE iface.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vm_interface: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: VmInterfaceListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VmInterfaceRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            "
            SELECT iface.id,
                   iface.virtual_machine_id,
                   vm.name AS virtual_machine_name,
                   iface.name,
                   iface.enabled,
                   iface.mtu,
                   iface.mode,
                   iface.description,
                   COUNT(*) OVER() AS total_count
            FROM virtualization_vminterface iface
            LEFT JOIN virtualization_virtualmachine vm
              ON vm.id = iface.virtual_machine_id
            ",
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(&mut builder, VM_INTERFACE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list vm_interfaces: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read vm_interface total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_iface: &NewVmInterface) -> Result<VmInterfaceRecord, AppError> {
        let enabled = new_iface.enabled.unwrap_or(true);
        let description = new_iface.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO virtualization_vminterface (
                id,
                custom_field_data,
                virtual_machine_id,
                name,
                _name,
                enabled,
                mtu,
                mode,
                description,
                tags
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $3, $4, $5, $6, $7, '{}')
            "#,
        )
        .bind(new_iface.id)
        .bind(new_iface.virtual_machine_id)
        .bind(&new_iface.name)
        .bind(enabled)
        .bind(new_iface.mtu)
        .bind(new_iface.mode.as_deref())
        .bind(&description)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create vm_interface: {}", e)))?;

        self.get_by_id(new_iface.id)
            .await?
            .ok_or_else(|| AppError::Database("Created vm interface disappeared".to_string()))
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVmInterface,
    ) -> Result<Option<VmInterfaceRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let enabled = changes.enabled.unwrap_or(existing.enabled);
        let mtu = if changes.mtu.is_some() {
            changes.mtu
        } else {
            existing.mtu
        };
        let mode = if changes.mode.is_some() {
            changes.mode.clone()
        } else {
            existing.mode
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE virtualization_vminterface
            SET name = $2, _name = $2, enabled = $3, mtu = $4, mode = $5, description = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(enabled)
        .bind(mtu)
        .bind(mode.as_deref())
        .bind(description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update vm_interface: {}", e)))?;

        self.get_by_id(id).await
    }

    async fn bulk_create(
        &self,
        items: &[NewVmInterface],
    ) -> Result<BulkVmInterfaceResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let enabled = item.enabled.unwrap_or(true);
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO virtualization_vminterface (
                    id,
                    custom_field_data,
                    virtual_machine_id,
                    name,
                    _name,
                    enabled,
                    mtu,
                    mode,
                    description,
                    tags
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $3, $4, $5, $6, $7, '{}')
                "#,
                )
                .bind(item.id)
                .bind(item.virtual_machine_id)
                .bind(&item.name)
                .bind(enabled)
                .bind(item.mtu)
                .bind(item.mode.as_deref())
                .bind(&description),
            )
            .await?;

            match result {
                Ok(_) => created.push(VmInterfaceRecord {
                    id: item.id,
                    virtual_machine_id: item.virtual_machine_id,
                    virtual_machine_name: None,
                    name: item.name.clone(),
                    enabled,
                    mtu: item.mtu,
                    mode: item.mode.clone(),
                    description: Some(description),
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert vm_interface '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk vm_interface insert: {}", e))
        })?;

        let mut hydrated = Vec::with_capacity(created.len());
        for record in created {
            let current = self.get_by_id(record.id).await?.ok_or_else(|| {
                AppError::Database("Bulk-created vm interface disappeared".to_string())
            })?;
            hydrated.push(current);
        }

        Ok(BulkVmInterfaceResult {
            created: hydrated,
            errors,
        })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "virtualization_vminterface",
            id,
        )
        .await
        .map(|_| ())
    }
}
