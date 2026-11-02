//! IPAM role repository — full CRUD over `ipam_role`.
//!
//! The shared prefix/VLAN role (distinct from DCIM `device_role`).
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

/// NetBox default ordering weight for a freshly-created role.
const DEFAULT_WEIGHT: i16 = 1000;

pub const IPAM_ROLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.ipamRole.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.ipamRole.columns.slug"),
    ListColumn::sortable("weight", "weight", "inventory.ipamRole.columns.weight"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.ipamRole.columns.description",
    ),
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.ipamRole.columns.externalId",
    ),
];

/// Representation of an `ipam_role` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct IpamRoleRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub weight: i16,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for creating a new `ipam_role` row.
#[derive(Debug, Clone)]
pub struct NewIpamRole {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub weight: Option<i16>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_role` row.
#[derive(Debug, Clone)]
pub struct UpdateIpamRole {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub weight: Option<i16>,
    pub description: Option<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait IpamRoleRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<IpamRoleRecord>, AppError>;
    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<IpamRoleRecord>, i64), AppError>;
    async fn create(&self, new_role: &NewIpamRole) -> Result<IpamRoleRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateIpamRole,
    ) -> Result<Option<IpamRoleRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewIpamRole],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<IpamRoleRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxIpamRoleRepository {
    pool: PgPool,
}

impl SqlxIpamRoleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<IpamRoleRecord, AppError> {
        Ok(IpamRoleRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read ipam_role id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_role external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read ipam_role name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read ipam_role slug: {}", e)))?,
            weight: row.try_get("weight").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_role weight: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_role owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read ipam_role piping_synced_at: {}", e))
                })?,
        })
    }
}

impl IpamRoleRepository for SqlxIpamRoleRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<IpamRoleRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, weight, description, owner_id,
                   tags, custom_field_data, piping_synced_at
            FROM ipam_role
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load ipam_role: {}", e)))?;

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
    ) -> Result<(Vec<IpamRoleRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, weight, description, owner_id,
                   tags, custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM ipam_role
            "#,
        );
        push_ipam_role_filters(&mut builder, search.as_deref());
        push_order_by(&mut builder, IPAM_ROLE_COLUMNS, sort.as_ref(), "weight");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list ipam_roles: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read ipam_role total_count: {}", e))
            })?,
            None => count_ipam_roles(&self.pool, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<IpamRoleRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_role: &NewIpamRole) -> Result<IpamRoleRecord, AppError> {
        let weight = new_role.weight.unwrap_or(DEFAULT_WEIGHT);
        let description = new_role.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_role (
                id, custom_field_data, name, slug, weight, description, comments,
                owner_id, tags
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, '', $6, '{}')
            "#,
        )
        .bind(new_role.id)
        .bind(&new_role.name)
        .bind(&new_role.slug)
        .bind(weight)
        .bind(&description)
        .bind(new_role.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create ipam_role: {}", e)))?;

        Ok(IpamRoleRecord {
            id: new_role.id,
            external_id: None,
            name: new_role.name.clone(),
            slug: new_role.slug.clone(),
            weight,
            description: Some(description),
            owner_id: new_role.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateIpamRole,
    ) -> Result<Option<IpamRoleRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let weight = changes.weight.unwrap_or(existing.weight);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE ipam_role
            SET name = $2, slug = $3, weight = $4, description = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(weight)
        .bind(description.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update ipam_role: {}", e)))?;

        Ok(Some(IpamRoleRecord {
            id,
            external_id: existing.external_id,
            name,
            slug,
            weight,
            description,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewIpamRole],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<IpamRoleRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk ipam_role insert",
            |connection, item| {
                Box::pin(async move {
                    let weight = item.weight.unwrap_or(DEFAULT_WEIGHT);
                    let description = item.description.clone().unwrap_or_default();

                    sqlx::query(
                        r#"
                INSERT INTO ipam_role (
                    id, custom_field_data, name, slug, weight, description, comments,
                    owner_id, tags
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, '', $6, '{}')
                "#,
                    )
                    .bind(item.id)
                    .bind(&item.name)
                    .bind(&item.slug)
                    .bind(weight)
                    .bind(&description)
                    .bind(item.owner_id)
                    .execute(connection)
                    .await
                    .map_err(|error| {
                        format!("Failed to insert ipam_role '{}': {error}", item.name)
                    })?;

                    Ok(IpamRoleRecord {
                        id: item.id,
                        external_id: None,
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        weight,
                        description: Some(description),
                        owner_id: item.owner_id,
                        tags: Vec::new(),
                        custom_fields: None,
                        piping_synced_at: None,
                    })
                })
            },
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_role", id)
            .await
            .map(|_| ())
    }
}

fn push_ipam_role_filters(builder: &mut QueryBuilder<'_, sqlx::Postgres>, search: Option<&str>) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "external_id",
            "name",
            "slug",
            "weight",
            "description",
            "tags",
        ],
        search,
    );
}

async fn count_ipam_roles(pool: &PgPool, search: Option<&str>) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM ipam_role");
    push_ipam_role_filters(&mut builder, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count ipam_roles: {e}")))
}
