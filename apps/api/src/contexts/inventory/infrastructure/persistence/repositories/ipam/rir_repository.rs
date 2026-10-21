//! RIR repository — full CRUD over `ipam_rir`.
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

pub const RIR_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::enum_filter(
        "is_private",
        "is_private",
        "is_private",
        "inventory.resources.fields.isPrivate",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
    ListColumn::fk_filter(
        "owner",
        "owner_id",
        "owner_id",
        "owner_id",
        "users",
        "inventory.resources.fields.owner",
    ),
];

/// Representation of an `ipam_rir` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct RirRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub is_private: bool,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for creating a new `ipam_rir` row.
#[derive(Debug, Clone)]
pub struct NewRir {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub is_private: bool,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `ipam_rir` row.
#[derive(Debug, Clone)]
pub struct UpdateRir {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub is_private: Option<bool>,
    pub description: Option<String>,
}

/// Selection, paging and ordering for the RIR collection.
pub struct RirListParams<'a> {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub is_private: &'a [String],
    pub owner_ids: &'a [Uuid],
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RirRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RirRecord>, AppError>;
    async fn list(&self, params: RirListParams<'_>) -> Result<(Vec<RirRecord>, i64), AppError>;
    async fn create(&self, new_rir: &NewRir) -> Result<RirRecord, AppError>;
    async fn update(&self, id: Uuid, changes: &UpdateRir) -> Result<Option<RirRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewRir],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<RirRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxRirRepository {
    pool: PgPool,
}

impl SqlxRirRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<RirRecord, AppError> {
        Ok(RirRecord {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read rir id: {}", e)))?,
            external_id: row.try_get::<Option<i64>, _>("external_id").map_err(|e| {
                AppError::Database(format!("Failed to read rir external_id: {}", e))
            })?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read rir name: {}", e)))?,
            slug: row
                .try_get("slug")
                .map_err(|e| AppError::Database(format!("Failed to read rir slug: {}", e)))?,
            is_private: row
                .try_get("is_private")
                .map_err(|e| AppError::Database(format!("Failed to read rir is_private: {}", e)))?,
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten(),
            owner_id: row
                .try_get::<Option<Uuid>, _>("owner_id")
                .map_err(|e| AppError::Database(format!("Failed to read rir owner_id: {}", e)))?,
            tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
            piping_synced_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("piping_synced_at")
                .map_err(|e| {
                    AppError::Database(format!("Failed to read rir piping_synced_at: {}", e))
                })?,
        })
    }
}

impl RirRepository for SqlxRirRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RirRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, external_id, name, slug, is_private, description, owner_id,
                   tags, custom_field_data, piping_synced_at
            FROM ipam_rir
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load rir: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(&self, params: RirListParams<'_>) -> Result<(Vec<RirRecord>, i64), AppError> {
        let limit = clamp_limit(params.limit) as i64;
        let offset = clamp_offset(params.offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, external_id, name, slug, is_private, description, owner_id,
                   tags, custom_field_data, piping_synced_at,
                   COUNT(*) OVER() AS total_count
            FROM ipam_rir
            "#,
        );
        push_rir_filters(
            &mut builder,
            params.is_private,
            params.owner_ids,
            params.search.as_deref(),
        );
        push_order_by(&mut builder, RIR_COLUMNS, params.sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list rirs: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read rir total_count: {}", e))
            })?,
            None => {
                count_rirs(
                    &self.pool,
                    params.is_private,
                    params.owner_ids,
                    params.search.as_deref(),
                )
                .await?
            }
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<RirRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_rir: &NewRir) -> Result<RirRecord, AppError> {
        let description = new_rir.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO ipam_rir (
                id, custom_field_data, name, slug, is_private, description, comments,
                owner_id, tags
            )
            VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, '', $6, '{}')
            "#,
        )
        .bind(new_rir.id)
        .bind(&new_rir.name)
        .bind(&new_rir.slug)
        .bind(new_rir.is_private)
        .bind(&description)
        .bind(new_rir.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create rir: {}", e)))?;

        Ok(RirRecord {
            id: new_rir.id,
            external_id: None,
            name: new_rir.name.clone(),
            slug: new_rir.slug.clone(),
            is_private: new_rir.is_private,
            description: Some(description),
            owner_id: new_rir.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(&self, id: Uuid, changes: &UpdateRir) -> Result<Option<RirRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let name = changes.name.clone().unwrap_or(existing.name);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let is_private = changes.is_private.unwrap_or(existing.is_private);
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE ipam_rir
            SET name = $2, slug = $3, is_private = $4, description = $5
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&name)
        .bind(&slug)
        .bind(is_private)
        .bind(description.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update rir: {}", e)))?;

        Ok(Some(RirRecord {
            id,
            external_id: existing.external_id,
            name,
            slug,
            is_private,
            description,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewRir],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<RirRecord>, AppError> {
        super::super::bulk_insert::execute(
            &self.pool,
            items,
            "bulk rir insert",
            |connection, item| {
                Box::pin(async move {
                    let description = item.description.clone().unwrap_or_default();

                    sqlx::query(
                        r#"
                INSERT INTO ipam_rir (
                    id, custom_field_data, name, slug, is_private, description, comments,
                    owner_id, tags
                )
                VALUES ($1, '{}'::jsonb, $2, $3, $4, $5, '', $6, '{}')
                "#,
                    )
                    .bind(item.id)
                    .bind(&item.name)
                    .bind(&item.slug)
                    .bind(item.is_private)
                    .bind(&description)
                    .bind(item.owner_id)
                    .execute(connection)
                    .await
                    .map_err(|error| format!("Failed to insert rir '{}': {error}", item.name))?;

                    Ok(RirRecord {
                        id: item.id,
                        external_id: None,
                        name: item.name.clone(),
                        slug: item.slug.clone(),
                        is_private: item.is_private,
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
        super::super::delete_guard::delete_inventory_record(&self.pool, "ipam_rir", id)
            .await
            .map(|_| ())
    }
}

fn push_rir_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    is_private: &[String],
    owner_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_string_in(builder, &mut where_state, "is_private::text", is_private);
    push_uuid_in(builder, &mut where_state, "owner_id", owner_ids);
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "external_id",
            "name",
            "slug",
            "is_private",
            "description",
            "tags",
        ],
        search,
    );
}

async fn count_rirs(
    pool: &PgPool,
    is_private: &[String],
    owner_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM ipam_rir");
    push_rir_filters(&mut builder, is_private, owner_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count rirs: {}", e)))
}
