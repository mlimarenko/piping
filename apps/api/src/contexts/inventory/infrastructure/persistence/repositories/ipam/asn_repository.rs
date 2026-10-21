//! ASN repository — full CRUD over `ipam_asn`.
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

pub const ASN_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("asn", "a.asn", "inventory.resources.fields.asn"),
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

/// Representation of an `ipam_asn` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct AsnRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub asn: i64,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `ipam_asn` — includes rir/tenant names resolved via
/// LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct AsnListItem {
    pub record: AsnRecord,
    pub rir_name: Option<String>,
    pub tenant_name: Option<String>,
}

/// Input for creating a new `ipam_asn` row.
#[derive(Debug, Clone)]
pub struct NewAsn {
    pub id: Uuid,
    pub asn: i64,
    pub rir_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_asn` row.
#[derive(Debug, Clone)]
pub struct UpdateAsn {
    pub asn: Option<i64>,
    pub rir_id: Option<Uuid>,
    pub tenant_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Selection, paging and ordering for the ASN collection.
pub struct AsnListParams<'a> {
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
pub trait AsnRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<AsnRecord>, AppError>;
    async fn list(&self, params: AsnListParams<'_>) -> Result<(Vec<AsnListItem>, i64), AppError>;
    async fn create(&self, new_asn: &NewAsn) -> Result<AsnRecord, AppError>;
    async fn update(&self, id: Uuid, changes: &UpdateAsn) -> Result<Option<AsnRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewAsn],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<AsnRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxAsnRepository {
    pool: PgPool,
}

impl SqlxAsnRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<AsnRecord, AppError> {
        Ok(AsnRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read asn id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read asn external_id: {}", e))
            })?,
            asn: row
                .try_get("asn")
                .map_err(|e| AppError::Database(format!("Failed to read asn asn: {}", e)))?,
            rir_id: row
                .try_get("rir_id")
                .map_err(|e| AppError::Database(format!("Failed to read asn rir_id: {}", e)))?,
            tenant_id: row
                .try_get::<Option<Uuid>, _>("tenant_id")
                .map_err(|e| AppError::Database(format!("Failed to read asn tenant_id: {}", e)))?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            comments: row.try_get::<Option<String>, _>("comments").ok().flatten(),
            owner_id: row
                .try_get::<Option<Uuid>, _>("owner_id")
                .map_err(|e| AppError::Database(format!("Failed to read asn owner_id: {}", e)))?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read asn piping_synced_at: {}", e))
                })?,
        })
    }
}

impl AsnRepository for SqlxAsnRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<AsnRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, asn, rir_id, tenant_id, description, comments,
                   owner_id, tags, custom_field_data, piping_synced_at
            FROM ipam_asn
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load asn: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(&self, params: AsnListParams<'_>) -> Result<(Vec<AsnListItem>, i64), AppError> {
        let limit = clamp_limit(params.limit) as i64;
        let offset = clamp_offset(params.offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT a.id, a.external_id, a.asn, a.rir_id, a.tenant_id, a.description, a.comments,
                   a.owner_id, a.tags, a.custom_field_data, a.piping_synced_at,
                   r.name AS rir_name,
                   t.name AS tenant_name,
                   COUNT(*) OVER() AS total_count
            FROM ipam_asn a
            LEFT JOIN ipam_rir r ON r.id = a.rir_id
            LEFT JOIN tenancy_tenant t ON t.id = a.tenant_id
            "#,
        );
        push_asn_filters(
            &mut builder,
            params.rir_ids,
            params.tenant_ids,
            params.search.as_deref(),
        );
        push_order_by(&mut builder, ASN_COLUMNS, params.sort.as_ref(), "a.asn");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list asns: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read asn total_count: {}", e))
            })?,
            None => {
                count_asns(
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
                Ok(AsnListItem {
                    record: Self::parse_row(row)?,
                    rir_name: row.try_get("rir_name").ok(),
                    tenant_name: row.try_get("tenant_name").ok(),
                })
            })
            .collect::<Result<Vec<AsnListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_asn: &NewAsn) -> Result<AsnRecord, AppError> {
        let description = new_asn.description.clone().unwrap_or_default();
        let comments = new_asn.comments.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_asn (
                id, custom_field_data, asn, rir_id, tenant_id, description, comments,
                owner_id, tags, external_id, piping_synced_at
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, '{}', NULL, NULL)
            "#,
        )
        .bind(new_asn.id)
        .bind(new_asn.asn)
        .bind(new_asn.rir_id)
        .bind(new_asn.tenant_id)
        .bind(&description)
        .bind(&comments)
        .bind(new_asn.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create asn: {}", e)))?;

        Ok(AsnRecord {
            id: new_asn.id,
            external_id: None,
            asn: new_asn.asn,
            rir_id: new_asn.rir_id,
            tenant_id: new_asn.tenant_id,
            description: Some(description),
            comments: Some(comments),
            owner_id: new_asn.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(&self, id: Uuid, changes: &UpdateAsn) -> Result<Option<AsnRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let asn = changes.asn.unwrap_or(existing.asn);
        let rir_id = changes.rir_id.unwrap_or(existing.rir_id);
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
            UPDATE ipam_asn
            SET asn = $2, rir_id = $3, tenant_id = $4, description = $5, comments = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(asn)
        .bind(rir_id)
        .bind(tenant_id)
        .bind(description.clone().unwrap_or_default())
        .bind(comments.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update asn: {}", e)))?;

        Ok(Some(AsnRecord {
            id,
            external_id: existing.external_id,
            asn,
            rir_id,
            tenant_id,
            description,
            comments,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewAsn],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<AsnRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk asn insert",
            |connection, item| {
                Box::pin(async move {
                    let description = item.description.clone().unwrap_or_default();
                    let comments = item.comments.clone().unwrap_or_default();

                    sqlx::query(
                        r#"
                INSERT INTO ipam_asn (
                    id, custom_field_data, asn, rir_id, tenant_id, description, comments,
                    owner_id, tags, external_id, piping_synced_at
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, '{}', NULL, NULL)
                "#,
                    )
                    .bind(item.id)
                    .bind(item.asn)
                    .bind(item.rir_id)
                    .bind(item.tenant_id)
                    .bind(&description)
                    .bind(&comments)
                    .bind(item.owner_id)
                    .execute(connection)
                    .await
                    .map_err(|error| format!("Failed to insert asn '{}': {error}", item.asn))?;

                    Ok(AsnRecord {
                        id: item.id,
                        external_id: None,
                        asn: item.asn,
                        rir_id: item.rir_id,
                        tenant_id: item.tenant_id,
                        description: Some(description),
                        comments: Some(comments),
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
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_asn", id)
            .await
            .map(|_| ())
    }
}

fn push_asn_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    rir_ids: &[Uuid],
    tenant_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["a.asn", "a.description", "a.comments", "r.name", "t.name"],
        search,
    );
    push_uuid_in(builder, &mut where_state, "a.rir_id", rir_ids);
    push_uuid_in(builder, &mut where_state, "a.tenant_id", tenant_ids);
}

async fn count_asns(
    pool: &PgPool,
    rir_ids: &[Uuid],
    tenant_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new(
        "SELECT COUNT(*) FROM ipam_asn a \
         LEFT JOIN ipam_rir r ON r.id = a.rir_id \
         LEFT JOIN tenancy_tenant t ON t.id = a.tenant_id",
    );
    push_asn_filters(&mut builder, rir_ids, tenant_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count asns: {}", e)))
}
