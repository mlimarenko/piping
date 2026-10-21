//! Rack role repository — full CRUD over `dcim_rackrole`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const RACK_ROLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.rackRoles.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.rackRoles.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.rackRoles.columns.slug"),
    ListColumn::sortable("color", "color", "inventory.rackRoles.columns.color"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.rackRoles.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.rackRoles.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.rackRoles.columns.syncedAt",
    ),
];

/// Representation of a `dcim_rackrole` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct RackRoleRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for creating a new `dcim_rackrole` row.
#[derive(Debug, Clone)]
pub struct NewRackRole {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Fields that may be updated on an existing `dcim_rackrole` row.
#[derive(Debug, Clone)]
pub struct UpdateRackRole {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RackRoleRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackRoleRecord>, AppError>;
    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<RackRoleRecord>, i64), AppError>;
    async fn create(&self, new_role: &NewRackRole) -> Result<RackRoleRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRackRole,
    ) -> Result<Option<RackRoleRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewRackRole],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<RackRoleRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxRackRoleRepository {
    pool: PgPool,
}

impl SqlxRackRoleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<RackRoleRecord, AppError> {
        Ok(RackRoleRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read rack_role id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read rack_role external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read rack_role name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read rack_role slug: {}", e)))?,
            color: row.try_get::<Option<String>, _>("color").ok().flatten(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read rack_role owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read rack_role piping_synced_at: {}", e))
                })?,
        })
    }
}

impl RackRoleRepository for SqlxRackRoleRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackRoleRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, color, description, owner_id,
                   tags, custom_field_data, piping_synced_at
            FROM dcim_rackrole
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load rack_role: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<RackRoleRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, color, description, owner_id,
                   tags, custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM dcim_rackrole
            "#,
        );
        push_rack_role_filters(&mut builder, search.as_deref());
        push_order_by(&mut builder, RACK_ROLE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list rack_roles: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read rack_role total_count: {}", e))
            })?,
            None => count_rack_roles(&self.pool, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<RackRoleRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_role: &NewRackRole) -> Result<RackRoleRecord, AppError> {
        let color = new_role.color.clone().unwrap_or_default();
        let description = new_role.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO dcim_rackrole (
                id, custom_field_data, name, slug, color, description, comments,
                owner_id, tags
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, '', $7, '{}')
            "#,
        )
        .bind(new_role.id)
        .bind(new_role.custom_fields.clone())
        .bind(&new_role.name)
        .bind(&new_role.slug)
        .bind(&color)
        .bind(&description)
        .bind(new_role.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create rack_role: {}", e)))?;

        Ok(RackRoleRecord {
            id: new_role.id,
            external_id: None,
            name: new_role.name.clone(),
            slug: new_role.slug.clone(),
            color: Some(color),
            description: Some(description),
            owner_id: new_role.owner_id,
            tags: Vec::new(),
            custom_fields: new_role.custom_fields.clone(),
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRackRole,
    ) -> Result<Option<RackRoleRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let color = if changes.color.is_some() {
            changes.color.clone()
        } else {
            existing.color
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };
        let custom_fields = changes.custom_fields.clone().or(existing.custom_fields);

        sqlx::query(
            r#"
            UPDATE dcim_rackrole
            SET name = $2,
                slug = $3,
                color = $4,
                description = $5,
                custom_field_data = COALESCE($6, custom_field_data)
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(color.clone().unwrap_or_default())
        .bind(description.clone().unwrap_or_default())
        .bind(changes.custom_fields.clone())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update rack_role: {}", e)))?;

        Ok(Some(RackRoleRecord {
            id,
            external_id: existing.external_id,
            name,
            slug,
            color,
            description,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewRackRole],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<RackRoleRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk rack_role insert",
            |connection, item| {
                Box::pin(async move {
                    let color = item.color.clone().unwrap_or_default();
                    let description = item.description.clone().unwrap_or_default();

                    sqlx::query(
                        r#"
                INSERT INTO dcim_rackrole (
                    id, custom_field_data, name, slug, color, description, comments,
                    owner_id, tags
                )
                VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, '', $7, '{}')
                "#,
                    )
                    .bind(item.id)
                    .bind(item.custom_fields.clone())
                    .bind(&item.name)
                    .bind(&item.slug)
                    .bind(&color)
                    .bind(&description)
                    .bind(item.owner_id)
                    .execute(connection)
                    .await
                    .map_err(|error| {
                        format!("Failed to insert rack_role '{}': {error}", item.name)
                    })?;

                    Ok(RackRoleRecord {
                        id: item.id,
                        external_id: None,
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        color: Some(color),
                        description: Some(description),
                        owner_id: item.owner_id,
                        tags: Vec::new(),
                        custom_fields: item.custom_fields.clone(),
                        piping_synced_at: None,
                    })
                })
            },
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_rackrole", id)
            .await
            .map(|_| ())
    }
}

fn push_rack_role_filters(builder: &mut QueryBuilder<'_, sqlx::Postgres>, search: Option<&str>) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "external_id",
            "name",
            "slug",
            "color",
            "description",
            "tags",
        ],
        search,
    );
}

async fn count_rack_roles(pool: &PgPool, search: Option<&str>) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_rackrole");
    push_rack_role_filters(&mut builder, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count rack_roles: {}", e)))
}
