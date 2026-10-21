//! Location repository — full CRUD over `dcim_location`.
//!
//! The location hierarchy is read via `parent_id` + a Postgres recursive CTE
//! (`list_descendants`); the Django-MPTT columns (`lft`, `rght`, `tree_id`,
//! `level`) are never read. They are NOT NULL in the NetBox-derived schema, so
//! inserts write neutral `0` placeholders to satisfy the constraints without
//! taking a dependency on MPTT bookkeeping.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

/// Representation of a `dcim_location` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct LocationRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub site_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub status: String,
    pub tenant_id: Option<Uuid>,
    pub facility: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `dcim_location` — includes site/parent/tenant names
/// resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct LocationListItem {
    pub record: LocationRecord,
    pub site_name: Option<String>,
    pub parent_name: Option<String>,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `dcim_location` row.
#[derive(Debug, Clone)]
pub struct NewLocation {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub site_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub status: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub facility: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `dcim_location` row.
#[derive(Debug, Clone)]
pub struct UpdateLocation {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub site_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub status: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub facility: Option<String>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkLocationResult {
    pub created: Vec<LocationRecord>,
    pub errors: Vec<String>,
}

/// Filters, search and sort of one `list` call.
#[derive(Debug, Default)]
pub struct LocationFilter<'a> {
    pub site_ids: &'a [Uuid],
    pub parent_ids: &'a [Uuid],
    pub tenant_ids: &'a [Uuid],
    pub status: &'a [String],
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait LocationRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<LocationRecord>, AppError>;
    async fn list(
        &self,
        filter: LocationFilter<'_>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<LocationListItem>, i64), AppError>;
    /// Returns the location and all of its descendants via a recursive CTE over
    /// `parent_id`. Used for subtree queries without MPTT columns.
    async fn list_descendants(&self, root_id: Uuid) -> Result<Vec<LocationRecord>, AppError>;
    async fn create(&self, new_location: &NewLocation) -> Result<LocationRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateLocation,
    ) -> Result<Option<LocationRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewLocation]) -> Result<BulkLocationResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxLocationRepository {
    pool: PgPool,
}

const LOCATION_COLUMNS: &str = r#"
    id, external_id, name, slug, site_id, parent_id, status, tenant_id, facility,
    description, owner_id, tags, custom_field_data, piping_synced_at
"#;

/// Same columns as `LOCATION_COLUMNS` but qualified with the `l` table alias,
/// for use in the hydrated list query that LEFT JOINs site/parent/tenant.
const LOCATION_COLUMNS_QUALIFIED: &str = r#"
    l.id, l.external_id, l.name, l.slug, l.site_id, l.parent_id, l.status,
    l.tenant_id, l.facility, l.description, l.owner_id, l.tags,
    l.custom_field_data, l.piping_synced_at
"#;

pub const LOCATION_LIST_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "l.external_id",
        "inventory.locations.columns.externalId",
    ),
    ListColumn::sortable("name", "l.name", "inventory.locations.columns.name"),
    ListColumn::sortable("slug", "l.slug", "inventory.locations.columns.slug"),
    ListColumn::fk_filter(
        "site",
        "s.name",
        "site_id",
        "l.site_id",
        "sites",
        "inventory.locations.columns.site",
    ),
    ListColumn::fk_filter(
        "parent",
        "pl.name",
        "parent_id",
        "l.parent_id",
        "locations",
        "inventory.locations.columns.parent",
    ),
    ListColumn::enum_filter(
        "status",
        "l.status",
        "l.status",
        "inventory.locations.columns.status",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "l.tenant_id",
        "tenants",
        "inventory.locations.columns.tenant",
    ),
    ListColumn::sortable(
        "facility",
        "l.facility",
        "inventory.locations.columns.facility",
    ),
    ListColumn::sortable(
        "description",
        "l.description",
        "inventory.locations.columns.description",
    ),
    ListColumn::sortable("tags", "l.tags", "inventory.locations.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "l.piping_synced_at",
        "inventory.locations.columns.syncedAt",
    ),
];

impl SqlxLocationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<LocationRecord, AppError> {
        Ok(LocationRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read location id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read location external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read location name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read location slug: {}", e)))?,
            site_id: row.try_get("site_id").map_err(|e| {
                AppError::Database(format!("Failed to read location site_id: {}", e))
            })?,
            parent_id: row.try_get::<Option<Uuid>, _>("parent_id").map_err(|e| {
                AppError::Database(format!("Failed to read location parent_id: {}", e))
            })?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!("Failed to read location status: {}", e))
            })?,
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read location tenant_id: {}", e))
            })?,
            facility: row.try_get::<Option<String>, _>("facility").ok().flatten(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read location owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read location piping_synced_at: {}", e))
                })?,
        })
    }
}

impl LocationRepository for SqlxLocationRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<LocationRecord>, AppError> {
        let sql = format!("SELECT {LOCATION_COLUMNS} FROM dcim_location WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load location: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: LocationFilter<'_>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<LocationListItem>, i64), AppError> {
        let LocationFilter {
            site_ids,
            parent_ids,
            tenant_ids,
            status,
            search,
            sort,
        } = filter;
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(format!(
            r#"
            SELECT {LOCATION_COLUMNS_QUALIFIED},
                   s.name AS site_name,
                   pl.name AS parent_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM dcim_location l
            LEFT JOIN dcim_site s ON s.id = l.site_id
            LEFT JOIN dcim_location pl ON pl.id = l.parent_id
            LEFT JOIN tenancy_tenant t ON t.id = l.tenant_id
            "#,
        ));
        let filter = LocationListFilter {
            site_ids,
            parent_ids,
            tenant_ids,
            status,
            alias: "l.",
            search: search.as_deref(),
        };
        push_location_filters(&mut builder, &filter);
        push_order_by(&mut builder, LOCATION_LIST_COLUMNS, sort.as_ref(), "l.name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list locations: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read location total_count: {}", e))
            })?,
            None => {
                let mut count_builder = QueryBuilder::new(
                    r#"
                    SELECT COUNT(*) FROM dcim_location l
                    LEFT JOIN dcim_site s ON s.id = l.site_id
                    LEFT JOIN dcim_location pl ON pl.id = l.parent_id
                    LEFT JOIN tenancy_tenant t ON t.id = l.tenant_id
                    "#,
                );
                push_location_filters(&mut count_builder, &filter);
                count_builder
                    .build_query_scalar::<i64>()
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| AppError::Database(format!("Failed to count locations: {}", e)))?
            }
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(LocationListItem {
                    record: Self::parse_row(row)?,
                    site_name: row.try_get("site_name").ok(),
                    parent_name: row.try_get("parent_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<LocationListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn list_descendants(&self, root_id: Uuid) -> Result<Vec<LocationRecord>, AppError> {
        // Recursive CTE over parent_id — the MPTT-free tree walk.
        let sql = format!(
            r#"
            WITH RECURSIVE subtree AS (
                SELECT {LOCATION_COLUMNS}
                FROM dcim_location
                WHERE id = $1
                UNION ALL
                SELECT {child_columns}
                FROM dcim_location AS child
                JOIN subtree ON child.parent_id = subtree.id
            )
            SELECT {LOCATION_COLUMNS} FROM subtree
            ORDER BY name
            "#,
            child_columns = LOCATION_COLUMNS
                .split(',')
                .map(|col| format!("child.{}", col.trim()))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let rows = sqlx::query(&sql)
            .bind(root_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list location subtree: {}", e)))?;

        rows.iter().map(Self::parse_row).collect()
    }

    async fn create(&self, new_location: &NewLocation) -> Result<LocationRecord, AppError> {
        let status = new_location
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("active")
            .to_string();
        let facility = new_location.facility.clone().unwrap_or_default();
        let description = new_location.description.clone().unwrap_or_default();

        // lft/rght/tree_id/level are NOT NULL Django-MPTT bookkeeping columns we
        // never read; neutral 0 placeholders satisfy the CHECK (>= 0) constraints.
        sqlx::query(
            r#"
            INSERT INTO dcim_location (
                id, custom_field_data, name, slug, description, lft, rght, tree_id,
                level, parent_id, site_id, tenant_id, status, facility, comments,
                owner_id, tags
            )
            VALUES (
                $1, '{}'::jsonb, $2, $3, $4, 0, 0, 0, 0, $5, $6, $7, $8, $9, '', $10, '{}'
            )
            "#,
        )
        .bind(new_location.id)
        .bind(&new_location.name)
        .bind(&new_location.slug)
        .bind(&description)
        .bind(new_location.parent_id)
        .bind(new_location.site_id)
        .bind(new_location.tenant_id)
        .bind(&status)
        .bind(&facility)
        .bind(new_location.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create location: {}", e)))?;

        Ok(LocationRecord {
            id: new_location.id,
            external_id: None,
            name: new_location.name.clone(),
            slug: new_location.slug.clone(),
            site_id: new_location.site_id,
            parent_id: new_location.parent_id,
            status,
            tenant_id: new_location.tenant_id,
            facility: Some(facility),
            description: Some(description),
            owner_id: new_location.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateLocation,
    ) -> Result<Option<LocationRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let site_id = changes.site_id.unwrap_or(existing.site_id);
        let status = changes.status.clone().unwrap_or(existing.status);
        let parent_id = if changes.parent_id.is_some() {
            changes.parent_id
        } else {
            existing.parent_id
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let facility = if changes.facility.is_some() {
            changes.facility.clone()
        } else {
            existing.facility
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE dcim_location
            SET name = $2, slug = $3, site_id = $4, parent_id = $5, status = $6,
                tenant_id = $7, facility = $8, description = $9
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(site_id)
        .bind(parent_id)
        .bind(&status)
        .bind(tenant_id)
        .bind(facility.clone().unwrap_or_default())
        .bind(description.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update location: {}", e)))?;

        Ok(Some(LocationRecord {
            id,
            external_id: existing.external_id,
            name,
            slug,
            site_id,
            parent_id,
            status,
            tenant_id,
            facility,
            description,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(&self, items: &[NewLocation]) -> Result<BulkLocationResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let status = item
                .status
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("active")
                .to_string();
            let facility = item.facility.clone().unwrap_or_default();
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_location (
                    id, custom_field_data, name, slug, description, lft, rght, tree_id,
                    level, parent_id, site_id, tenant_id, status, facility, comments,
                    owner_id, tags
                )
                VALUES (
                    $1, '{}'::jsonb, $2, $3, $4, 0, 0, 0, 0, $5, $6, $7, $8, $9, '', $10, '{}'
                )
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&item.slug)
                .bind(&description)
                .bind(item.parent_id)
                .bind(item.site_id)
                .bind(item.tenant_id)
                .bind(&status)
                .bind(&facility)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(LocationRecord {
                    id: item.id,
                    external_id: None,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    site_id: item.site_id,
                    parent_id: item.parent_id,
                    status,
                    tenant_id: item.tenant_id,
                    facility: Some(facility),
                    description: Some(description),
                    owner_id: item.owner_id,
                    tags: Vec::new(),
                    custom_fields: None,
                    piping_synced_at: None,
                }),
                Err(e) => errors.push(format!("Failed to insert location '{}': {}", item.name, e)),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk location insert: {}", e))
        })?;

        Ok(BulkLocationResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_location", id)
            .await
            .map(|_| ())
    }
}

/// Borrowed location list filter shared by the page query and its count query.
struct LocationListFilter<'a> {
    site_ids: &'a [Uuid],
    parent_ids: &'a [Uuid],
    tenant_ids: &'a [Uuid],
    status: &'a [String],
    alias: &'static str,
    search: Option<&'a str>,
}

fn push_location_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    filter: &LocationListFilter<'_>,
) {
    let &LocationListFilter {
        site_ids,
        parent_ids,
        tenant_ids,
        status,
        alias,
        search,
    } = filter;
    let mut where_state = WhereState::new();

    let site_column = format!("{alias}site_id");
    push_uuid_in(builder, &mut where_state, &site_column, site_ids);
    let parent_column = format!("{alias}parent_id");
    push_uuid_in(builder, &mut where_state, &parent_column, parent_ids);
    let tenant_column = format!("{alias}tenant_id");
    push_uuid_in(builder, &mut where_state, &tenant_column, tenant_ids);
    let status_column = format!("{alias}status");
    push_string_in(builder, &mut where_state, &status_column, status);
    let search_columns: &[&str] = if alias == "l." {
        &[
            "l.external_id",
            "l.name",
            "l.slug",
            "l.status",
            "l.facility",
            "l.description",
            "l.tags",
            "s.name",
            "pl.name",
            "t.name",
        ]
    } else {
        &[
            "external_id",
            "name",
            "slug",
            "status",
            "facility",
            "description",
            "tags",
        ]
    };
    push_search_ilike(builder, &mut where_state, search_columns, search);
}
