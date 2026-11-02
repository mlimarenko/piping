//! Platform repository — full CRUD.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::super::super::SortSpec;
use crate::contexts::shared::list_query::{ListColumn, WhereState, push_order_by, push_uuid_in};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const PLATFORM_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "p.name", "inventory.platforms.columns.name"),
    ListColumn::sortable("slug", "p.slug", "inventory.platforms.columns.slug"),
    ListColumn::fk_filter(
        "manufacturer",
        "m.name",
        "manufacturer_id",
        "p.manufacturer_id",
        "manufacturers",
        "inventory.platforms.columns.manufacturer",
    ),
    ListColumn::sortable(
        "napalm_driver",
        "p.napalm_driver",
        "inventory.platforms.columns.napalmDriver",
    ),
    ListColumn::sortable(
        "description",
        "p.description",
        "inventory.platforms.columns.description",
    ),
];

/// Minimal representation of a `dcim_platform` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct PlatformRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<Uuid>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

/// Hydrated list row for `dcim_platform` — includes the manufacturer name
/// resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct PlatformListItem {
    pub record: PlatformRecord,
    pub manufacturer_name: Option<String>,
}

/// Input for creating a new `dcim_platform` row.
#[derive(Debug, Clone)]
pub struct NewPlatform {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<Uuid>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

/// Fields that may be updated on an existing `dcim_platform` row.
#[derive(Debug, Clone)]
pub struct UpdatePlatform {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub manufacturer_id: Option<Uuid>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait PlatformRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PlatformRecord>, AppError>;
    async fn list_hydrated(
        &self,
        manufacturer_id: Vec<Uuid>,
        search: Option<String>,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PlatformListItem>, i64), AppError>;
    async fn create(&self, new_platform: &NewPlatform) -> Result<PlatformRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePlatform,
    ) -> Result<Option<PlatformRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewPlatform],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<PlatformRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxPlatformRepository {
    pool: PgPool,
}

impl SqlxPlatformRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<PlatformRecord, AppError> {
        Ok(PlatformRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read platform id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read platform name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read platform slug: {}", e)))?,
            manufacturer_id: row
                .try_get::<Option<Uuid>, _>("manufacturer_id")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read platform manufacturer_id: {}", e))
                })?,
            napalm_driver: row
                .try_get::<Option<String>, _>("napalm_driver")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read platform napalm_driver: {}", e))
                })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read platform description: {}", e))
                })?,
        })
    }

    fn push_list_filters<'a>(
        builder: &mut QueryBuilder<'a, Postgres>,
        search_pattern: Option<&'a str>,
        manufacturer_id: &[Uuid],
    ) {
        let mut where_state = WhereState::new();
        push_uuid_in(
            builder,
            &mut where_state,
            "p.manufacturer_id",
            manufacturer_id,
        );
        if let Some(search_pattern) = search_pattern {
            where_state.and(builder);
            builder.push("(");
            builder.push_bind(search_pattern);
            builder.push("::text IS NULL OR p.name ILIKE ");
            builder.push_bind(search_pattern);
            builder.push(" OR p.slug ILIKE ");
            builder.push_bind(search_pattern);
            builder.push(")");
        }
    }
}

impl PlatformRepository for SqlxPlatformRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<PlatformRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, manufacturer_id, napalm_driver, description
            FROM dcim_platform
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load platform: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_hydrated(
        &self,
        manufacturer_id: Vec<Uuid>,
        search: Option<String>,
        sort: Option<SortSpec>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<PlatformListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let search_pattern = search.as_deref().map(|s| format!("%{}%", s));
        let mut builder = QueryBuilder::new(
            r#"
            SELECT p.id, p.name, p.slug, p.manufacturer_id, p.napalm_driver, p.description,
                   m.name AS manufacturer_name,
                   COUNT(*) OVER() AS total_count
            FROM dcim_platform p
            LEFT JOIN dcim_manufacturer m ON m.id = p.manufacturer_id
            "#,
        );
        Self::push_list_filters(&mut builder, search_pattern.as_deref(), &manufacturer_id);
        push_order_by(&mut builder, PLATFORM_COLUMNS, sort.as_ref(), "p.name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to list platforms hydrated: {}", e))
            })?;

        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total_count"))
            .transpose()
            .map_err(|e| AppError::Database(format!("Failed to read platform total_count: {}", e)))?
            .unwrap_or(0);
        let items = rows
            .iter()
            .map(|row| {
                Ok::<PlatformListItem, AppError>(PlatformListItem {
                    record: Self::parse_row(row)?,
                    manufacturer_name: row.try_get("manufacturer_name").ok(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_platform: &NewPlatform) -> Result<PlatformRecord, AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_platform (
                id, name, slug, manufacturer_id, napalm_driver, description,
                tags, custom_field_data, comments, level, lft, rght, tree_id
            )
            VALUES ($1, $2, $3, $4, $5, COALESCE($6, ''), '{}', '{}'::jsonb, '', 0, 0, 0, 0)
            "#,
        )
        .bind(new_platform.id)
        .bind(&new_platform.name)
        .bind(&new_platform.slug)
        .bind(new_platform.manufacturer_id)
        .bind(new_platform.napalm_driver.as_deref())
        .bind(new_platform.description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create platform: {}", e)))?;

        Ok(PlatformRecord {
            id: new_platform.id,
            name: new_platform.name.clone(),
            slug: new_platform.slug.clone(),
            manufacturer_id: new_platform.manufacturer_id,
            napalm_driver: new_platform.napalm_driver.clone(),
            description: new_platform.description.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdatePlatform,
    ) -> Result<Option<PlatformRecord>, AppError> {
        let existing = self.get_by_id(id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let manufacturer_id = if changes.manufacturer_id.is_some() {
            changes.manufacturer_id
        } else {
            existing.manufacturer_id
        };
        let napalm_driver = if changes.napalm_driver.is_some() {
            changes.napalm_driver.clone()
        } else {
            existing.napalm_driver
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE dcim_platform
            SET name = $2, slug = $3, manufacturer_id = $4, napalm_driver = $5, description = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(manufacturer_id)
        .bind(napalm_driver.as_deref())
        .bind(description.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update platform: {}", e)))?;

        Ok(Some(PlatformRecord {
            id,
            name,
            slug,
            manufacturer_id,
            napalm_driver,
            description,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewPlatform],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<PlatformRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk platform insert",
            |connection, item| {
                Box::pin(async move {
                    sqlx::query(
                        r#"
                INSERT INTO dcim_platform (
                    id, name, slug, manufacturer_id, napalm_driver, description,
                    tags, custom_field_data, comments, level, lft, rght, tree_id
                )
                VALUES ($1, $2, $3, $4, $5, COALESCE($6, ''), '{}', '{}'::jsonb, '', 0, 0, 0, 0)
                "#,
                    )
                    .bind(item.id)
                    .bind(&item.name)
                    .bind(&item.slug)
                    .bind(item.manufacturer_id)
                    .bind(item.napalm_driver.as_deref())
                    .bind(item.description.as_deref())
                    .execute(connection)
                    .await
                    .map_err(|error| {
                        format!("Failed to insert platform '{}': {error}", item.name)
                    })?;

                    Ok(PlatformRecord {
                        id: item.id,
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        manufacturer_id: item.manufacturer_id,
                        napalm_driver: item.napalm_driver.clone(),
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
        let deleted =
            super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_platform", id)
                .await?;
        if deleted == 0 {
            return Err(AppError::NotFound(format!("Platform '{id}' not found")));
        }
        Ok(())
    }
}
