use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::VirtualCircuitType;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const VIRTUAL_CIRCUIT_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::sortable("color", "color", "inventory.resources.fields.color"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.resources.fields.comments",
    ),
];

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VirtualCircuitTypeRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualCircuitType>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VirtualCircuitType>, i64), AppError>;
    async fn create(&self, item: &VirtualCircuitType) -> Result<(), AppError>;
    async fn update(&self, item: &VirtualCircuitType) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxVirtualCircuitTypeRepository {
    pool: PgPool,
}

impl SqlxVirtualCircuitTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<VirtualCircuitType, AppError> {
        Ok(VirtualCircuitType {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype id: {}",
                    e
                ))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype name: {}",
                    e
                ))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype slug: {}",
                    e
                ))
            })?,
            color: row.try_get("color").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype color: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype comments: {}",
                    e
                ))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl VirtualCircuitTypeRepository for SqlxVirtualCircuitTypeRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualCircuitType>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, color, description, comments, custom_field_data
            FROM circuits_virtualcircuittype
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load circuits_virtualcircuittype: {}", e))
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
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<VirtualCircuitType>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, color, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_virtualcircuittype
            "#,
        );
        push_virtual_circuit_type_filters(&mut builder, search.as_deref());
        push_order_by(
            &mut builder,
            VIRTUAL_CIRCUIT_TYPE_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to list circuits_virtualcircuittypes: {}",
                e
            ))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_virtualcircuittype total_count: {}",
                    e
                ))
            })?,
            None => count_virtual_circuit_types(&self.pool, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<VirtualCircuitType>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &VirtualCircuitType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO circuits_virtualcircuittype (
                id, custom_field_data, name, slug, color, description, comments
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7)
            "#,
        )
        .bind(item.id)
        .bind(item.custom_fields.clone())
        .bind(&item.name)
        .bind(&item.slug)
        .bind(&item.color)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to create circuits_virtualcircuittype: {}",
                e
            ))
        })?;
        Ok(())
    }

    async fn update(&self, item: &VirtualCircuitType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE circuits_virtualcircuittype
            SET name = $2,
                slug = $3,
                color = $4,
                description = $5,
                comments = $6,
                custom_field_data = COALESCE($7, custom_field_data)
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.slug)
        .bind(&item.color)
        .bind(&item.description)
        .bind(&item.comments)
        .bind(item.custom_fields.clone())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to update circuits_virtualcircuittype: {}",
                e
            ))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_virtualcircuittype",
            id,
        )
        .await
        .map(|_| ())
    }
}

fn push_virtual_circuit_type_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "slug", "color", "description", "comments"],
        search,
    );
}

async fn count_virtual_circuit_types(pool: &PgPool, search: Option<&str>) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM circuits_virtualcircuittype");
    push_virtual_circuit_type_filters(&mut builder, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to count circuits_virtualcircuittypes: {}",
                e
            ))
        })
}
