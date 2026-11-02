use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::{ListColumn, SortSpec, push_order_by, push_uuid_in};
use crate::contexts::inventory::domain::ProviderAccount;
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::list_query::push_search_ilike;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const PROVIDER_ACCOUNT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "pa.name", "inventory.providerAccounts.columns.name"),
    ListColumn::sortable(
        "account",
        "pa.account",
        "inventory.providerAccounts.columns.account",
    ),
    ListColumn::fk_filter(
        "provider",
        "p.name",
        "provider_id",
        "pa.provider_id",
        "providers",
        "inventory.providerAccounts.columns.provider",
    ),
    ListColumn::sortable(
        "description",
        "pa.description",
        "inventory.providerAccounts.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "pa.comments",
        "inventory.providerAccounts.columns.comments",
    ),
];

/// Hydrated list row for `circuits_provideraccount` — includes provider name
/// resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct ProviderAccountListItem {
    pub record: ProviderAccount,
    pub provider_name: Option<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ProviderAccountRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ProviderAccount>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ProviderAccount>, i64), AppError>;
    async fn list_paginated_hydrated(
        &self,
        provider_id: Vec<Uuid>,
        search: Option<String>,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ProviderAccountListItem>, i64), AppError>;
    async fn create(&self, item: &ProviderAccount) -> Result<(), AppError>;
    async fn update(&self, item: &ProviderAccount) -> Result<(), AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxProviderAccountRepository {
    pool: PgPool,
}

impl SqlxProviderAccountRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn count_hydrated(
        &self,
        provider_id: &[Uuid],
        search: Option<&str>,
    ) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*) FROM circuits_provideraccount pa LEFT JOIN circuits_provider p ON p.id = pa.provider_id",
        );
        push_provider_account_filters(&mut builder, provider_id, search);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to count circuits_provideraccounts: {}", e))
            })
    }

    fn parse_row(row: &PgRow) -> Result<ProviderAccount, AppError> {
        Ok(ProviderAccount {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read circuits_provideraccount id: {}", e))
            })?,
            account: row.try_get("account").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount account: {}",
                    e
                ))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount name: {}",
                    e
                ))
            })?,
            provider_id: row.try_get("provider_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount provider_id: {}",
                    e
                ))
            })?,
            description: row.try_get("description").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount description: {}",
                    e
                ))
            })?,
            comments: row.try_get("comments").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount comments: {}",
                    e
                ))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl ProviderAccountRepository for SqlxProviderAccountRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ProviderAccount>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, account, name, provider_id, description, comments, custom_field_data
            FROM circuits_provideraccount
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load circuits_provideraccount: {}", e))
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
    ) -> Result<(Vec<ProviderAccount>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let rows = sqlx::query(
            r#"
            SELECT id, account, name, provider_id, description, comments, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_provideraccount
            ORDER BY name
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to list circuits_provideraccounts: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount total_count: {}",
                    e
                ))
            })?,
            None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM circuits_provideraccount")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    AppError::Database(format!("Failed to count circuits_provideraccounts: {}", e))
                })?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<ProviderAccount>, AppError>>()?;
        Ok((items, total))
    }

    async fn list_paginated_hydrated(
        &self,
        provider_id: Vec<Uuid>,
        search: Option<String>,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ProviderAccountListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT pa.id, pa.account, pa.name, pa.provider_id, pa.description,
                   pa.comments, pa.custom_field_data,
                   p.name AS provider_name,
                   COUNT(*) OVER() AS total_count
            FROM circuits_provideraccount pa
            LEFT JOIN circuits_provider p ON p.id = pa.provider_id
            "#,
        );
        push_provider_account_filters(&mut builder, &provider_id, search.as_deref());
        push_order_by(
            &mut builder,
            PROVIDER_ACCOUNT_COLUMNS,
            sort.as_ref(),
            "pa.name ASC, pa.id",
        );
        if sort.is_some() {
            builder.push(", pa.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to list circuits_provideraccounts hydrated: {}",
                e
            ))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read circuits_provideraccount total_count: {}",
                    e
                ))
            })?,
            None => self.count_hydrated(&provider_id, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(ProviderAccountListItem {
                    record: Self::parse_row(row)?,
                    provider_name: col(row, "provider_name")?,
                })
            })
            .collect::<Result<Vec<ProviderAccountListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &ProviderAccount) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO circuits_provideraccount (
                id, custom_field_data, account, name, provider_id, description, comments
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(item.id)
        .bind(&item.account)
        .bind(&item.name)
        .bind(item.provider_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to create circuits_provideraccount: {}", e))
        })?;
        Ok(())
    }

    async fn update(&self, item: &ProviderAccount) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE circuits_provideraccount
            SET account = $2, name = $3, provider_id = $4, description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(item.id)
        .bind(&item.account)
        .bind(&item.name)
        .bind(item.provider_id)
        .bind(&item.description)
        .bind(&item.comments)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to update circuits_provideraccount: {}", e))
        })?;
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_provideraccount",
            id,
        )
        .await
        .map(|_| ())
    }
}

fn push_provider_account_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    provider_id: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_uuid_in(builder, &mut where_state, "pa.provider_id", provider_id);
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "pa.name",
            "pa.account",
            "pa.description",
            "pa.comments",
            "p.name",
        ],
        search,
    );
}
