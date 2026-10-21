//! Route target repository — full CRUD over `ipam_routetarget`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const ROUTE_TARGET_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "rt.name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "description",
        "rt.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "rt.tenant_id",
        "tenants",
        "inventory.resources.fields.tenant",
    ),
    ListColumn::sortable("tenantName", "t.name", "inventory.resources.fields.tenant"),
];

/// Representation of an `ipam_routetarget` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct RouteTargetRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `ipam_routetarget` — includes the tenant name resolved
/// via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct RouteTargetListItem {
    pub record: RouteTargetRecord,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `ipam_routetarget` row.
#[derive(Debug, Clone)]
pub struct NewRouteTarget {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_routetarget` row.
#[derive(Debug, Clone)]
pub struct UpdateRouteTarget {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tenant_id: Option<Uuid>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkRouteTargetResult {
    pub created: Vec<RouteTargetRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RouteTargetRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RouteTargetRecord>, AppError>;
    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        tenant_ids: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<RouteTargetListItem>, i64), AppError>;
    async fn create(&self, new_target: &NewRouteTarget) -> Result<RouteTargetRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRouteTarget,
    ) -> Result<Option<RouteTargetRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewRouteTarget],
    ) -> Result<BulkRouteTargetResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxRouteTargetRepository {
    pool: PgPool,
}

impl SqlxRouteTargetRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<RouteTargetRecord, AppError> {
        Ok(RouteTargetRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read route_target id: {}", e))
            })?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read route_target external_id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read route_target name: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").ok().flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read route_target owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!(
                        "Failed to read route_target piping_synced_at: {}",
                        e
                    ))
                })?,
        })
    }
}

impl RouteTargetRepository for SqlxRouteTargetRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RouteTargetRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, description, tenant_id, owner_id,
                   tags, custom_field_data, piping_synced_at
            FROM ipam_routetarget
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load route_target: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        tenant_ids: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<RouteTargetListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT rt.id, rt.external_id, rt.name, rt.description, rt.tenant_id, rt.owner_id,
                   rt.tags, rt.custom_field_data, rt.piping_synced_at,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_routetarget rt
            LEFT JOIN tenancy_tenant t ON t.id = rt.tenant_id
            "#,
        );
        push_route_target_filters(&mut builder, tenant_ids, search.as_deref());
        push_order_by(&mut builder, ROUTE_TARGET_COLUMNS, sort.as_ref(), "rt.name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list route_targets: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read route_target total_count: {}", e))
            })?,
            None => count_route_targets(&self.pool, tenant_ids, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(RouteTargetListItem {
                    record: Self::parse_row(row)?,
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<RouteTargetListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_target: &NewRouteTarget) -> Result<RouteTargetRecord, AppError> {
        let description = new_target.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_routetarget (
                id, custom_field_data, name, description, tenant_id, comments,
                owner_id, tags
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, '', $5, '{}')
            "#,
        )
        .bind(new_target.id)
        .bind(&new_target.name)
        .bind(&description)
        .bind(new_target.tenant_id)
        .bind(new_target.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create route_target: {}", e)))?;

        Ok(RouteTargetRecord {
            id: new_target.id,
            external_id: None,
            name: new_target.name.clone(),
            description: Some(description),
            tenant_id: new_target.tenant_id,
            owner_id: new_target.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRouteTarget,
    ) -> Result<Option<RouteTargetRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };

        sqlx::query(
            r#"
            UPDATE ipam_routetarget
            SET name = $2, description = $3, tenant_id = $4
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(description.clone().unwrap_or_default())
        .bind(tenant_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update route_target: {}", e)))?;

        Ok(Some(RouteTargetRecord {
            id,
            external_id: existing.external_id,
            name,
            description,
            tenant_id,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewRouteTarget],
    ) -> Result<BulkRouteTargetResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let description = item.description.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO ipam_routetarget (
                    id, custom_field_data, name, description, tenant_id, comments,
                    owner_id, tags
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, '', $5, '{}')
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&description)
                .bind(item.tenant_id)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(RouteTargetRecord {
                    id: item.id,
                    external_id: None,
                    name: item.name.clone(),
                    description: Some(description),
                    tenant_id: item.tenant_id,
                    owner_id: item.owner_id,
                    tags: Vec::new(),
                    custom_fields: None,
                    piping_synced_at: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert route_target '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk route_target insert: {}", e))
        })?;

        Ok(BulkRouteTargetResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_routetarget", id)
            .await
            .map(|_| ())
    }
}

fn push_route_target_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    tenant_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_uuid_in(builder, &mut where_state, "rt.tenant_id", tenant_ids);
    push_search_ilike(
        builder,
        &mut where_state,
        &["rt.name", "rt.description", "t.name"],
        search,
    );
}

async fn count_route_targets(
    pool: &PgPool,
    tenant_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new(
        "SELECT COUNT(*) FROM ipam_routetarget rt LEFT JOIN tenancy_tenant t ON t.id = rt.tenant_id",
    );
    push_route_target_filters(&mut builder, tenant_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count route_targets: {}", e)))
}
