//! Inventory item role repository — full CRUD over `dcim_inventoryitemrole`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const INVENTORY_ITEM_ROLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.inventoryItemRoles.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.inventoryItemRoles.columns.slug"),
    ListColumn::sortable(
        "color",
        "color",
        "inventory.inventoryItemRoles.columns.color",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.inventoryItemRoles.columns.description",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.inventoryItemRoles.columns.owner",
    ),
];

/// Representation of a `dcim_inventoryitemrole` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct InventoryItemRoleRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `dcim_inventoryitemrole` row.
#[derive(Debug, Clone)]
pub struct NewInventoryItemRole {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Fields that may be updated on an existing `dcim_inventoryitemrole` row.
#[derive(Debug, Clone)]
pub struct UpdateInventoryItemRole {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkInventoryItemRoleResult {
    pub created: Vec<InventoryItemRoleRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait InventoryItemRoleRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InventoryItemRoleRecord>, AppError>;
    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        owner_id: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<InventoryItemRoleRecord>, i64), AppError>;
    async fn create(
        &self,
        new_role: &NewInventoryItemRole,
    ) -> Result<InventoryItemRoleRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInventoryItemRole,
    ) -> Result<Option<InventoryItemRoleRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewInventoryItemRole],
    ) -> Result<BulkInventoryItemRoleResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxInventoryItemRoleRepository {
    pool: PgPool,
}

impl SqlxInventoryItemRoleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<InventoryItemRoleRecord, AppError> {
        Ok(InventoryItemRoleRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item_role id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item_role name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read inventory_item_role slug: {}", e))
            })?,
            color: row.try_get::<Option<String>, _>("color").ok().flatten(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read inventory_item_role owner_id: {}",
                    e
                ))
            })?,
            owner_name: row
                .try_get::<Option<String>, _>("owner_name")
                .ok()
                .flatten(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, Postgres>,
        owner_id: &[Uuid],
        search: Option<&str>,
    ) {
        let mut where_state = WhereState::new();
        push_uuid_in(builder, &mut where_state, "owner_id", owner_id);
        push_search_ilike(
            builder,
            &mut where_state,
            &["name", "slug", "color", "description"],
            search,
        );
    }
}

impl InventoryItemRoleRepository for SqlxInventoryItemRoleRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<InventoryItemRoleRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, color, description, owner_id, NULL::text AS owner_name,
                   custom_field_data
            FROM dcim_inventoryitemrole
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load inventory_item_role: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        owner_id: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<InventoryItemRoleRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            "
            SELECT id, name, slug, color, description, owner_id, owner_name, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT iir.id, iir.name, iir.slug, iir.color, iir.description,
                       iir.owner_id, COALESCE(NULLIF(u.username, ''), u.email) AS owner_name,
                       iir.custom_field_data
                FROM dcim_inventoryitemrole iir
                LEFT JOIN identity_user u ON u.id = iir.owner_id
            ) inventory_item_role_rows
            ",
        );
        Self::push_list_filters(&mut builder, owner_id, search.as_deref());
        push_order_by(
            &mut builder,
            INVENTORY_ITEM_ROLE_COLUMNS,
            sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list inventory_item_roles: {}", e))
        })?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read inventory_item_role total_count: {}",
                    e
                ))
            })?,
            None => {
                let mut count_builder =
                    QueryBuilder::new("SELECT COUNT(*) FROM dcim_inventoryitemrole");
                Self::push_list_filters(&mut count_builder, owner_id, search.as_deref());
                count_builder
                    .build_query_scalar::<i64>()
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| {
                        AppError::Database(format!("Failed to count inventory_item_roles: {}", e))
                    })?
            }
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<InventoryItemRoleRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(
        &self,
        new_role: &NewInventoryItemRole,
    ) -> Result<InventoryItemRoleRecord, AppError> {
        let color = new_role.color.clone().unwrap_or_default();
        let description = new_role.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO dcim_inventoryitemrole (
                id, custom_field_data, name, slug, color, description, comments,
                owner_id
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, '', $7)
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
        .map_err(|e| AppError::Database(format!("Failed to create inventory_item_role: {}", e)))?;

        Ok(InventoryItemRoleRecord {
            id: new_role.id,
            name: new_role.name.clone(),
            slug: new_role.slug.clone(),
            color: Some(color),
            description: Some(description),
            owner_id: new_role.owner_id,
            owner_name: None,
            custom_fields: new_role.custom_fields.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateInventoryItemRole,
    ) -> Result<Option<InventoryItemRoleRecord>, AppError> {
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
            UPDATE dcim_inventoryitemrole
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
        .map_err(|e| AppError::Database(format!("Failed to update inventory_item_role: {}", e)))?;

        Ok(Some(InventoryItemRoleRecord {
            id,
            name,
            slug,
            color,
            description,
            owner_id: existing.owner_id,
            owner_name: existing.owner_name,
            custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewInventoryItemRole],
    ) -> Result<BulkInventoryItemRoleResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let color = item.color.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_inventoryitemrole (
                    id, custom_field_data, name, slug, color, description, comments,
                    owner_id
                )
                VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, '', $7)
                "#,
                )
                .bind(item.id)
                .bind(item.custom_fields.clone())
                .bind(&item.name)
                .bind(&item.slug)
                .bind(&color)
                .bind(&description)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(InventoryItemRoleRecord {
                    id: item.id,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    color: Some(color),
                    description: Some(description),
                    owner_id: item.owner_id,
                    owner_name: None,
                    custom_fields: item.custom_fields.clone(),
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert inventory_item_role '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!(
                "Failed to commit bulk inventory_item_role insert: {}",
                e
            ))
        })?;

        Ok(BulkInventoryItemRoleResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_inventoryitemrole",
            id,
        )
        .await
        .map(|_| ())
    }
}
