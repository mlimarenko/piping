//! Rack type repository — full CRUD over `dcim_racktype`.
//!
//! Runtime sqlx only (`sqlx::query` + `.bind`) so the crate type-checks and the
//! contract verifies via `cargo check` without a database.

use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

/// Representation of a `dcim_racktype` row exposed by the REST API.
#[derive(Debug, Clone)]
pub struct RackTypeRecord {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub slug: String,
    pub form_factor: String,
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: bool,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Hydrated list row for `dcim_racktype` — includes the manufacturer name
/// resolved via LEFT JOIN for the collection list endpoint.
#[derive(Debug, Clone)]
pub struct RackTypeListItem {
    pub record: RackTypeRecord,
    pub manufacturer_name: Option<String>,
}

/// Input for creating a new `dcim_racktype` row.
#[derive(Debug, Clone)]
pub struct NewRackType {
    pub id: Uuid,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub slug: String,
    pub form_factor: String,
    pub width: i16,
    pub u_height: i16,
    pub starting_unit: i16,
    pub desc_units: bool,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Uuid>,
}

/// Fields that may be updated on an existing `dcim_racktype` row.
#[derive(Debug, Clone)]
pub struct UpdateRackType {
    pub manufacturer_id: Option<Uuid>,
    pub model: Option<String>,
    pub slug: Option<String>,
    pub form_factor: Option<String>,
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: Option<bool>,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
}

/// Result of a bulk-create operation.
#[derive(Debug, Clone)]
pub struct BulkRackTypeResult {
    pub created: Vec<RackTypeRecord>,
    pub errors: Vec<String>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RackTypeRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackTypeRecord>, AppError>;
    async fn list(
        &self,
        filter: RackTypeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RackTypeListItem>, i64), AppError>;
    async fn create(&self, new_type: &NewRackType) -> Result<RackTypeRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRackType,
    ) -> Result<Option<RackTypeRecord>, AppError>;
    async fn bulk_create(&self, items: &[NewRackType]) -> Result<BulkRackTypeResult, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Debug, Default)]
pub struct RackTypeFilter {
    pub search: Option<String>,
    pub manufacturer_id: Vec<Uuid>,
    pub form_factor: Vec<String>,
    pub desc_units: Vec<String>,
    pub outer_unit: Vec<String>,
    pub weight_unit: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Clone)]
pub struct SqlxRackTypeRepository {
    pool: PgPool,
}

impl SqlxRackTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Parse one `RACK_TYPE_COLUMNS` row. The projection is shared by every
    /// caller (the qualified constant selects the same set), and the
    /// `weight::float8` cast is load-bearing — NUMERIC does not decode into
    /// f64, so under the old `.ok()` reads weight was always None.
    fn parse_row(row: &PgRow) -> Result<RackTypeRecord, AppError> {
        Ok(RackTypeRecord {
            id: col(row, "id")?,
            external_id: col(row, "external_id")?,
            manufacturer_id: col(row, "manufacturer_id")?,
            model: col(row, "model")?,
            slug: col(row, "slug")?,
            form_factor: col(row, "form_factor")?,
            width: col(row, "width")?,
            u_height: col(row, "u_height")?,
            starting_unit: col(row, "starting_unit")?,
            desc_units: col(row, "desc_units")?,
            outer_width: col(row, "outer_width")?,
            outer_depth: col(row, "outer_depth")?,
            outer_height: col(row, "outer_height")?,
            outer_unit: col(row, "outer_unit")?,
            mounting_depth: col(row, "mounting_depth")?,
            weight: col(row, "weight")?,
            max_weight: col(row, "max_weight")?,
            weight_unit: col(row, "weight_unit")?,
            description: col(row, "description")?,
            owner_id: col(row, "owner_id")?,
            tags: col(row, "tags")?,
            custom_fields: col(row, "custom_field_data")?,
            piping_synced_at: col(row, "piping_synced_at")?,
        })
    }

    fn push_list_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &RackTypeFilter) {
        let mut where_state = WhereState::new();
        push_search_ilike(
            builder,
            &mut where_state,
            &[
                "rt.external_id",
                "rt.model",
                "rt.slug",
                "rt.form_factor",
                "rt.description",
                "rt.tags",
            ],
            filter.search.as_deref(),
        );
        push_uuid_in(
            builder,
            &mut where_state,
            "rt.manufacturer_id",
            &filter.manufacturer_id,
        );
        push_string_in(
            builder,
            &mut where_state,
            "rt.form_factor",
            &filter.form_factor,
        );
        push_string_in(
            builder,
            &mut where_state,
            "rt.desc_units::text",
            &filter.desc_units,
        );
        push_string_in(
            builder,
            &mut where_state,
            "rt.outer_unit",
            &filter.outer_unit,
        );
        push_string_in(
            builder,
            &mut where_state,
            "rt.weight_unit",
            &filter.weight_unit,
        );
    }
}

const RACK_TYPE_COLUMNS: &str = r#"
    id, external_id, manufacturer_id, model, slug, form_factor, width, u_height,
    starting_unit, desc_units, outer_width, outer_depth, outer_height, outer_unit,
    mounting_depth, weight::float8 AS weight, max_weight, weight_unit, description,
    owner_id, tags, custom_field_data, piping_synced_at
"#;

/// Same columns as `RACK_TYPE_COLUMNS` but qualified with the `rt` table alias,
/// for use in the hydrated list query that LEFT JOINs `dcim_manufacturer`.
const RACK_TYPE_COLUMNS_QUALIFIED: &str = r#"
    rt.id, rt.external_id, rt.manufacturer_id, rt.model, rt.slug, rt.form_factor,
    rt.width, rt.u_height, rt.starting_unit, rt.desc_units, rt.outer_width,
    rt.outer_depth, rt.outer_height, rt.outer_unit, rt.mounting_depth,
    rt.weight::float8 AS weight, rt.max_weight, rt.weight_unit, rt.description,
    rt.owner_id, rt.tags, rt.custom_field_data, rt.piping_synced_at
"#;

pub const RACK_TYPE_LIST_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "rt.external_id",
        "inventory.rackTypes.columns.externalId",
    ),
    ListColumn::fk_filter(
        "manufacturer",
        "m.name",
        "manufacturer_id",
        "rt.manufacturer_id",
        "manufacturers",
        "inventory.rackTypes.columns.manufacturer",
    ),
    ListColumn::sortable("model", "rt.model", "inventory.rackTypes.columns.model"),
    ListColumn::sortable("slug", "rt.slug", "inventory.rackTypes.columns.slug"),
    ListColumn::enum_filter(
        "form_factor",
        "rt.form_factor",
        "rt.form_factor",
        "inventory.rackTypes.columns.formFactor",
    ),
    ListColumn::sortable("width", "rt.width", "inventory.rackTypes.columns.width"),
    ListColumn::sortable(
        "u_height",
        "rt.u_height",
        "inventory.rackTypes.columns.uHeight",
    ),
    ListColumn::sortable(
        "starting_unit",
        "rt.starting_unit",
        "inventory.rackTypes.columns.startingUnit",
    ),
    ListColumn::enum_filter(
        "desc_units",
        "rt.desc_units",
        "rt.desc_units::text",
        "inventory.rackTypes.columns.descUnits",
    ),
    ListColumn::sortable(
        "outer_width",
        "rt.outer_width",
        "inventory.rackTypes.columns.outerWidth",
    ),
    ListColumn::sortable(
        "outer_depth",
        "rt.outer_depth",
        "inventory.rackTypes.columns.outerDepth",
    ),
    ListColumn::sortable(
        "outer_height",
        "rt.outer_height",
        "inventory.rackTypes.columns.outerHeight",
    ),
    ListColumn::enum_filter(
        "outer_unit",
        "rt.outer_unit",
        "rt.outer_unit",
        "inventory.rackTypes.columns.outerUnit",
    ),
    ListColumn::sortable(
        "mounting_depth",
        "rt.mounting_depth",
        "inventory.rackTypes.columns.mountingDepth",
    ),
    ListColumn::sortable("weight", "rt.weight", "inventory.rackTypes.columns.weight"),
    ListColumn::sortable(
        "max_weight",
        "rt.max_weight",
        "inventory.rackTypes.columns.maxWeight",
    ),
    ListColumn::enum_filter(
        "weight_unit",
        "rt.weight_unit",
        "rt.weight_unit",
        "inventory.rackTypes.columns.weightUnit",
    ),
    ListColumn::sortable(
        "description",
        "rt.description",
        "inventory.rackTypes.columns.description",
    ),
    ListColumn::sortable("tags", "rt.tags", "inventory.rackTypes.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "rt.piping_synced_at",
        "inventory.rackTypes.columns.syncedAt",
    ),
];

impl RackTypeRepository for SqlxRackTypeRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<RackTypeRecord>, AppError> {
        let sql = format!("SELECT {RACK_TYPE_COLUMNS} FROM dcim_racktype WHERE id = $1",);
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load rack_type: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        filter: RackTypeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<RackTypeListItem>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::new(format!(
            r#"
            SELECT {RACK_TYPE_COLUMNS_QUALIFIED},
                   m.name AS manufacturer_name,
                   COUNT(*) OVER() AS total_count
            FROM dcim_racktype rt
            LEFT JOIN dcim_manufacturer m ON m.id = rt.manufacturer_id
            "#,
        ));
        Self::push_list_filters(&mut builder, &filter);
        push_order_by(
            &mut builder,
            RACK_TYPE_LIST_COLUMNS,
            filter.sort.as_ref(),
            "rt.model",
        );
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list rack_types: {}", e)))?;

        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|e| {
                AppError::Database(format!("Failed to read rack_type total_count: {}", e))
            })?,
            None => {
                let mut count_builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_racktype rt");
                Self::push_list_filters(&mut count_builder, &filter);
                count_builder
                    .build_query_scalar::<i64>()
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| AppError::Database(format!("Failed to count rack_types: {}", e)))?
            }
        };
        let items = rows
            .iter()
            .map(|row| {
                Ok(RackTypeListItem {
                    record: Self::parse_row(row)?,
                    manufacturer_name: col(row, "manufacturer_name")?,
                })
            })
            .collect::<Result<Vec<RackTypeListItem>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, new_type: &NewRackType) -> Result<RackTypeRecord, AppError> {
        let description = new_type.description.clone().unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO dcim_racktype (
                id, custom_field_data, manufacturer_id, model, slug, form_factor,
                width, u_height, starting_unit, desc_units, outer_width, outer_depth,
                outer_height, outer_unit, mounting_depth, weight, max_weight,
                weight_unit, description, comments, rack_count, owner_id, tags
            )
            VALUES (
                $1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                $14, $15, $16, $17, $18, '', 0, $19, '{}'
            )
            "#,
        )
        .bind(new_type.id)
        .bind(new_type.manufacturer_id)
        .bind(&new_type.model)
        .bind(&new_type.slug)
        .bind(&new_type.form_factor)
        .bind(new_type.width)
        .bind(new_type.u_height)
        .bind(new_type.starting_unit)
        .bind(new_type.desc_units)
        .bind(new_type.outer_width)
        .bind(new_type.outer_depth)
        .bind(new_type.outer_height)
        .bind(new_type.outer_unit.as_deref())
        .bind(new_type.mounting_depth)
        .bind(new_type.weight)
        .bind(new_type.max_weight)
        .bind(new_type.weight_unit.as_deref())
        .bind(&description)
        .bind(new_type.owner_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create rack_type: {}", e)))?;

        Ok(RackTypeRecord {
            id: new_type.id,
            external_id: None,
            manufacturer_id: new_type.manufacturer_id,
            model: new_type.model.clone(),
            slug: new_type.slug.clone(),
            form_factor: new_type.form_factor.clone(),
            width: Some(new_type.width),
            u_height: Some(new_type.u_height),
            starting_unit: Some(new_type.starting_unit),
            desc_units: new_type.desc_units,
            outer_width: new_type.outer_width,
            outer_depth: new_type.outer_depth,
            outer_height: new_type.outer_height,
            outer_unit: new_type.outer_unit.clone(),
            mounting_depth: new_type.mounting_depth,
            weight: new_type.weight,
            max_weight: new_type.max_weight,
            weight_unit: new_type.weight_unit.clone(),
            description: Some(description),
            owner_id: new_type.owner_id,
            tags: Vec::new(),
            custom_fields: None,
            piping_synced_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateRackType,
    ) -> Result<Option<RackTypeRecord>, AppError> {
        let Some(existing) = self.get_by_id(id).await? else {
            return Ok(None);
        };

        let manufacturer_id = changes.manufacturer_id.unwrap_or(existing.manufacturer_id);
        let model = changes.model.clone().unwrap_or(existing.model);
        let slug = changes.slug.clone().unwrap_or(existing.slug);
        let form_factor = changes.form_factor.clone().unwrap_or(existing.form_factor);
        let width = changes.width.or(existing.width);
        let u_height = changes.u_height.or(existing.u_height);
        let starting_unit = changes.starting_unit.or(existing.starting_unit);
        let desc_units = changes.desc_units.unwrap_or(existing.desc_units);
        let outer_width = if changes.outer_width.is_some() {
            changes.outer_width
        } else {
            existing.outer_width
        };
        let outer_depth = if changes.outer_depth.is_some() {
            changes.outer_depth
        } else {
            existing.outer_depth
        };
        let outer_height = if changes.outer_height.is_some() {
            changes.outer_height
        } else {
            existing.outer_height
        };
        let outer_unit = if changes.outer_unit.is_some() {
            changes.outer_unit.clone()
        } else {
            existing.outer_unit
        };
        let mounting_depth = if changes.mounting_depth.is_some() {
            changes.mounting_depth
        } else {
            existing.mounting_depth
        };
        let weight = if changes.weight.is_some() {
            changes.weight
        } else {
            existing.weight
        };
        let max_weight = if changes.max_weight.is_some() {
            changes.max_weight
        } else {
            existing.max_weight
        };
        let weight_unit = if changes.weight_unit.is_some() {
            changes.weight_unit.clone()
        } else {
            existing.weight_unit
        };
        let description = if changes.description.is_some() {
            changes.description.clone()
        } else {
            existing.description
        };

        sqlx::query(
            r#"
            UPDATE dcim_racktype
            SET manufacturer_id = $2, model = $3, slug = $4, form_factor = $5,
                width = $6, u_height = $7, starting_unit = $8, desc_units = $9,
                outer_width = $10, outer_depth = $11, outer_height = $12,
                outer_unit = $13, mounting_depth = $14, weight = $15,
                max_weight = $16, weight_unit = $17, description = $18
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(manufacturer_id)
        .bind(&model)
        .bind(&slug)
        .bind(&form_factor)
        .bind(width)
        .bind(u_height)
        .bind(starting_unit)
        .bind(desc_units)
        .bind(outer_width)
        .bind(outer_depth)
        .bind(outer_height)
        .bind(outer_unit.as_deref())
        .bind(mounting_depth)
        .bind(weight)
        .bind(max_weight)
        .bind(weight_unit.as_deref())
        .bind(description.clone().unwrap_or_default())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update rack_type: {}", e)))?;

        Ok(Some(RackTypeRecord {
            id,
            external_id: existing.external_id,
            manufacturer_id,
            model,
            slug,
            form_factor,
            width,
            u_height,
            starting_unit,
            desc_units,
            outer_width,
            outer_depth,
            outer_height,
            outer_unit,
            mounting_depth,
            weight,
            max_weight,
            weight_unit,
            description,
            owner_id: existing.owner_id,
            tags: existing.tags,
            custom_fields: existing.custom_fields,
            piping_synced_at: existing.piping_synced_at,
        }))
    }

    async fn bulk_create(&self, items: &[NewRackType]) -> Result<BulkRackTypeResult, AppError> {
        let mut created = Vec::new();
        let mut errors = Vec::new();

        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;

        for item in items {
            let description = item.description.clone().unwrap_or_default();
            let result = super::super::bulk_insert::insert_row(
                &mut tx,
                sqlx::query(
                    r#"
                INSERT INTO dcim_racktype (
                    id, custom_field_data, manufacturer_id, model, slug, form_factor,
                    width, u_height, starting_unit, desc_units, outer_width, outer_depth,
                    outer_height, outer_unit, mounting_depth, weight, max_weight,
                    weight_unit, description, comments, rack_count, owner_id, tags
                )
                VALUES (
                    $1, '{}'::jsonb, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                    $14, $15, $16, $17, $18, '', 0, $19, '{}'
                )
                "#,
                )
                .bind(item.id)
                .bind(item.manufacturer_id)
                .bind(&item.model)
                .bind(&item.slug)
                .bind(&item.form_factor)
                .bind(item.width)
                .bind(item.u_height)
                .bind(item.starting_unit)
                .bind(item.desc_units)
                .bind(item.outer_width)
                .bind(item.outer_depth)
                .bind(item.outer_height)
                .bind(item.outer_unit.as_deref())
                .bind(item.mounting_depth)
                .bind(item.weight)
                .bind(item.max_weight)
                .bind(item.weight_unit.as_deref())
                .bind(&description)
                .bind(item.owner_id),
            )
            .await?;

            match result {
                Ok(_) => created.push(RackTypeRecord {
                    id: item.id,
                    external_id: None,
                    manufacturer_id: item.manufacturer_id,
                    model: item.model.clone(),
                    slug: item.slug.clone(),
                    form_factor: item.form_factor.clone(),
                    width: Some(item.width),
                    u_height: Some(item.u_height),
                    starting_unit: Some(item.starting_unit),
                    desc_units: item.desc_units,
                    outer_width: item.outer_width,
                    outer_depth: item.outer_depth,
                    outer_height: item.outer_height,
                    outer_unit: item.outer_unit.clone(),
                    mounting_depth: item.mounting_depth,
                    weight: item.weight,
                    max_weight: item.max_weight,
                    weight_unit: item.weight_unit.clone(),
                    description: Some(description),
                    owner_id: item.owner_id,
                    tags: Vec::new(),
                    custom_fields: None,
                    piping_synced_at: None,
                }),
                Err(e) => errors.push(format!(
                    "Failed to insert rack_type '{}': {}",
                    item.model, e
                )),
            }
        }

        tx.commit().await.map_err(|e| {
            AppError::Database(format!("Failed to commit bulk rack_type insert: {}", e))
        })?;

        Ok(BulkRackTypeResult { created, errors })
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_racktype", id)
            .await
            .map(|_| ())
    }
}
