use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const INTERFACE_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "device_id",
        "device_id",
        "device_id",
        "device_id",
        "devices",
        "inventory.interfaces.columns.device",
    ),
    ListColumn::sortable("name", "name", "inventory.interfaces.columns.name"),
    ListColumn::sortable("label", "label", "inventory.interfaces.columns.label"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.interfaces.columns.description",
    ),
    ListColumn::enum_filter("type", "type", "type", "inventory.interfaces.columns.type"),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "inventory.interfaces.columns.enabled",
    ),
    ListColumn::enum_filter(
        "mgmt_only",
        "mgmt_only",
        "mgmt_only::text",
        "inventory.interfaces.columns.mgmtOnly",
    ),
];

/// Minimal representation of a `dcim_interface` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct InterfaceRecord {
    pub id: Uuid,
    pub device_id: Option<Uuid>,
    pub device_name: Option<String>,
    pub name: String,
    pub label: String,
    pub description: String,
    pub type_: String,
    pub enabled: bool,
    pub mgmt_only: bool,
}

/// Input for creating a minimal `dcim_interface` row.
#[derive(Debug, Clone)]
pub struct NewInterface {
    pub id: Uuid,
    pub device_id: Uuid,
    pub name: String,
    pub type_: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
}

/// Fields that may be updated on an existing `dcim_interface` row.
#[derive(Debug, Clone)]
pub struct UpdateInterface {
    pub name: Option<String>,
    pub type_: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mgmt_only: Option<bool>,
}

/// Multi-value filters for listing `dcim_interface` rows.
#[derive(Debug, Clone, Default)]
pub struct InterfaceListFilter {
    pub search: Option<String>,
    pub device_id: Vec<Uuid>,
    pub type_: Vec<String>,
    pub enabled: Vec<String>,
    pub mgmt_only: Vec<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkInterfaceResult {
    pub created: Vec<InterfaceRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait InterfaceRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InterfaceRecord>, AppError>;
    async fn list(
        &self,
        filter: InterfaceListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<InterfaceRecord>, i64), AppError>;
    async fn list_by_device(
        &self,
        device_id: Uuid,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<InterfaceRecord>, AppError>;
    async fn create(&self, new_interface: &NewInterface) -> Result<InterfaceRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInterface,
    ) -> Result<Option<InterfaceRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewInterface]) -> Result<BulkInterfaceResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxInterfaceRepository {
    pool: PgPool,
}

impl SqlxInterfaceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<InterfaceRecord, AppError> {
        Ok(InterfaceRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read interface id: {}", e)))?,
            device_id: row.try_get("device_id").ok(),
            device_name: row.try_get("device_name").ok(),
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read interface name: {}", e)))?,
            label: row.try_get("label").map_err(|e| {
                AppError::Database(format!("Failed to read interface label: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read interface description: {}", e))
            })?,
            type_: row
                .try_get("type")
                .map_err(|e| AppError::Database(format!("Failed to read interface type: {}", e)))?,
            enabled: row.try_get("enabled").map_err(|e| {
                AppError::Database(format!("Failed to read interface enabled: {}", e))
            })?,
            mgmt_only: row.try_get("mgmt_only").map_err(|e| {
                AppError::Database(format!("Failed to read interface mgmt_only: {}", e))
            })?,
        })
    }

    fn filter_sql(param: &str, kind: FilterKind) -> Option<&'static str> {
        INTERFACE_COLUMNS.iter().find_map(|column| {
            let filter = column.filter?;
            (filter.param == param && filter.kind == kind).then_some(filter.sql)
        })
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &InterfaceListFilter) {
        let has_filter = filter.search.is_some()
            || !filter.device_id.is_empty()
            || !filter.type_.is_empty()
            || !filter.enabled.is_empty()
            || !filter.mgmt_only.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "iface.name",
                "iface.label",
                "iface.description",
                "iface.type",
            ],
            filter.search.as_deref(),
        );

        if let Some(sql) = Self::filter_sql("device_id", FilterKind::Uuid) {
            push_uuid_in(builder, &mut where_state, sql, &filter.device_id);
        }
        if let Some(sql) = Self::filter_sql("type", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.type_);
        }
        if let Some(sql) = Self::filter_sql("enabled", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.enabled);
        }
        if let Some(sql) = Self::filter_sql("mgmt_only", FilterKind::String) {
            push_string_in(builder, &mut where_state, sql, &filter.mgmt_only);
        }
    }

    async fn count_filtered(&self, filter: &InterfaceListFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM dcim_interface iface");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count interfaces: {}", e)))
    }
}

impl InterfaceRepository for SqlxInterfaceRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InterfaceRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT iface.id,
                   iface.device_id,
                   device.name AS device_name,
                   iface.name,
                   iface.label,
                   iface.description,
                   iface.type,
                   iface.enabled,
                   iface.mgmt_only
            FROM dcim_interface iface
            LEFT JOIN dcim_device device ON device.id = iface.device_id
            WHERE iface.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load interface: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: InterfaceListFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<InterfaceRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT iface.id,
                   iface.device_id,
                   device.name AS device_name,
                   iface.name,
                   iface.label,
                   iface.description,
                   iface.type,
                   iface.enabled,
                   iface.mgmt_only,
                   COUNT(*) OVER() AS total_count
            FROM dcim_interface iface
            LEFT JOIN dcim_device device ON device.id = iface.device_id
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(&mut builder, INTERFACE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list interfaces: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| {
                AppError::Database(format!("Failed to read interface total_count: {}", e))
            })?;
        let total = match total {
            Some(total) => total,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn list_by_device(
        &self,
        device_id: Uuid,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<InterfaceRecord>, AppError> {
        self.list(
            InterfaceListFilter {
                device_id: vec![device_id],
                ..InterfaceListFilter::default()
            },
            limit,
            offset,
            None,
        )
        .await
        .map(|(items, _total)| items)
    }

    async fn create(&self, new_interface: &NewInterface) -> Result<InterfaceRecord, AppError> {
        let label = new_interface.label.clone().unwrap_or_default();
        let description = new_interface.description.clone().unwrap_or_default();
        let enabled = new_interface.enabled.unwrap_or(true);
        let mgmt_only = new_interface.mgmt_only.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO dcim_interface (
                id,
                custom_field_data,
                name,
                label,
                description,
                mark_connected,
                enabled,
                _name,
                type,
                mgmt_only,
                device_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, false, $5, $2, $6, $7, $8)
            "#,
        )
        .bind(new_interface.id)
        .bind(&new_interface.name)
        .bind(&label)
        .bind(&description)
        .bind(enabled)
        .bind(&new_interface.type_)
        .bind(mgmt_only)
        .bind(new_interface.device_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create interface: {}", e)))?;

        self.get_by_id(new_interface.id)
            .await?
            .ok_or_else(|| AppError::Database("Created interface disappeared".to_string()))
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInterface,
    ) -> Result<Option<InterfaceRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let type_ = changes.type_.clone().unwrap_or(existing.type_);
        let label = changes.label.clone().unwrap_or(existing.label);
        let description = changes.description.clone().unwrap_or(existing.description);
        let enabled = changes.enabled.unwrap_or(existing.enabled);
        let mgmt_only = changes.mgmt_only.unwrap_or(existing.mgmt_only);

        sqlx::query(
            r#"
            UPDATE dcim_interface
            SET name = $2, _name = $2, type = $3, label = $4,
                description = $5, enabled = $6, mgmt_only = $7
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&type_)
        .bind(&label)
        .bind(&description)
        .bind(enabled)
        .bind(mgmt_only)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update interface: {}", e)))?;

        self.get_by_id(id).await
    }

    async fn bulk_create(&self, items: &[NewInterface]) -> Result<BulkInterfaceResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let label = item.label.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();
            let enabled = item.enabled.unwrap_or(true);
            let mgmt_only = item.mgmt_only.unwrap_or(false);

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_interface (
                    id, custom_field_data, name, label, description,
                    mark_connected, enabled, _name, type, mgmt_only, device_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, false, $5, $2, $6, $7, $8)
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&label)
                .bind(&description)
                .bind(enabled)
                .bind(&item.type_)
                .bind(mgmt_only)
                .bind(item.device_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(InterfaceRecord {
                    id: item.id,
                    device_id: Some(item.device_id),
                    device_name: None,
                    name: item.name.clone(),
                    label,
                    description,
                    type_: item.type_.clone(),
                    enabled,
                    mgmt_only,
                }),
                Err(e) => errors.push(format!("Failed to insert interface '{}': {}", item.name, e)),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk interface insert: {}", e))
        })?;

        let mut hydrated = Vec::with_capacity(created.len());
        for record in created {
            let current = self.get_by_id(record.id).await?.ok_or_else(|| {
                AppError::Database("Bulk-created interface disappeared".to_string())
            })?;
            hydrated.push(current);
        }

        Ok(BulkInterfaceResult {
            created: hydrated,
            errors,
        })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_interface", id)
            .await
            .map(|_| ())
    }
}
