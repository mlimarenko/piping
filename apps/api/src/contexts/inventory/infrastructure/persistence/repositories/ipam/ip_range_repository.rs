//! IP range repository — full CRUD over `ipam_iprange`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! INET columns (`start_address`, `end_address`) are read via `::text` cast and
//! written via `$N::inet` cast, matching the approach used by `prefix_repository`
//! for `cidr` columns.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const IP_RANGE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "start_address",
        "ir.start_address",
        "inventory.resources.fields.startAddress",
    ),
    ListColumn::sortable(
        "end_address",
        "ir.end_address",
        "inventory.resources.fields.endAddress",
    ),
    ListColumn::sortable("size", "ir.size", "inventory.resources.fields.size"),
    ListColumn::enum_filter(
        "status",
        "ir.status",
        "ir.status",
        "inventory.resources.fields.status",
    ),
    ListColumn::fk_filter(
        "vrf",
        "v.name",
        "vrf_id",
        "ir.vrf_id",
        "vrfs",
        "inventory.resources.fields.vrf",
    ),
    ListColumn::sortable("vrfName", "v.name", "inventory.resources.fields.vrf"),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "ir.tenant_id",
        "tenants",
        "inventory.resources.fields.tenant",
    ),
    ListColumn::sortable("tenantName", "t.name", "inventory.resources.fields.tenant"),
    ListColumn::sortable(
        "description",
        "ir.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::enum_filter(
        "mark_utilized",
        "ir.mark_utilized",
        "ir.mark_utilized::text",
        "inventory.resources.fields.markUtilized",
    ),
    ListColumn::enum_filter(
        "mark_populated",
        "ir.mark_populated",
        "ir.mark_populated::text",
        "inventory.resources.fields.markPopulated",
    ),
];

/// Representation of an `ipam_iprange` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct IpRangeRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub start_address: String,
    pub end_address: String,
    pub size: i32,
    pub status: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: bool,
    pub mark_populated: bool,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `ipam_iprange` — includes vrf/tenant names resolved via
/// LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct IpRangeListItem {
    pub record: IpRangeRecord,
    pub vrf_name: Option<String>,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `ipam_iprange` row.
#[derive(Debug, Clone)]
pub struct NewIpRange {
    pub id: Uuid,
    pub start_address: String,
    pub end_address: String,
    pub status: Option<String>,
    pub description: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: Option<bool>,
    pub mark_populated: Option<bool>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_iprange` row.
#[derive(Debug, Clone)]
pub struct UpdateIpRange {
    pub start_address: Option<String>,
    pub end_address: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: Option<bool>,
    pub mark_populated: Option<bool>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkIpRangeResult {
    pub created: Vec<IpRangeRecord>,
    pub errors: Vec<String>,
}

/// IP range list filter. Empty vectors mean "no filter"; non-empty vectors
/// emit parameterized `IN` predicates. `sort` is validated by the REST layer
/// against [`IP_RANGE_COLUMNS`] before reaching the repository.
#[derive(Debug, Clone, Default)]
pub struct IpRangeFilter {
    pub search: Option<String>,
    pub parent_prefix: Option<String>,
    pub status: Vec<String>,
    pub vrf_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub mark_utilized: Vec<String>,
    pub mark_populated: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait IpRangeRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<IpRangeRecord>, AppError>;
    async fn list(
        &self,
        filter: IpRangeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<IpRangeListItem>, i64), AppError>;
    async fn create(&self, new_range: &NewIpRange) -> Result<IpRangeRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateIpRange,
    ) -> Result<Option<IpRangeRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewIpRange]) -> Result<BulkIpRangeResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxIpRangeRepository {
    pool: PgPool,
}

impl SqlxIpRangeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<IpRangeRecord, AppError> {
        Ok(IpRangeRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read ip_range id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range external_id: {}", e))
            })?,
            start_address: row.try_get("start_address").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range start_address: {}", e))
            })?,
            end_address: row.try_get("end_address").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range end_address: {}", e))
            })?,
            size: row
                .try_get("size")
                .map_err(|e| AppError::Database(format!("Failed to read ip_range size: {}", e)))?,
            status: row.try_get("status").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range status: {}", e))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            comments: row.try_get::<Option<String>, _>("comments").ok().flatten(),
            vrf_id: row.try_get::<Option<Uuid>, _>("vrf_id").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range vrf_id: {}", e))
            })?,
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range tenant_id: {}", e))
            })?,
            role_id: row.try_get::<Option<Uuid>, _>("role_id").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range role_id: {}", e))
            })?,
            mark_utilized: row.try_get("mark_utilized").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range mark_utilized: {}", e))
            })?,
            mark_populated: row.try_get("mark_populated").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range mark_populated: {}", e))
            })?,
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read ip_range piping_synced_at: {}", e))
                })?,
        })
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &IpRangeFilter) {
        let has_filter = filter.search.is_some()
            || filter.parent_prefix.is_some()
            || !filter.status.is_empty()
            || !filter.vrf_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.mark_utilized.is_empty()
            || !filter.mark_populated.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "ir.start_address",
                "ir.end_address",
                "ir.size",
                "ir.status",
                "ir.description",
            ],
            filter.search.as_deref(),
        );

        if let Some(parent_prefix) = filter.parent_prefix.clone() {
            where_state.and(builder);
            builder.push("ir.start_address <<= ");
            builder.push_bind(parent_prefix.clone());
            builder.push("::cidr AND ir.end_address <<= ");
            builder.push_bind(parent_prefix);
            builder.push("::cidr");
        }
        push_string_in(builder, &mut where_state, "ir.status", &filter.status);
        push_uuid_in(builder, &mut where_state, "ir.vrf_id", &filter.vrf_id);
        push_uuid_in(builder, &mut where_state, "ir.tenant_id", &filter.tenant_id);
        push_string_in(
            builder,
            &mut where_state,
            "ir.mark_utilized::text",
            &filter.mark_utilized,
        );
        push_string_in(
            builder,
            &mut where_state,
            "ir.mark_populated::text",
            &filter.mark_populated,
        );
    }

    async fn count_filtered(&self, filter: &IpRangeFilter) -> Result<i64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM ipam_iprange ir");
        Self::push_list_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count ip_ranges: {}", e)))
    }
}

impl IpRangeRepository for SqlxIpRangeRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<IpRangeRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id,
                   start_address::text AS start_address,
                   end_address::text   AS end_address,
                   size, status, description, comments,
                   vrf_id, tenant_id, role_id,
                   mark_utilized, mark_populated, owner_id,
                   tags, custom_field_data, piping_synced_at
            FROM ipam_iprange
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load ip_range: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: IpRangeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<IpRangeListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT ir.id, ir.external_id,
                   ir.start_address::text AS start_address,
                   ir.end_address::text   AS end_address,
                   ir.size, ir.status, ir.description, ir.comments,
                   ir.vrf_id, ir.tenant_id, ir.role_id,
                   ir.mark_utilized, ir.mark_populated, ir.owner_id,
                   ir.tags, ir.custom_field_data, ir.piping_synced_at,
                   v.name AS vrf_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_iprange ir
            LEFT JOIN ipam_vrf v ON v.id = ir.vrf_id
            LEFT JOIN tenancy_tenant t ON t.id = ir.tenant_id
            "#,
        );
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            IP_RANGE_COLUMNS,
            filter.sort.as_ref(),
            "ir.start_address",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list ip_ranges: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read ip_range total_count: {}", e))
            })?,
            None => self.count_filtered(&filter).await?,
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(IpRangeListItem {
                    record: Self::parse_row(row)?,
                    vrf_name: row.try_get("vrf_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<IpRangeListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_range: &NewIpRange) -> Result<IpRangeRecord, AppError> {
        let status = new_range
            .status
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("active")
            .to_string();
        let mark_utilized = new_range.mark_utilized.unwrap_or(false);
        let mark_populated = new_range.mark_populated.unwrap_or(false);

        sqlx::query(
            r#"
            INSERT INTO ipam_iprange (
                id, custom_field_data,
                start_address, end_address, size,
                status, description, comments,
                vrf_id, tenant_id, role_id,
                mark_utilized, mark_populated, owner_id, tags
            )
            VALUES (
                $1, '{}'::jsonb,
                $2::inet, $3::inet,
                (($3::inet - $2::inet) + 1),
                $4, $5, '',
                $6, $7, $8,
                $9, $10, $11, '{}'
            )
            "#,
        )
        .bind(new_range.id)
        .bind(&new_range.start_address)
        .bind(&new_range.end_address)
        .bind(&status)
        .bind(new_range.description.as_deref())
        .bind(new_range.vrf_id)
        .bind(new_range.tenant_id)
        .bind(new_range.role_id)
        .bind(mark_utilized)
        .bind(mark_populated)
        .bind(new_range.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create ip_range: {}", e)))?;

        // Re-fetch to get the database-computed size value.
        self.get_by_id(new_range.id)
            .await?
            .ok_or_else(|| AppError::Database("ip_range disappeared after insert".to_string()))
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateIpRange,
    ) -> Result<Option<IpRangeRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let start_address = changes
            .start_address
            .clone()
            .unwrap_or(existing.start_address);
        let end_address = changes.end_address.clone().unwrap_or(existing.end_address);
        let status = changes.status.clone().unwrap_or(existing.status);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };
        let vrf_id = if changes.vrf_id.is_some() {
            changes.vrf_id
        } else {
            existing.vrf_id
        };
        let tenant_id = if changes.tenant_id.is_some() {
            changes.tenant_id
        } else {
            existing.tenant_id
        };
        let role_id = if changes.role_id.is_some() {
            changes.role_id
        } else {
            existing.role_id
        };
        let mark_utilized = changes.mark_utilized.unwrap_or(existing.mark_utilized);
        let mark_populated = changes.mark_populated.unwrap_or(existing.mark_populated);

        sqlx::query(
            r#"
            UPDATE ipam_iprange
            SET start_address = $2::inet,
                end_address   = $3::inet,
                size          = (($3::inet - $2::inet) + 1),
                status        = $4,
                description   = $5,
                vrf_id        = $6,
                tenant_id     = $7,
                role_id       = $8,
                mark_utilized = $9,
                mark_populated = $10
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&start_address)
        .bind(&end_address)
        .bind(&status)
        .bind(description.as_deref())
        .bind(vrf_id)
        .bind(tenant_id)
        .bind(role_id)
        .bind(mark_utilized)
        .bind(mark_populated)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update ip_range: {}", e)))?;

        // Re-fetch to return the database-computed size.
        self.get_by_id(id).await
    }

    async fn bulk_create(&self, items: &[NewIpRange]) -> Result<BulkIpRangeResult, AppError> {
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
            let mark_utilized = item.mark_utilized.unwrap_or(false);
            let mark_populated = item.mark_populated.unwrap_or(false);

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO ipam_iprange (
                    id, custom_field_data,
                    start_address, end_address, size,
                    status, description, comments,
                    vrf_id, tenant_id, role_id,
                    mark_utilized, mark_populated, owner_id, tags
                )
                VALUES (
                    $1, '{}'::jsonb,
                    $2::inet, $3::inet,
                    (($3::inet - $2::inet) + 1),
                    $4, $5, '',
                    $6, $7, $8,
                    $9, $10, $11, '{}'
                )
                "#,
                )
                .bind(item.id)
                .bind(&item.start_address)
                .bind(&item.end_address)
                .bind(&status)
                .bind(item.description.as_deref())
                .bind(item.vrf_id)
                .bind(item.tenant_id)
                .bind(item.role_id)
                .bind(mark_utilized)
                .bind(mark_populated)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(IpRangeRecord {
                    id: item.id,
                    external_id: None,
                    start_address: item.start_address.clone(),
                    end_address: item.end_address.clone(),
                    // Size is computed by Postgres; use a placeholder here since
                    // we cannot query inside the transaction without an extra round-trip.
                    size: 0,
                    status,
                    description: item.description.clone(),
                    comments: None,
                    vrf_id: item.vrf_id,
                    tenant_id: item.tenant_id,
                    role_id: item.role_id,
                    mark_utilized,
                    mark_populated,
                    owner_id: item.owner_id,
                    tags: Vec::new(),
                    custom_fields: None,
                    piping_synced_at: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert ip_range '{}-{}': {}",
                    item.start_address, item.end_address, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk ip_range insert: {}", e))
        })?;

        Ok(BulkIpRangeResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_iprange", id)
            .await
            .map(|_| ())
    }
}
