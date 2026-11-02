//! VirtualDisk repository — full CRUD over `virtualization_virtualdisk`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    FilterKind, ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const VIRTUAL_DISK_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "virtual_machine_id",
        "virtual_machine_id",
        "virtual_machine_id",
        "virtual_machine_id",
        "virtualMachines",
        "inventory.virtualDisks.columns.virtualMachine",
    ),
    ListColumn::sortable("name", "name", "inventory.virtualDisks.columns.name"),
    ListColumn::sortable("size", "size", "inventory.virtualDisks.columns.size"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.virtualDisks.columns.description",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.virtualDisks.columns.owner",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VirtualDiskFilter {
    pub search: Option<String>,
    pub virtual_machine_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

/// Representation of a `virtualization_virtualdisk` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct VirtualDiskRecord {
    pub id: Uuid,
    pub virtual_machine_id: Uuid,
    pub name: String,
    pub size: i32,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `virtualization_virtualdisk` row.
#[derive(Debug, Clone)]
pub struct NewVirtualDisk {
    pub id: Uuid,
    pub virtual_machine_id: Uuid,
    pub name: String,
    pub size: i32,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `virtualization_virtualdisk` row.
#[derive(Debug, Clone)]
pub struct UpdateVirtualDisk {
    pub name: Option<String>,
    pub size: Option<i32>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkVirtualDiskResult {
    pub created: Vec<VirtualDiskRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VirtualDiskRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualDiskRecord>, AppError>;
    async fn list(
        &self,
        filter: VirtualDiskFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VirtualDiskRecord>, i64), AppError>;
    async fn create(&self, new_disk: &NewVirtualDisk) -> Result<VirtualDiskRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVirtualDisk,
    ) -> Result<Option<VirtualDiskRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewVirtualDisk],
    ) -> Result<BulkVirtualDiskResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVirtualDiskRepository {
    pool: PgPool,
}

impl SqlxVirtualDiskRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VirtualDiskRecord, AppError> {
        Ok(VirtualDiskRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_disk id: {}", e))
            })?,
            virtual_machine_id: row.try_get("virtual_machine_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read virtual_disk virtual_machine_id: {}",
                    e
                ))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_disk name: {}", e))
            })?,
            size: row.try_get("size").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_disk size: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read virtual_disk description: {}", e))
                })?,
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_disk owner_id: {}", e))
            })?,
            owner_name: row
                .try_get::<Option<String>, _>("owner_name")
                .ok()
                .flatten(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn filter_values<'a>(filter: &'a VirtualDiskFilter, param: &str) -> &'a [Uuid] {
        match param {
            "virtual_machine_id" => &filter.virtual_machine_id,
            "owner_id" => &filter.owner_id,
            _ => &[],
        }
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &VirtualDiskFilter) {
        let has_filter = filter.search.is_some()
            || !filter.virtual_machine_id.is_empty()
            || !filter.owner_id.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "size", "description"],
            filter.search.as_deref(),
        );

        for column in VIRTUAL_DISK_COLUMNS {
            let Some(column_filter) = column.filter else {
                continue;
            };
            if column_filter.kind == FilterKind::Uuid {
                push_uuid_in(
                    builder,
                    &mut where_state,
                    column_filter.sql,
                    Self::filter_values(filter, column_filter.param),
                );
            }
        }
    }

    async fn count_filtered(&self, filter: &VirtualDiskFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM virtualization_virtualdisk");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count virtual_disks: {}", e)))
    }
}

impl VirtualDiskRepository for SqlxVirtualDiskRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualDiskRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, virtual_machine_id, name, size, description, owner_id,
                   NULL::text AS owner_name, custom_field_data
            FROM virtualization_virtualdisk
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load virtual_disk: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: VirtualDiskFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<VirtualDiskRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            "
            SELECT id, virtual_machine_id, name, size, description, owner_id, owner_name,
                   custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT vd.id, vd.virtual_machine_id, vd.name, vd.size, vd.description,
                       vd.owner_id, COALESCE(NULLIF(u.username, ''), u.email) AS owner_name,
                       vd.custom_field_data
                FROM virtualization_virtualdisk vd
                LEFT JOIN identity_user u ON u.id = vd.owner_id
            ) virtual_disk_rows
            ",
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            VIRTUAL_DISK_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list virtual_disks: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read virtual_disk total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };

        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VirtualDiskRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_disk: &NewVirtualDisk) -> Result<VirtualDiskRecord, AppError> {
        let description = new_disk.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO virtualization_virtualdisk (
                id, custom_field_data, virtual_machine_id, name, size, description, owner_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(new_disk.id)
        .bind(new_disk.virtual_machine_id)
        .bind(&new_disk.name)
        .bind(new_disk.size)
        .bind(&description)
        .bind(new_disk.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create virtual_disk: {}", e)))?;

        Ok(VirtualDiskRecord {
            id: new_disk.id,
            virtual_machine_id: new_disk.virtual_machine_id,
            name: new_disk.name.clone(),
            size: new_disk.size,
            description: Some(description),
            owner_id: new_disk.owner_id,
            owner_name: None,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateVirtualDisk,
    ) -> Result<Option<VirtualDiskRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let size = changes.size.unwrap_or(existing.size);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE virtualization_virtualdisk
            SET name = $2, size = $3, description = $4
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(size)
        .bind(description.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update virtual_disk: {}", e)))?;

        Ok(Some(VirtualDiskRecord {
            id,
            virtual_machine_id: existing.virtual_machine_id,
            name,
            size,
            description,
            owner_id: existing.owner_id,
            owner_name: existing.owner_name,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewVirtualDisk],
    ) -> Result<BulkVirtualDiskResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO virtualization_virtualdisk (
                    id, custom_field_data, virtual_machine_id, name, size, description, owner_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
                "#,
                )
                .bind(item.id)
                .bind(item.virtual_machine_id)
                .bind(&item.name)
                .bind(item.size)
                .bind(&description)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(VirtualDiskRecord {
                    id: item.id,
                    virtual_machine_id: item.virtual_machine_id,
                    name: item.name.clone(),
                    size: item.size,
                    description: Some(description),
                    owner_id: item.owner_id,
                    owner_name: None,
                    custom_fields: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert virtual_disk '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk virtual_disk insert: {}", e))
        })?;

        Ok(BulkVirtualDiskResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "virtualization_virtualdisk",
            id,
        )
        .await
        .map(|_| ())
    }
}
