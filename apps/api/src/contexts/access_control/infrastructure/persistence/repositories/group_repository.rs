use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::repository::{GROUP_COLUMNS, GroupRepository, NamedListFilter};
use crate::contexts::access_control::domain::Group;
use crate::contexts::shared::list_query::push_order_by;
use crate::contexts::shared::pagination::PageResult;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxGroupRepository {
    pool: PgPool,
}

impl SqlxGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_group_row(row: &PgRow) -> Result<Group, AppError> {
        Ok(Group {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read group id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read group name: {}", e)))?,
            external_id: row.try_get("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read group external_id: {}", e))
            })?,
        })
    }
}

#[async_trait]
impl GroupRepository for SqlxGroupRepository {
    async fn get_by_id(&self, group_id: Uuid) -> Result<Option<Group>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, external_id
            FROM identity_group
            WHERE id = $1
            "#,
        )
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load group: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_group_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_groups(&self) -> Result<Vec<Group>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, external_id
            FROM identity_group
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list groups: {}", e)))?;

        rows.iter()
            .map(Self::parse_group_row)
            .collect::<Result<Vec<Group>, AppError>>()
    }

    async fn list_groups_page(
        &self,
        filter: NamedListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Group>, AppError> {
        let limit = clamp_limit(Some(limit));
        let offset = clamp_offset(Some(offset));

        let push_where = |builder: &mut QueryBuilder<'_, Postgres>| {
            if let Some(search) = filter.search.as_deref() {
                builder.push(" WHERE name ILIKE ");
                builder.push_bind(search.to_string());
            }
        };

        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT id, name, external_id FROM identity_group");
        push_where(&mut builder);
        push_order_by(&mut builder, GROUP_COLUMNS, filter.sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(offset as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list groups page: {}", e)))?;

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*)::bigint FROM identity_group");
        push_where(&mut count_builder);
        let total: i64 = count_builder
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count groups: {}", e)))?;

        let total = u64::try_from(total)
            .map_err(|_| AppError::Internal("Groups total count overflow".to_string()))?;

        Ok(PageResult {
            items: rows
                .iter()
                .map(Self::parse_group_row)
                .collect::<Result<Vec<Group>, AppError>>()?,
            total,
            limit,
            offset,
        })
    }

    async fn create_group(&self, group: Group) -> Result<Group, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO identity_group (id, name, external_id)
            VALUES ($1, $2, $3)
            RETURNING id, name, external_id
            "#,
        )
        .bind(group.id)
        .bind(&group.name)
        .bind(&group.external_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create group: {}", e)))?;

        Self::parse_group_row(&row)
    }

    async fn update_group(&self, group: Group) -> Result<Group, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE identity_group
            SET name = $2,
                external_id = $3
            WHERE id = $1
            RETURNING id, name, external_id
            "#,
        )
        .bind(group.id)
        .bind(&group.name)
        .bind(&group.external_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update group: {}", e)))?;

        Self::parse_group_row(&row)
    }

    async fn delete_group(&self, group_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM identity_group WHERE id = $1")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to delete group: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }
}
