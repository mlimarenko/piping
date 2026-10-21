use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::CircuitGroup;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const CIRCUIT_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::fk_filter(
        "tenant",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "Tenants",
        "inventory.resources.fields.tenant",
    ),
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
pub trait CircuitGroupRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitGroup>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        tenant_ids: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CircuitGroup>, i64), AppError>;
    async fn create(&self, item: &CircuitGroup) -> Result<(), AppError>;
    async fn update(&self, item: &CircuitGroup) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxCircuitGroupRepository {
    pool: PgPool,
}

impl SqlxCircuitGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<CircuitGroup, AppError> {
        Ok(CircuitGroup {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read circuits_circuitgroup id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read circuits_circuitgroup name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read circuits_circuitgroup slug: {}", e))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuitgroup description: {}",
                    e
                ))
            })?,
            tenant_id: row.try_get("tenant_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuitgroup tenant_id: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuitgroup comments: {}",
                    e
                ))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl CircuitGroupRepository for SqlxCircuitGroupRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitGroup>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, description, tenant_id, comments, custom_field_data
            FROM circuits_circuitgroup
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load circuits_circuitgroup: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        tenant_ids: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CircuitGroup>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, description, tenant_id, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_circuitgroup
            "#,
        );
        push_circuit_group_filters(&mut builder, tenant_ids, search.as_deref());
        push_order_by(&mut builder, CIRCUIT_GROUP_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list circuits_circuitgroups: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_circuitgroup total_count: {}",
                    e
                ))
            })?,
            None => count_circuit_groups(&self.pool, tenant_ids, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<CircuitGroup>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &CircuitGroup) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO circuits_circuitgroup (
                id, custom_field_data, name, slug, description, tenant_id, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.slug)
        .bind(&item.description)
        .bind(item.tenant_id)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to create circuits_circuitgroup: {}", e))
        })?;
        Ok(())
    }

    async fn update(&self, item: &CircuitGroup) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE circuits_circuitgroup
            SET name = $2, slug = $3, description = $4, tenant_id = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.name)
        .bind(&item.slug)
        .bind(&item.description)
        .bind(item.tenant_id)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to update circuits_circuitgroup: {}", e))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "circuits_circuitgroup", id)
            .await
            .map(|_| ())
    }
}

fn push_circuit_group_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    tenant_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "slug", "description", "comments"],
        search,
    );
    push_uuid_in(builder, &mut where_state, "tenant_id", tenant_ids);
}

async fn count_circuit_groups(
    pool: &PgPool,
    tenant_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM circuits_circuitgroup");
    push_circuit_group_filters(&mut builder, tenant_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count circuits_circuitgroups: {}", e)))
}
