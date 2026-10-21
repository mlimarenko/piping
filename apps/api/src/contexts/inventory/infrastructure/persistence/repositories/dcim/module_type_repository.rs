use sqlx::postgres::PgRow;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::inventory::domain::ModuleType;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

pub const MODULE_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("model", "model", "inventory.resources.fields.model"),
    ListColumn::fk_filter(
        "manufacturer",
        "manufacturer_id",
        "manufacturer_id",
        "manufacturer_id",
        "Manufacturers",
        "inventory.resources.fields.manufacturer",
    ),
    ListColumn::sortable(
        "part_number",
        "part_number",
        "inventory.resources.fields.partNumber",
    ),
    ListColumn::sortable("weight", "weight", "inventory.resources.fields.weight"),
    ListColumn::sortable(
        "weight_unit",
        "weight_unit",
        "inventory.resources.fields.weightUnit",
    ),
    ListColumn::sortable("airflow", "airflow", "inventory.resources.fields.airflow"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.resources.fields.comments",
    ),
];

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ModuleTypeRepository: Send + Sync {
    async fn get_by_id(&self, module_type_id: Uuid) -> Result<Option<ModuleType>, AppError>;
    async fn list_paginated(
        &self,
        manufacturer_ids: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ModuleType>, i64), AppError>;
    async fn create(&self, module_type: &ModuleType) -> Result<(), AppError>;
    async fn update(&self, module_type: &ModuleType) -> Result<(), AppError>;
    async fn delete(&self, module_type_id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxModuleTypeRepository {
    pool: PgPool,
}

impl SqlxModuleTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_module_type_row(row: &PgRow) -> Result<ModuleType, AppError> {
        Ok(ModuleType {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read module type id: {}", e)))?,
            manufacturer_id: row.try_get("manufacturer_id").map_err(|e| {
                AppError::Database(format!("Failed to read module type manufacturer_id: {}", e))
            })?,
            model: row.try_get("model").map_err(|e| {
                AppError::Database(format!("Failed to read module type model: {}", e))
            })?,
            part_number: row
                .try_get::<Option<String>, _>("part_number")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            // NUMERIC in the schema, f64 here: without the projection cast this
            // read always failed and `.ok()` hid it as an absent weight.
            weight: row.try_get::<Option<f64>, _>("weight").map_err(|e| {
                AppError::Database(format!("Failed to read module type weight: {e}"))
            })?,
            weight_unit: row
                .try_get::<Option<String>, _>("weight_unit")
                .ok()
                .flatten(),
            airflow: row.try_get::<Option<String>, _>("airflow").ok().flatten(),
            description: row
                .try_get::<Option<String>, _>("description")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            comments: row
                .try_get::<Option<String>, _>("comments")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .ok()
                .flatten(),
        })
    }
}

impl ModuleTypeRepository for SqlxModuleTypeRepository {
    async fn get_by_id(&self, module_type_id: Uuid) -> Result<Option<ModuleType>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, manufacturer_id, model, part_number, weight::float8 AS weight, weight_unit,
                   airflow, description, comments, custom_field_data
            FROM dcim_moduletype
            WHERE id = $1
            "#,
        )
        .bind(module_type_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load module type: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_module_type_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        manufacturer_ids: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ModuleType>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut count_builder = QueryBuilder::new("SELECT COUNT(*) FROM dcim_moduletype");
        push_module_type_filters(&mut count_builder, manufacturer_ids, search.as_deref());
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count module types: {}", e)))?;

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, manufacturer_id, model, part_number, weight::float8 AS weight, weight_unit,
                   airflow, description, comments, custom_field_data
            FROM dcim_moduletype
            "#,
        );
        push_module_type_filters(&mut builder, manufacturer_ids, search.as_deref());
        push_order_by(&mut builder, MODULE_TYPE_COLUMNS, sort.as_ref(), "model");
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list module types: {}", e)))?;

        let items = rows
            .iter()
            .map(Self::parse_module_type_row)
            .collect::<Result<Vec<ModuleType>, AppError>>()?;
        Ok((items, total))
    }

    async fn create(&self, module_type: &ModuleType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO dcim_moduletype (
                id,
                custom_field_data,
                manufacturer_id,
                model,
                part_number,
                weight,
                weight_unit,
                airflow,
                description,
                comments,
                module_count
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5,
                $6, $7, $8, $9, $10,
                0
            )
            "#,
        )
        .bind(module_type.id)
        .bind(module_type.custom_fields.clone())
        .bind(module_type.manufacturer_id)
        .bind(&module_type.model)
        .bind(module_type.part_number.as_deref().unwrap_or(""))
        .bind(module_type.weight)
        .bind(module_type.weight_unit.as_deref())
        .bind(module_type.airflow.as_deref())
        .bind(module_type.description.as_deref().unwrap_or(""))
        .bind(module_type.comments.as_deref().unwrap_or(""))
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create module type: {}", e)))?;
        Ok(())
    }

    async fn update(&self, module_type: &ModuleType) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE dcim_moduletype
            SET manufacturer_id = $2,
                model = $3,
                part_number = $4,
                weight = $5,
                weight_unit = $6,
                airflow = $7,
                description = $8,
                comments = $9,
                custom_field_data = COALESCE($10, custom_field_data)
            WHERE id = $1
            "#,
        )
        .bind(module_type.id)
        .bind(module_type.manufacturer_id)
        .bind(&module_type.model)
        .bind(module_type.part_number.as_deref().unwrap_or(""))
        .bind(module_type.weight)
        .bind(module_type.weight_unit.as_deref())
        .bind(module_type.airflow.as_deref())
        .bind(module_type.description.as_deref().unwrap_or(""))
        .bind(module_type.comments.as_deref().unwrap_or(""))
        .bind(module_type.custom_fields.clone())
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update module type: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, module_type_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_moduletype",
            module_type_id,
        )
        .await
        .map(|_| ())
    }
}

fn push_module_type_filters(
    builder: &mut QueryBuilder<'_, sqlx::Postgres>,
    manufacturer_ids: &[Uuid],
    search: Option<&str>,
) {
    let mut where_state = WhereState::new();
    push_search_ilike(
        builder,
        &mut where_state,
        &[
            "model",
            "part_number",
            "weight",
            "weight_unit",
            "airflow",
            "description",
            "comments",
        ],
        search,
    );
    push_uuid_in(
        builder,
        &mut where_state,
        "manufacturer_id",
        manufacturer_ids,
    );
}
