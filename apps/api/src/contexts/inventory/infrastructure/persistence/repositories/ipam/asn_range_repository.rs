//! ASN range repository — full CRUD over `ipam_asnrange`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! The columns `start` and `"end"` are SQL reserved words; they are always
//! double-quoted in every query to avoid parse errors.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const ASN_RANGE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.asnRanges.columns.name"),
    ListColumn::sortable("slug", "slug", "inventory.asnRanges.columns.slug"),
    ListColumn::fk_filter(
        "rir_id",
        "rir_id",
        "rir_id",
        "rir_id",
        "rirs",
        "inventory.asnRanges.columns.rir",
    ),
    ListColumn::sortable("start", "\"start\"", "inventory.asnRanges.columns.start"),
    ListColumn::sortable("end", "\"end\"", "inventory.asnRanges.columns.end"),
    ListColumn::fk_filter(
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenant_id",
        "tenants",
        "inventory.asnRanges.columns.tenant",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.asnRanges.columns.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.asnRanges.columns.comments",
    ),
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.asnRanges.columns.owner",
    ),
];

/// Representation of an `ipam_asnrange` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct AsnRangeRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub rir_id: Uuid,
    pub start: i64,
    pub end: i64,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `ipam_asnrange` row.
#[derive(Debug, Clone)]
pub struct NewAsnRange {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub rir_id: Uuid,
    pub start: i64,
    pub end: i64,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_asnrange` row.
#[derive(Debug, Clone)]
pub struct UpdateAsnRange {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub rir_id: Option<Uuid>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub tenant_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkAsnRangeResult {
    pub created: Vec<AsnRangeRecord>,
    pub errors: Vec<String>,
}

/// Selection, paging and ordering for the ASN range collection.
pub struct AsnRangeListParams<'a> {
    pub rir_ids: &'a [Uuid],
    pub tenant_ids: &'a [Uuid],
    pub owner_ids: &'a [Uuid],
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait AsnRangeRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<AsnRangeRecord>, AppError>;
    async fn list(
        &self,
        params: AsnRangeListParams<'_>,
    ) -> Result<(Vec<AsnRangeRecord>, i64), AppError>;
    async fn create(&self, new_asn_range: &NewAsnRange) -> Result<AsnRangeRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateAsnRange,
    ) -> Result<Option<AsnRangeRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewAsnRange]) -> Result<BulkAsnRangeResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxAsnRangeRepository {
    pool: PgPool,
}

impl SqlxAsnRangeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<AsnRangeRecord, AppError> {
        Ok(AsnRangeRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read asn_range id: {e}")))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read asn_range name: {e}")))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read asn_range slug: {e}")))?,
            rir_id: row
                .try_get("rir_id")
                .map_err(|e| AppError::Database(format!("Failed to read asn_range rir_id: {e}")))?,
            start: row
                .try_get("start")
                .map_err(|e| AppError::Database(format!("Failed to read asn_range start: {e}")))?,
            end: row
                .try_get("end")
                .map_err(|e| AppError::Database(format!("Failed to read asn_range end: {e}")))?,
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read asn_range tenant_id: {e}"))
            })?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            comments: row.try_get::<Option<String>, _>("comments").ok().flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read asn_range owner_id: {e}"))
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
}

impl AsnRangeRepository for SqlxAsnRangeRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<AsnRangeRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, rir_id, "start", "end", tenant_id, description, comments,
                   owner_id, NULL::text AS owner_name, custom_field_data
            FROM ipam_asnrange
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load asn_range: {e}")))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        params: AsnRangeListParams<'_>,
    ) -> Result<(Vec<AsnRangeRecord>, i64), AppError> {
        let limit = clamp_limit(params.limit) as i64;
        let offset = clamp_offset(params.offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, rir_id, "start", "end", tenant_id, description, comments,
                   owner_id, owner_name, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM (
                SELECT ar.id, ar.name, ar.slug, ar.rir_id, ar."start", ar."end",
                       ar.tenant_id, ar.description, ar.comments, ar.owner_id,
                       COALESCE(NULLIF(u.username, ''), u.email) AS owner_name,
                       ar.custom_field_data
                FROM ipam_asnrange ar
                LEFT JOIN identity_user u ON u.id = ar.owner_id
            ) asn_range_rows
            "#,
        );
        push_asn_range_filters(
            &mut builder,
            params.rir_ids,
            params.tenant_ids,
            params.owner_ids,
            params.search.as_deref(),
        );
        push_order_by(
            &mut builder,
            ASN_RANGE_COLUMNS,
            params.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list asn_ranges: {e}")))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read asn_range total_count: {e}"))
            })?,
            None => {
                count_asn_ranges(
                    &self.pool,
                    params.rir_ids,
                    params.tenant_ids,
                    params.owner_ids,
                    params.search.as_deref(),
                )
                .await?
            }
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<AsnRangeRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_asn_range: &NewAsnRange) -> Result<AsnRangeRecord, AppError> {
        let description = new_asn_range.description.clone().unwrap_or_default();
        let comments = new_asn_range.comments.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_asnrange (
                id, custom_field_data, name, slug, rir_id, "start", "end",
                tenant_id, description, comments, owner_id
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(new_asn_range.id)
        .bind(&new_asn_range.name)
        .bind(&new_asn_range.slug)
        .bind(new_asn_range.rir_id)
        .bind(new_asn_range.start)
        .bind(new_asn_range.end)
        .bind(new_asn_range.tenant_id)
        .bind(&description)
        .bind(&comments)
        .bind(new_asn_range.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create asn_range: {e}")))?;

        Ok(AsnRangeRecord {
            id: new_asn_range.id,
            name: new_asn_range.name.clone(),
            slug: new_asn_range.slug.clone(),
            rir_id: new_asn_range.rir_id,
            start: new_asn_range.start,
            end: new_asn_range.end,
            tenant_id: new_asn_range.tenant_id,
            description: Some(description),
            comments: Some(comments),
            owner_id: new_asn_range.owner_id,
            owner_name: None,
            custom_fields: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateAsnRange,
    ) -> Result<Option<AsnRangeRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let rir_id = changes.rir_id.unwrap_or(existing.rir_id);
        let start = changes.start.unwrap_or(existing.start);
        let end = changes.end.unwrap_or(existing.end);
        let tenant_id = match &changes.tenant_id {
            Some(v) => *v,
            None => existing.tenant_id,
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };
        let comments = if changes.comments.is_some() {
            changes.comments.clone()
        } else {
            existing.comments
        };

        sqlx::query(
            r#"
            UPDATE ipam_asnrange
            SET name = $2, slug = $3, rir_id = $4, "start" = $5, "end" = $6,
                tenant_id = $7, description = $8, comments = $9
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(rir_id)
        .bind(start)
        .bind(end)
        .bind(tenant_id)
        .bind(description.clone().unwrap_or_default())
        .bind(comments.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update asn_range: {e}")))?;

        Ok(Some(AsnRangeRecord {
            id,
            name,
            slug,
            rir_id,
            start,
            end,
            tenant_id,
            description,
            comments,
            owner_id: existing.owner_id,
            owner_name: existing.owner_name,
            custom_fields: existing.custom_fields,
        }))
    }

    async fn bulk_create(&self, items: &[NewAsnRange]) -> Result<BulkAsnRangeResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let description = item.description.clone().unwrap_or_default();
            let comments = item.comments.clone().unwrap_or_default();

            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO ipam_asnrange (
                    id, custom_field_data, name, slug, rir_id, "start", "end",
                    tenant_id, description, comments, owner_id
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
                )
                .bind(item.id)
                .bind(&item.name)
                .bind(&item.slug)
                .bind(item.rir_id)
                .bind(item.start)
                .bind(item.end)
                .bind(item.tenant_id)
                .bind(&description)
                .bind(&comments)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(AsnRangeRecord {
                    id: item.id,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    rir_id: item.rir_id,
                    start: item.start,
                    end: item.end,
                    tenant_id: item.tenant_id,
                    description: Some(description),
                    comments: Some(comments),
                    owner_id: item.owner_id,
                    owner_name: None,
                    custom_fields: None,
                }),
                Err(e) => errors.push(format!("Failed to insert asn_range '{}': {e}", item.name)),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk asn_range insert: {e}"))
        })?;

        Ok(BulkAsnRangeResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_asnrange", id)
            .await
            .map(|_| ())
    }
}

fn push_asn_range_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    rir_ids: &[Uuid],
    tenant_ids: &[Uuid],
    owner_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_uuid_in(builder, &mut where_state, "rir_id", rir_ids);
    push_uuid_in(builder, &mut where_state, "tenant_id", tenant_ids);
    push_uuid_in(builder, &mut where_state, "owner_id", owner_ids);
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "name",
            "slug",
            "\"start\"",
            "\"end\"",
            "description",
            "comments",
        ],
        search,
    );
}

async fn count_asn_ranges(
    pool: &PgPool,
    rir_ids: &[Uuid],
    tenant_ids: &[Uuid],
    owner_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM ipam_asnrange");
    push_asn_range_filters(&mut builder, rir_ids, tenant_ids, owner_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count asn_ranges: {e}")))
}
