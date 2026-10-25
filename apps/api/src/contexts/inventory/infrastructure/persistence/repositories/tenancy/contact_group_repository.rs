//! ContactGroup repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const CONTACT_GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::fk_filter(
        "parent",
        "parent_id",
        "parent_id",
        "parent_id",
        "contactGroups",
        "inventory.resources.fields.parent",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

/// Minimal representation of a `tenancy_contactgroup` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ContactGroupRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Input for creating a new `tenancy_contactgroup` row.
#[derive(Debug, Clone)]
pub struct NewContactGroup {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `tenancy_contactgroup` row.
#[derive(Debug, Clone)]
pub struct UpdateContactGroup {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkContactGroupResult {
    pub created: Vec<ContactGroupRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ContactGroupRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactGroupRecord>, AppError>;
    async fn list(
        &self,
        parent_ids: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ContactGroupRecord>, i64), AppError>;
    async fn create(&self, new_group: &NewContactGroup) -> Result<ContactGroupRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContactGroup,
    ) -> Result<Option<ContactGroupRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewContactGroup],
    ) -> Result<BulkContactGroupResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxContactGroupRepository {
    pool: PgPool,
}

impl SqlxContactGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ContactGroupRecord, AppError> {
        Ok(ContactGroupRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read contact_group id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read contact_group name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read contact_group slug: {}", e))
            })?,
            parent_id: row.try_get::<Option<Uuid>, _>("parent_id").map_err(|e| {
                AppError::Database(format!("Failed to read contactgroup parent_id: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read contactgroup description: {}", e))
                })?,
        })
    }
}

impl ContactGroupRepository for SqlxContactGroupRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactGroupRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, parent_id, description
            FROM tenancy_contactgroup
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load contact_group: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        parent_ids: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ContactGroupRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, parent_id, description,
                   COUNT(*) OVER() AS total_count
            FROM tenancy_contactgroup
            "#,
        );
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut builder,
            &mut where_state,
            &["name", "slug", "description"],
            search.as_deref(),
        );
        push_uuid_in(&mut builder, &mut where_state, "parent_id", parent_ids);
        push_order_by(&mut builder, CONTACT_GROUP_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list contact_groups: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| {
                AppError::Database(format!("Failed to read contact_group total_count: {}", e))
            })?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_group: &NewContactGroup) -> Result<ContactGroupRecord, AppError> {
        sqlx::query(
            r#"
            INSERT INTO tenancy_contactgroup (
                id, name, slug, parent_id, description,
                custom_field_data, comments, lft, rght, tree_id, level, tags
            )
            VALUES ($1, $2, $3, $4, $5, '{}'::jsonb, '', 0, 0, 0, 0, '{}')
            "#,
        )
        .bind(new_group.id)
        .bind(&new_group.name)
        .bind(&new_group.slug)
        .bind(new_group.parent_id)
        .bind(new_group.description.as_deref().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create contact_group: {}", e)))?;

        Ok(ContactGroupRecord {
            id: new_group.id,
            name: new_group.name.clone(),
            slug: new_group.slug.clone(),
            parent_id: new_group.parent_id,
            description: Some(new_group.description.clone().unwrap_or_default()),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContactGroup,
    ) -> Result<Option<ContactGroupRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let parent_id = if changes.parent_id.is_some() {
            changes.parent_id
        } else {
            existing.parent_id
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE tenancy_contactgroup
            SET name = $2, slug = $3, parent_id = $4, description = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(parent_id)
        .bind(description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update contact_group: {}", e)))?;

        Ok(Some(ContactGroupRecord {
            id,
            name,
            slug,
            parent_id,
            description,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewContactGroup],
    ) -> Result<BulkContactGroupResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO tenancy_contactgroup (
                    id, name, slug, parent_id, description,
                    custom_field_data, comments, lft, rght, tree_id, level, tags
                )
                VALUES ($1, $2, $3, $4, $5, '{}'::jsonb, '', 0, 0, 0, 0, '{}')
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&item.slug)
                .bind(item.parent_id)
                .bind(item.description.as_deref().unwrap_or_default()),
            )
            .await?;

            match result {
                Ok(_) => created.push(ContactGroupRecord {
                    id: item.id,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    parent_id: item.parent_id,
                    description: Some(item.description.clone().unwrap_or_default()),
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert contact_group '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk contact_group insert: {}", e))
        })?;

        Ok(BulkContactGroupResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "tenancy_contactgroup", id)
            .await
            .map(|_| ())
    }
}
