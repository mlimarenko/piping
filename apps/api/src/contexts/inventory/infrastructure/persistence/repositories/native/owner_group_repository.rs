use sqlx::{PgPool, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::OwnerGroup;
use crate::contexts::shared::list_query::{ListColumn, SortSpec, push_order_by};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const OWNER_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

pub trait OwnerGroupRepository: Send + Sync {
    fn get_by_id(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<OwnerGroup>, AppError>> + Send;
    fn list(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> impl Future<Output = Result<(Vec<OwnerGroup>, i64), AppError>> + Send;
    fn create(
        &self,
        group: &OwnerGroup,
    ) -> impl Future<Output = Result<OwnerGroup, AppError>> + Send;
    fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
    ) -> impl Future<Output = Result<Option<OwnerGroup>, AppError>> + Send;
    fn delete(&self, id: Uuid) -> impl Future<Output = Result<(), AppError>> + Send;
}

#[derive(Clone)]
pub struct SqlxOwnerGroupRepository {
    pool: PgPool,
}
impl SqlxOwnerGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl OwnerGroupRepository for SqlxOwnerGroupRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<OwnerGroup>, AppError> {
        sqlx::query_as("SELECT id,name,description FROM users_ownergroup WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }
    async fn list(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<OwnerGroup>, i64), AppError> {
        let pattern = search.map(|s| format!("%{s}%"));
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let total = sqlx::query_scalar("SELECT COUNT(*) FROM users_ownergroup WHERE ($1::text IS NULL OR name ILIKE $1 OR description ILIKE $1)")
            .bind(&pattern).fetch_one(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        let mut query =
            QueryBuilder::new("SELECT id,name,description FROM users_ownergroup WHERE (");
        query
            .push_bind(&pattern)
            .push("::text IS NULL OR name ILIKE ")
            .push_bind(&pattern)
            .push(" OR description ILIKE ")
            .push_bind(&pattern)
            .push(")");
        push_order_by(&mut query, OWNER_GROUP_COLUMNS, sort.as_ref(), "name");
        query
            .push(", id LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok((rows, total))
    }
    async fn create(&self, group: &OwnerGroup) -> Result<OwnerGroup, AppError> {
        sqlx::query_as("INSERT INTO users_ownergroup (id,name,description) VALUES ($1,$2,$3) RETURNING id,name,description")
            .bind(group.id).bind(&group.name).bind(&group.description).fetch_one(&self.pool).await.map_err(classify_sqlx_error_owned)
    }
    async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Option<OwnerGroup>, AppError> {
        sqlx::query_as("UPDATE users_ownergroup SET name=COALESCE($2,name), description=COALESCE($3,description) WHERE id=$1 RETURNING id,name,description")
            .bind(id).bind(name).bind(description).fetch_optional(&self.pool).await.map_err(classify_sqlx_error_owned)
    }
    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        if super::super::delete_guard::delete_inventory_record(&self.pool, "users_ownergroup", id)
            .await?
            == 0
        {
            return Err(AppError::NotFound("Owner group not found".into()));
        }
        Ok(())
    }
}
