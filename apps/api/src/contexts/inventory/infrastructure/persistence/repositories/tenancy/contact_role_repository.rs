//! ContactRole repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{ListColumn, SortSpec, push_order_by};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const CONTACT_ROLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

/// Minimal representation of a `tenancy_contactrole` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct ContactRoleRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

/// Input for creating a new `tenancy_contactrole` row.
#[derive(Debug, Clone)]
pub struct NewContactRole {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `tenancy_contactrole` row.
#[derive(Debug, Clone)]
pub struct UpdateContactRole {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ContactRoleRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactRoleRecord>, AppError>;
    async fn list(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ContactRoleRecord>, i64), AppError>;
    async fn create(&self, new_role: &NewContactRole) -> Result<ContactRoleRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContactRole,
    ) -> Result<Option<ContactRoleRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewContactRole],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ContactRoleRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxContactRoleRepository {
    pool: PgPool,
}

impl SqlxContactRoleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<ContactRoleRecord, AppError> {
        Ok(ContactRoleRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read contact_role id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read contact_role name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read contact_role slug: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read contactrole description: {}", e))
                })?,
        })
    }
}

impl ContactRoleRepository for SqlxContactRoleRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<ContactRoleRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, description
            FROM tenancy_contactrole
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load contact_role: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ContactRoleRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let search_pattern = search.as_deref().map(|s| format!("%{}%", s));

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, description,
                   COUNT(*) OVER() AS total_count
            FROM tenancy_contactrole
            WHERE (
            "#,
        );
        builder.push_bind(search_pattern.as_deref());
        builder.push("::text IS NULL OR name ILIKE ");
        builder.push_bind(search_pattern.as_deref());
        builder.push(" OR slug ILIKE ");
        builder.push_bind(search_pattern.as_deref());
        builder.push(")");
        push_order_by(&mut builder, CONTACT_ROLE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list contact_roles: {}", e)))?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| {
                AppError::Database(format!("Failed to read contact_role total_count: {}", e))
            })?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_role: &NewContactRole) -> Result<ContactRoleRecord, AppError> {
        sqlx::query(
            r#"
            INSERT INTO tenancy_contactrole (
                id, name, slug, description, custom_field_data, comments, tags
            )
            VALUES ($1, $2, $3, COALESCE($4, ''), '{}'::jsonb, '', '{}')
            "#,
        )
        .bind(new_role.id)
        .bind(&new_role.name)
        .bind(&new_role.slug)
        .bind(new_role.description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create contact_role: {}", e)))?;

        Ok(ContactRoleRecord {
            id: new_role.id,
            name: new_role.name.clone(),
            slug: new_role.slug.clone(),
            description: new_role.description.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateContactRole,
    ) -> Result<Option<ContactRoleRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE tenancy_contactrole
            SET name = $2, slug = $3, description = $4
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update contact_role: {}", e)))?;

        Ok(Some(ContactRoleRecord {
            id,
            name,
            slug,
            description,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewContactRole],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<ContactRoleRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk contact_role insert",
            |connection, item| {
                Box::pin(async move {
                    sqlx::query(
                        r#"
                INSERT INTO tenancy_contactrole (
                    id, name, slug, description, custom_field_data, comments, tags
                )
                VALUES ($1, $2, $3, COALESCE($4, ''), '{}'::jsonb, '', '{}')
                "#,
                    )
                    .bind(item.id)
                    .bind(&item.name)
                    .bind(&item.slug)
                    .bind(item.description.as_deref())
                    .execute(connection)
                    .await
                    .map_err(|error| {
                        format!("Failed to insert contact_role '{}': {error}", item.name)
                    })?;

                    Ok(ContactRoleRecord {
                        id: item.id,
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        description: item.description.clone(),
                    })
                })
            },
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        // The DELETE contract for this entity advertises 404; deleting an
        // absent row must not silently answer 204.
        let deleted = super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "tenancy_contactrole",
            id,
        )
        .await?;
        if deleted == 0 {
            return Err(AppError::NotFound(format!("Contact role '{id}' not found")));
        }
        Ok(())
    }
}
