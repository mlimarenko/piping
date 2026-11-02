//! Aggregate repository — full CRUD over `ipam_aggregate`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.
//!
//! The `prefix` column is a Postgres `cidr`; it is read via a `::text` cast and
//! written via a `$N::cidr` cast, matching the approach used by
//! `prefix_repository` / `ip_range_repository` for network columns.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const AGGREGATE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("prefix", "a.prefix", "inventory.resources.fields.prefix"),
    ListColumn::fk_filter(
        "rir",
        "r.name",
        "rir_id",
        "a.rir_id",
        "rirs",
        "inventory.resources.fields.rir",
    ),
    ListColumn::sortable("rirName", "r.name", "inventory.resources.fields.rir"),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "a.tenant_id",
        "tenants",
        "inventory.resources.fields.tenant",
    ),
    ListColumn::sortable("tenantName", "t.name", "inventory.resources.fields.tenant"),
    ListColumn::sortable(
        "date_added",
        "a.date_added",
        "inventory.resources.fields.dateAdded",
    ),
    ListColumn::sortable(
        "description",
        "a.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "a.comments",
        "inventory.resources.fields.comments",
    ),
];

/// Representation of an `ipam_aggregate` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct AggregateRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub prefix: String,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub date_added: Option<chrono::NaiveDate>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `ipam_aggregate` — includes rir/tenant names resolved
/// via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct AggregateListItem {
    pub record: AggregateRecord,
    pub rir_name: Option<String>,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `ipam_aggregate` row.
#[derive(Debug, Clone)]
pub struct NewAggregate {
    pub id: Uuid,
    pub prefix: String,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub date_added: Option<chrono::NaiveDate>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_aggregate` row.
#[derive(Debug, Clone)]
pub struct UpdateAggregate {
    pub prefix: Option<String>,
    pub rir_id: Option<Uuid>,
    pub tenant_id: Option<Option<Uuid>>,
    pub date_added: Option<Option<chrono::NaiveDate>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkAggregateResult {
    pub created: Vec<AggregateRecord>,
    pub errors: Vec<String>,
}

/// Selection, paging and ordering for the aggregate collection.
pub struct AggregateListParams<'a> {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub rir_ids: &'a [Uuid],
    pub tenant_ids: &'a [Uuid],
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait AggregateRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<AggregateRecord>, AppError>;
    async fn list(
        &self,
        params: AggregateListParams<'_>,
    ) -> Result<(Vec<AggregateListItem>, i64), AppError>;
    async fn create(&self, new_aggregate: &NewAggregate) -> Result<AggregateRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateAggregate,
    ) -> Result<Option<AggregateRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewAggregate]) -> Result<BulkAggregateResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxAggregateRepository {
    pool: PgPool,
}

impl SqlxAggregateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<AggregateRecord, AppError> {
        Ok(AggregateRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read aggregate id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read aggregate external_id: {}", e))
            })?,
            prefix: row.try_get("prefix").map_err(|e| {
                AppError::Database(format!("Failed to read aggregate prefix: {}", e))
            })?,
            rir_id: row.try_get("rir_id").map_err(|e| {
                AppError::Database(format!("Failed to read aggregate rir_id: {}", e))
            })?,
            tenant_id: row.try_get::<Option<Uuid>, _>("tenant_id").map_err(|e| {
                AppError::Database(format!("Failed to read aggregate tenant_id: {}", e))
            })?,
            date_added: row
                .try_get::<Option<chrono::NaiveDate>, _>("date_added")
                .ok()
                .flatten(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            comments: row.try_get::<Option<String>, _>("comments").ok().flatten(),
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read aggregate owner_id: {}", e))
            })?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read aggregate piping_synced_at: {}", e))
                })?,
        })
    }
}

impl AggregateRepository for SqlxAggregateRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<AggregateRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id,
                   prefix::text AS prefix,
                   rir_id, tenant_id, date_added, description, comments,
                   owner_id, tags, custom_field_data, piping_synced_at
            FROM ipam_aggregate
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load aggregate: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        params: AggregateListParams<'_>,
    ) -> Result<(Vec<AggregateListItem>, i64), AppError> {
        let limit = clamp_limit(params.limit) as i64;
        let offset = clamp_offset(params.offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT a.id, a.external_id,
                   a.prefix::text AS prefix,
                   a.rir_id, a.tenant_id, a.date_added, a.description, a.comments,
                   a.owner_id, a.tags, a.custom_field_data, a.piping_synced_at,
                   r.name AS rir_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_aggregate a
            LEFT JOIN ipam_rir r ON r.id = a.rir_id
            LEFT JOIN tenancy_tenant t ON t.id = a.tenant_id
            "#,
        );
        push_aggregate_filters(
            &mut builder,
            params.rir_ids,
            params.tenant_ids,
            params.search.as_deref(),
        );
        push_order_by(
            &mut builder,
            AGGREGATE_COLUMNS,
            params.sort.as_ref(),
            "a.prefix",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list aggregates: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read aggregate total_count: {}", e))
            })?,
            None => {
                count_aggregates(
                    &self.pool,
                    params.rir_ids,
                    params.tenant_ids,
                    params.search.as_deref(),
                )
                .await?
            }
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(AggregateListItem {
                    record: Self::parse_row(row)?,
                    rir_name: row.try_get("rir_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<AggregateListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_aggregate: &NewAggregate) -> Result<AggregateRecord, AppError> {
        let description = new_aggregate.description.clone().unwrap_or_default();
        let comments = new_aggregate.comments.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_aggregate (
                id, custom_field_data, prefix, date_added, description,
                rir_id, tenant_id, comments, owner_id, tags
            )
            VALUES ($1, '{}'::jsonb, $2::cidr, $3, $4, $5, $6, $7, $8, '{}')
            "#,
        )
        .bind(new_aggregate.id)
        .bind(&new_aggregate.prefix)
        .bind(new_aggregate.date_added)
        .bind(&description)
        .bind(new_aggregate.rir_id)
        .bind(new_aggregate.tenant_id)
        .bind(&comments)
        .bind(new_aggregate.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create aggregate: {}", e)))?;

        Ok(AggregateRecord {
            id: new_aggregate.id,
            external_id: None,
            prefix: new_aggregate.prefix.clone(),
            rir_id: new_aggregate.rir_id,
            tenant_id: new_aggregate.tenant_id,
            date_added: new_aggregate.date_added,
            description: Some(description),
            comments: Some(comments),
            owner_id: new_aggregate.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateAggregate,
    ) -> Result<Option<AggregateRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let prefix = changes.prefix.clone().unwrap_or(existing.prefix);
        let rir_id = changes.rir_id.unwrap_or(existing.rir_id);
        let tenant_id = match &changes.tenant_id {
            Some(v) => *v,
            None => existing.tenant_id,
        };
        let date_added = match &changes.date_added {
            Some(v) => *v,
            None => existing.date_added,
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
            UPDATE ipam_aggregate
            SET prefix = $2::cidr, rir_id = $3, tenant_id = $4,
                date_added = $5, description = $6, comments = $7
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&prefix)
        .bind(rir_id)
        .bind(tenant_id)
        .bind(date_added)
        .bind(description.clone().unwrap_or_default())
        .bind(comments.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update aggregate: {}", e)))?;

        Ok(Some(AggregateRecord {
            id,
            external_id: existing.external_id,
            prefix,
            rir_id,
            tenant_id,
            date_added,
            description,
            comments,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(&self, items: &[NewAggregate]) -> Result<BulkAggregateResult, AppError> {
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
                INSERT INTO ipam_aggregate (
                    id, custom_field_data, prefix, date_added, description,
                    rir_id, tenant_id, comments, owner_id, tags
                )
                VALUES ($1, '{}'::jsonb, $2::cidr, $3, $4, $5, $6, $7, $8, '{}')
                "#,
                )
                .bind(item.id)
                .bind(&item.prefix)
                .bind(item.date_added)
                .bind(&description)
                .bind(item.rir_id)
                .bind(item.tenant_id)
                .bind(&comments)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(AggregateRecord {
                    id: item.id,
                    external_id: None,
                    prefix: item.prefix.clone(),
                    rir_id: item.rir_id,
                    tenant_id: item.tenant_id,
                    date_added: item.date_added,
                    description: Some(description),
                    comments: Some(comments),
                    owner_id: item.owner_id,
                    tags: Vec::new(),
                    custom_fields: None,
                    piping_synced_at: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert aggregate '{}': {}",
                    item.prefix, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk aggregate insert: {}", e))
        })?;

        Ok(BulkAggregateResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_aggregate", id)
            .await
            .map(|_| ())
    }
}

fn push_aggregate_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    rir_ids: &[Uuid],
    tenant_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "a.prefix",
            "a.description",
            "a.comments",
            "r.name",
            "t.name",
        ],
        search,
    );
    push_uuid_in(builder, &mut where_state, "a.rir_id", rir_ids);
    push_uuid_in(builder, &mut where_state, "a.tenant_id", tenant_ids);
}

async fn count_aggregates(
    pool: &PgPool,
    rir_ids: &[Uuid],
    tenant_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new(
        "SELECT COUNT(*) FROM ipam_aggregate a \
         LEFT JOIN ipam_rir r ON r.id = a.rir_id \
         LEFT JOIN tenancy_tenant t ON t.id = a.tenant_id",
    );
    push_aggregate_filters(&mut builder, rir_ids, tenant_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count aggregates: {}", e)))
}
