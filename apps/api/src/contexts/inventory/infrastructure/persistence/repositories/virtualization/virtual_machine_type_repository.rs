use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VirtualMachineType;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VIRTUAL_MACHINE_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "t.name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "t.slug", "inventory.resources.fields.slug"),
    ListColumn::fk_filter(
        "default_platform",
        "p.name",
        "default_platform_id",
        "t.default_platform_id",
        "platforms",
        "inventory.resources.fields.defaultPlatform",
    ),
    ListColumn::sortable(
        "default_vcpus",
        "t.default_vcpus",
        "inventory.resources.fields.defaultVcpus",
    ),
    ListColumn::sortable(
        "default_memory",
        "t.default_memory",
        "inventory.resources.fields.defaultMemory",
    ),
    ListColumn::fk_filter(
        "owner",
        "o.name",
        "owner_id",
        "t.owner_id",
        "owners",
        "inventory.resources.fields.owner",
    ),
    ListColumn::sortable(
        "description",
        "t.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "t.comments",
        "inventory.resources.fields.comments",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct VirtualMachineTypeFilter {
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
    pub default_platform_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct VirtualMachineTypeChanges {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub default_platform_id: Option<Option<Uuid>>,
    pub default_vcpus: Option<Option<f64>>,
    pub default_memory: Option<Option<i32>>,
    pub owner_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

pub trait VirtualMachineTypeRepository: Send + Sync {
    fn get_by_id(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<VirtualMachineType>, AppError>> + Send;
    fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: VirtualMachineTypeFilter,
    ) -> impl Future<Output = Result<(Vec<VirtualMachineType>, i64), AppError>> + Send;
    fn create(
        &self,
        item: &VirtualMachineType,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
    fn update(
        &self,
        id: Uuid,
        changes: &VirtualMachineTypeChanges,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;
    fn delete(&self, id: Uuid) -> impl Future<Output = Result<(), AppError>> + Send;
}

#[derive(Clone)]
pub struct SqlxVirtualMachineTypeRepository {
    pool: PgPool,
}

impl SqlxVirtualMachineTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VirtualMachineType, AppError> {
        Ok(VirtualMachineType {
            id: col(row, "id")?,
            name: col(row, "name")?,
            slug: col(row, "slug")?,
            default_platform_id: col(row, "default_platform_id")?,
            // NUMERIC must be cast to float8 in the projection; sqlx cannot
            // decode NUMERIC into f64.
            default_vcpus: col(row, "default_vcpus")?,
            default_memory: col(row, "default_memory")?,
            owner_id: col(row, "owner_id")?,
            virtual_machine_count: col(row, "virtual_machine_count")?,
            description: col(row, "description")?,
            comments: col(row, "comments")?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VirtualMachineTypeRepository for SqlxVirtualMachineTypeRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualMachineType>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, default_platform_id,
                   default_vcpus::float8 AS default_vcpus, default_memory, owner_id,
                   description, comments, custom_field_data,
                   (SELECT COUNT(*) FROM virtualization_virtualmachine vm WHERE vm.virtual_machine_type_id = t.id) AS virtual_machine_count
            FROM virtualization_virtualmachinetype t
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load virtualization_virtualmachinetype: {}", e))
        })?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: VirtualMachineTypeFilter,
    ) -> Result<(Vec<VirtualMachineType>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT t.id, t.name, t.slug, t.default_platform_id,
                   t.default_vcpus::float8 AS default_vcpus, t.default_memory, t.owner_id,
                   t.description, t.comments, t.custom_field_data,
                   (SELECT COUNT(*) FROM virtualization_virtualmachine vm WHERE vm.virtual_machine_type_id = t.id) AS virtual_machine_count,
                   COUNT(*) OVER() AS total_count
            FROM virtualization_virtualmachinetype t
            LEFT JOIN dcim_platform p ON p.id = t.default_platform_id
            LEFT JOIN users_owner o ON o.id = t.owner_id
            "#,
        );
        push_virtual_machine_type_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            VIRTUAL_MACHINE_TYPE_COLUMNS,
            filter.sort.as_ref(),
            "t.name ASC, t.id",
        );
        if filter.sort.is_some() {
            builder.push(", t.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to list virtualization_virtualmachinetypes: {}",
                e
            ))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read virtualization_virtualmachinetype total_count: {}",
                    e
                ))
            })?,
            None => count_virtual_machine_types(&self.pool, &filter).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VirtualMachineType>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VirtualMachineType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO virtualization_virtualmachinetype (
                id, custom_field_data, name, slug, default_platform_id,
                default_vcpus, default_memory, description, comments, owner_id
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(item.id)
        .bind(item.custom_fields.clone())
        .bind(&item.name)
        .bind(&item.slug)
        .bind(item.default_platform_id)
        .bind(item.default_vcpus)
        .bind(item.default_memory)
        .bind(&item.description)
        .bind(&item.comments)
        .bind(item.owner_id)
        .execute(&self.pool)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
        Ok(())
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &VirtualMachineTypeChanges,
    ) -> Result<bool, AppError> {
        let mut query = QueryBuilder::new("UPDATE virtualization_virtualmachinetype SET id = id");
        macro_rules! assignment {
            ($field:ident) => {
                if let Some(value) = &changes.$field {
                    query
                        .push(concat!(", ", stringify!($field), " = "))
                        .push_bind(value);
                }
            };
        }
        assignment!(name);
        assignment!(slug);
        assignment!(default_platform_id);
        assignment!(default_vcpus);
        assignment!(default_memory);
        assignment!(owner_id);
        assignment!(description);
        assignment!(comments);
        if let Some(value) = &changes.custom_fields {
            query.push(", custom_field_data = ").push_bind(value);
        }
        query.push(" WHERE id = ").push_bind(id);
        let result = query
            .build()
            .execute(&self.pool)
            .await
            .map_err(crate::errors::classify_sqlx_error_owned)?;
        Ok(result.rows_affected() != 0)
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "virtualization_virtualmachinetype",
            id,
        )
        .await
        .map(|_| ())
    }
}

fn push_virtual_machine_type_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &VirtualMachineTypeFilter,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["t.name", "t.slug", "t.description", "t.comments"],
        filter.search.as_deref(),
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "t.default_platform_id",
        &filter.default_platform_id,
    );
    push_uuid_in(builder, &mut where_state, "t.owner_id", &filter.owner_id);
}

async fn count_virtual_machine_types(
    pool: &PgPool,
    filter: &VirtualMachineTypeFilter,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM virtualization_virtualmachinetype t");
    push_virtual_machine_type_filters(&mut builder, filter);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to count virtualization_virtualmachinetypes: {}",
                e
            ))
        })
}
