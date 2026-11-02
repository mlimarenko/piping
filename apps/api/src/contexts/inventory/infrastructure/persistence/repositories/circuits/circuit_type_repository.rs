//! Circuit type repository — full CRUD over `circuits_circuittype`.
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
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const CIRCUIT_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::sortable("color", "color", "inventory.resources.fields.color"),
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

/// Representation of a `circuits_circuittype` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct CircuitTypeRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Input for creating a new `circuits_circuittype` row.
#[derive(Debug, Clone)]
pub struct NewCircuitType {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Fields that may be updated on an existing `circuits_circuittype` row.
#[derive(Debug, Clone)]
pub struct UpdateCircuitType {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkCircuitTypeResult {
    pub created: Vec<CircuitTypeRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait CircuitTypeRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitTypeRecord>, AppError>;
    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        owner_ids: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CircuitTypeRecord>, i64), AppError>;
    async fn create(&self, new_ct: &NewCircuitType) -> Result<CircuitTypeRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCircuitType,
    ) -> Result<Option<CircuitTypeRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewCircuitType],
    ) -> Result<BulkCircuitTypeResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxCircuitTypeRepository {
    pool: PgPool,
}

impl SqlxCircuitTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<CircuitTypeRecord, AppError> {
        Ok(CircuitTypeRecord {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read circuit_type id: {}", e))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read circuit_type name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read circuit_type slug: {}", e))
            })?,
            color: col(row, "color")?,
            description: col(row, "description")?,
            owner_id: row.try_get::<Option<Uuid>, _>("owner_id").map_err(|e| {
                AppError::Database(format!("Failed to read circuit_type owner_id: {}", e))
            })?,
            custom_fields: col(row, "custom_field_data")?,
        })
    }
}

impl CircuitTypeRepository for SqlxCircuitTypeRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitTypeRecord>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, color, description, owner_id, custom_field_data
            FROM circuits_circuittype
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load circuit_type: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        owner_ids: &[Uuid],
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CircuitTypeRecord>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, color, description, owner_id, custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM circuits_circuittype
            "#,
        );
        push_circuit_type_filters(&mut builder, owner_ids, search.as_deref());
        push_order_by(&mut builder, CIRCUIT_TYPE_COLUMNS, sort.as_ref(), "name");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list circuit_types: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read circuit_type total_count: {}", e))
            })?,
            None => count_circuit_types(&self.pool, owner_ids, search.as_deref()).await?,
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<CircuitTypeRecord>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_ct: &NewCircuitType) -> Result<CircuitTypeRecord, AppError> {
        let color = new_ct.color.clone().unwrap_or_default();
        let description = new_ct.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO circuits_circuittype (
                id, custom_field_data, name, slug, color, description, comments,
                owner_id
            )
            VALUES ($1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, '', $7)
            "#,
        )
        .bind(new_ct.id)
        .bind(new_ct.custom_fields.clone())
        .bind(&new_ct.name)
        .bind(&new_ct.slug)
        .bind(&color)
        .bind(&description)
        .bind(new_ct.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create circuit_type: {}", e)))?;

        Ok(CircuitTypeRecord {
            id: new_ct.id,
            name: new_ct.name.clone(),
            slug: new_ct.slug.clone(),
            color: Some(color),
            description: Some(description),
            owner_id: new_ct.owner_id,
            custom_fields: new_ct.custom_fields.clone(),
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCircuitType,
    ) -> Result<Option<CircuitTypeRecord>, AppError> {
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
            UPDATE circuits_circuittype
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
        .map_err(|e| AppError::Database(format!("Failed to update circuit_type: {}", e)))?;

        Ok(Some(CircuitTypeRecord {
            id,
            name,
            slug,
            color,
            description,
            owner_id: existing.owner_id,
            custom_fields,
        }))
    }

    async fn bulk_create(
        &self,
        items: &[NewCircuitType],
    ) -> Result<BulkCircuitTypeResult, AppError> {
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
                INSERT INTO circuits_circuittype (
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
                Ok(_) => created.push(CircuitTypeRecord {
                    id: item.id,
                    name: item.name.clone(),
                    slug: item.slug.clone(),
                    color: Some(color),
                    description: Some(description),
                    owner_id: item.owner_id,
                    custom_fields: item.custom_fields.clone(),
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert circuit_type '{}': {}",
                    item.name, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk circuit_type insert: {}", e))
        })?;

        Ok(BulkCircuitTypeResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "circuits_circuittype", id)
            .await
            .map(|_| ())
    }
}

fn push_circuit_type_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    owner_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &["name", "slug", "color", "description"],
        search,
    );
    push_uuid_in(builder, &mut where_state, "owner_id", owner_ids);
}

async fn count_circuit_types(
    pool: &PgPool,
    owner_ids: &[Uuid],
    search: Option<&str>,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM circuits_circuittype");
    push_circuit_type_filters(&mut builder, owner_ids, search);
    builder
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count circuit_types: {}", e)))
}
