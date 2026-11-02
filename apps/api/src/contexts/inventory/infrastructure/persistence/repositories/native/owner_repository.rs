use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::Owner;
use crate::contexts::shared::list_query::{ListColumn, SortSpec, push_order_by};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const OWNER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "o.name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "description",
        "o.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::fk_filter(
        "group_id",
        "o.group_id",
        "group_id",
        "group_id",
        "ownerGroups",
        "inventory.resources.fields.group",
    ),
];

const SELECT_OWNER: &str = "SELECT o.id, o.name, o.description, o.group_id,
    ARRAY(SELECT user_id FROM users_owner_identity_user WHERE owner_id = o.id ORDER BY user_id) AS users,
    ARRAY(SELECT group_id FROM users_owner_identity_group WHERE owner_id = o.id ORDER BY group_id) AS user_groups
    FROM users_owner o";

#[derive(Debug, Clone, Default)]
pub struct OwnerChanges {
    pub name: Option<String>,
    pub description: Option<String>,
    /// None leaves the association unchanged; Some(None) clears it.
    pub group_id: Option<Option<Uuid>>,
    pub users: Option<Vec<Uuid>>,
    pub user_groups: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Default)]
pub struct OwnerFilter {
    pub search: Option<String>,
    pub group_ids: Vec<Uuid>,
    pub user_ids: Vec<Uuid>,
    pub user_group_ids: Vec<Uuid>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<SortSpec>,
}

pub trait OwnerRepository: Send + Sync {
    fn get_by_id(&self, id: Uuid) -> impl Future<Output = Result<Option<Owner>, AppError>> + Send;
    fn list(
        &self,
        filter: OwnerFilter,
    ) -> impl Future<Output = Result<(Vec<Owner>, i64), AppError>> + Send;
    fn create(&self, owner: &Owner) -> impl Future<Output = Result<Owner, AppError>> + Send;
    fn update(
        &self,
        id: Uuid,
        changes: &OwnerChanges,
    ) -> impl Future<Output = Result<Option<Owner>, AppError>> + Send;
    fn delete(&self, id: Uuid) -> impl Future<Output = Result<(), AppError>> + Send;
}

#[derive(Clone)]
pub struct SqlxOwnerRepository {
    pool: PgPool,
}

impl SqlxOwnerRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &OwnerFilter) {
    builder
        .push(" WHERE (")
        .push_bind(filter.search.as_ref().map(|s| format!("%{s}%")))
        .push("::text IS NULL OR o.name ILIKE ")
        .push_bind(filter.search.as_ref().map(|s| format!("%{s}%")))
        .push(" OR o.description ILIKE ")
        .push_bind(filter.search.as_ref().map(|s| format!("%{s}%")))
        .push(")");
    for (column, ids) in [
        ("o.group_id", &filter.group_ids),
        (
            "o.id IN (SELECT owner_id FROM users_owner_identity_user WHERE user_id",
            &filter.user_ids,
        ),
        (
            "o.id IN (SELECT owner_id FROM users_owner_identity_group WHERE group_id",
            &filter.user_group_ids,
        ),
    ] {
        if !ids.is_empty() {
            builder
                .push(" AND ")
                .push(column)
                .push(" = ANY(")
                .push_bind(ids.clone())
                .push(")");
            if column.starts_with("o.id IN") {
                builder.push(")");
            }
        }
    }
}

impl OwnerRepository for SqlxOwnerRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<Owner>, AppError> {
        sqlx::query_as(&format!("{SELECT_OWNER} WHERE o.id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    async fn list(&self, filter: OwnerFilter) -> Result<(Vec<Owner>, i64), AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM users_owner o");
        filters(&mut count, &filter);
        let total = count
            .build_query_scalar()
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(SELECT_OWNER);
        filters(&mut query, &filter);
        push_order_by(&mut query, OWNER_COLUMNS, filter.sort.as_ref(), "o.name");
        query
            .push(", o.id LIMIT ")
            .push_bind(i64::from(clamp_limit(filter.limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(filter.offset)));
        let rows = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok((rows, total))
    }

    async fn create(&self, owner: &Owner) -> Result<Owner, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("INSERT INTO users_owner (id,name,description,group_id) VALUES ($1,$2,$3,$4)")
            .bind(owner.id)
            .bind(&owner.name)
            .bind(&owner.description)
            .bind(owner.group_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        assignments(
            &mut tx,
            owner.id,
            Some(&owner.users),
            Some(&owner.user_groups),
        )
        .await?;
        let row = sqlx::query_as(&format!("{SELECT_OWNER} WHERE o.id = $1"))
            .bind(owner.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(row)
    }

    async fn update(&self, id: Uuid, changes: &OwnerChanges) -> Result<Option<Owner>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM users_owner WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
        if exists.is_none() {
            return Ok(None);
        }
        sqlx::query("UPDATE users_owner SET name=COALESCE($2,name), description=COALESCE($3,description), group_id=CASE WHEN $4 THEN $5 ELSE group_id END WHERE id=$1")
            .bind(id).bind(&changes.name).bind(&changes.description).bind(changes.group_id.is_some()).bind(changes.group_id.flatten())
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        assignments(
            &mut tx,
            id,
            changes.users.as_deref(),
            changes.user_groups.as_deref(),
        )
        .await?;
        let row = sqlx::query_as(&format!("{SELECT_OWNER} WHERE o.id = $1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(row))
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        if super::super::delete_guard::delete_inventory_record(&self.pool, "users_owner", id)
            .await?
            == 0
        {
            return Err(AppError::NotFound("Owner not found".into()));
        }
        Ok(())
    }
}

async fn assignments(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    id: Uuid,
    users: Option<&[Uuid]>,
    groups: Option<&[Uuid]>,
) -> Result<(), AppError> {
    for (table, column, values) in [
        ("users_owner_identity_user", "user_id", users),
        ("users_owner_identity_group", "group_id", groups),
    ] {
        if let Some(values) = values {
            sqlx::query(&format!("DELETE FROM {table} WHERE owner_id=$1"))
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            sqlx::query(&format!("INSERT INTO {table} (owner_id,{column}) SELECT $1, value FROM (SELECT DISTINCT unnest($2::uuid[]) AS value) ids"))
                .bind(id).bind(values).execute(&mut **tx).await.map_err(classify_sqlx_error_owned)?;
        }
    }
    Ok(())
}
