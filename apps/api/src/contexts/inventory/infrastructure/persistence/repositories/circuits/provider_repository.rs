use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::Provider;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const PROVIDER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
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
pub trait ProviderRepository: Send + Sync {
    async fn get_by_id(&self, provider_id: Uuid) -> Result<Option<Provider>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<Provider>, i64), AppError>;
    async fn create(&self, provider: &Provider) -> Result<(), AppError>;
    async fn update(&self, provider: &Provider) -> Result<(), AppError>;
    async fn delete(&self, provider_id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxProviderRepository {
    pool: PgPool,
}

impl SqlxProviderRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_provider_row(row: &PgRow) -> Result<Provider, AppError> {
        Ok(Provider {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read provider id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read provider name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read provider slug: {}", e)))?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!("Failed to read provider description: {}", e))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!("Failed to read provider comments: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl ProviderRepository for SqlxProviderRepository {
    async fn get_by_id(&self, provider_id: Uuid) -> Result<Option<Provider>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, description, comments, custom_field_data
            FROM circuits_provider
            WHERE id = $1
            "#,
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load provider: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_provider_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<Provider>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_provider
            "#,
        );
        push_provider_filters(&mut builder, search.as_deref());
        push_order_by(&mut builder, PROVIDER_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list providers: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read provider total_count: {}", e))
            })?,
            None => count_providers(&self.pool, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_provider_row)
            .collect::<Result<Vec<Provider>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, provider: &Provider) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO circuits_provider (
                id,
                custom_field_data,
                name,
                slug,
                description,
                comments
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6)
            "#,
        )
        .bind(provider.id)
        .bind(provider.custom_fields.clone())
        .bind(&provider.name)
        .bind(&provider.slug)
        .bind(&provider.description)
        .bind(&provider.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create provider: {}", e)))?;
        Ok(())
    }

    async fn update(&self, provider: &Provider) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE circuits_provider
            SET name = $2,
                slug = $3,
                description = $4,
                comments = $5,
                custom_field_data = COALESCE($6, custom_field_data)
            WHERE id = $1
            "#,
        )
        .bind(provider.id)
        .bind(&provider.name)
        .bind(&provider.slug)
        .bind(&provider.description)
        .bind(&provider.comments)
        .bind(provider.custom_fields.clone())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update provider: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, provider_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_provider",
            provider_id,
        )
        .await
        .map(|_| ())
    }
}

fn push_provider_filters(builder: &mut QueryBuilder<'_, sqlx::Postgres>, search: Option<&str>) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "slug", "description", "comments"],
        search,
    );
}

async fn count_providers(pool: &PgPool, search: Option<&str>) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM circuits_provider");
    push_provider_filters(&mut builder, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count providers: {}", e)))
}
