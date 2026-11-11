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

    pub async fn custom_field_snapshot(
        &self,
        id: Uuid,
    ) -> Result<Option<(serde_json::Value, serde_json::Value)>, AppError> {
        sqlx::query_as("SELECT custom_field_data,jsonb_build_object('owner_id',owner_id,'manufacturer_id',manufacturer_id,'profile_id',profile_id) FROM dcim_moduletype WHERE id=$1")
            .bind(id).fetch_optional(&self.pool).await.map_err(crate::errors::classify_sqlx_error_owned)
    }

    pub async fn update(
        &self,
        id: Uuid,
        changes: &crate::contexts::inventory::domain::dcim::module_type::UpdateModuleType,
        inputs: Vec<crate::contexts::secrets::application::ApplySecretFieldIntentInput>,
    ) -> Result<Option<ModuleType>, AppError> {
        crate::contexts::inventory::domain::dcim::cooling::validate_cooling_method(
            changes.cooling_method.as_ref().and_then(|v| v.as_deref()),
        )?;
        if changes
            .model
            .as_ref()
            .is_some_and(|v| v.trim().is_empty() || v.chars().count() > 100)
        {
            return Err(AppError::Validation(
                "model must contain 1 to 100 characters".into(),
            ));
        }
        if changes
            .description
            .as_ref()
            .is_some_and(|v| v.chars().count() > 200)
        {
            return Err(AppError::Validation(
                "description must be at most 200 characters".into(),
            ));
        }
        if changes
            .custom_fields
            .as_ref()
            .is_some_and(|v| !v.is_object())
        {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        if changes
            .weight
            .flatten()
            .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err(AppError::Validation(
                "weight must be a finite nonnegative number".into(),
            ));
        }
        for (field, value) in [
            ("part_number", changes.part_number.as_deref()),
            (
                "weight_unit",
                changes.weight_unit.as_ref().and_then(|v| v.as_deref()),
            ),
            (
                "airflow",
                changes.airflow.as_ref().and_then(|v| v.as_deref()),
            ),
        ] {
            if value.is_some_and(|v| v.chars().count() > 50) {
                return Err(AppError::Validation(format!(
                    "{field} must be at most 50 characters"
                )));
            }
        }
        let mut tx = self.pool.begin().await?;
        // Share deletion's Tag -> graph -> field catalog -> resource order.
        // Field intents can lock this type too, so graph ownership must precede
        // both the intent batch and the explicit row lock below.
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch(&mut tx, &inputs).await?;
        let current = sqlx::query(
            "SELECT profile_id,attribute_data FROM dcim_moduletype WHERE id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(current) = current else {
            return Ok(None);
        };
        if changes.profile_id.is_some() || changes.attribute_data.is_some() {
            // Resolve omitted profile/attributes from the locked stored row,
            // never from an earlier route snapshot. Graph ownership excludes
            // profile DELETE before either this type or its profile is locked.
            let profile_id = changes.profile_id.unwrap_or(current.try_get("profile_id")?);
            let attributes = changes
                .attribute_data
                .clone()
                .unwrap_or(current.try_get("attribute_data")?);
            let attributes = resolve_profile_attributes(&mut tx, profile_id, attributes).await?;
            sqlx::query("UPDATE dcim_moduletype SET profile_id=$2,attribute_data=$3 WHERE id=$1")
                .bind(id)
                .bind(profile_id)
                .bind(attributes)
                .execute(&mut *tx)
                .await
                .map_err(crate::errors::classify_sqlx_error_owned)?;
        }
        sqlx::query("UPDATE dcim_moduletype SET manufacturer_id=COALESCE($2,manufacturer_id),model=COALESCE($3,model),part_number=COALESCE($4,part_number),weight=CASE WHEN $5 THEN $6 ELSE weight END,weight_unit=CASE WHEN $7 THEN $8 ELSE weight_unit END,airflow=CASE WHEN $9 THEN $10 ELSE airflow END,description=COALESCE($11,description),comments=COALESCE($12,comments),custom_field_data=custom_field_data || COALESCE($13,'{}'::jsonb) WHERE id=$1")
            .bind(id).bind(changes.manufacturer_id).bind(&changes.model).bind(&changes.part_number)
            .bind(changes.weight.is_some()).bind(changes.weight.flatten())
            .bind(changes.weight_unit.is_some()).bind(changes.weight_unit.as_ref().and_then(|v|v.as_deref()))
            .bind(changes.airflow.is_some()).bind(changes.airflow.as_ref().and_then(|v|v.as_deref()))
            .bind(&changes.description).bind(&changes.comments).bind(&changes.custom_fields)
            .execute(&mut *tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
        super::module_bay_membership::replace(
            &mut tx,
            super::module_bay_membership::ModuleBayCategoryOwner::ModuleType,
            id,
            changes.module_bay_types.as_ref(),
        )
        .await?;
        sqlx::query("UPDATE dcim_moduletype SET cooling_method=CASE WHEN $2 THEN $3 ELSE cooling_method END,end_of_life=CASE WHEN $4 THEN $5 ELSE end_of_life END WHERE id=$1")
            .bind(id).bind(changes.cooling_method.is_some()).bind(changes.cooling_method.as_ref().and_then(|v|v.as_deref()))
            .bind(changes.end_of_life.is_some()).bind(changes.end_of_life.flatten()).execute(&mut *tx).await.map_err(crate::errors::classify_sqlx_error_owned)?;
        let row = sqlx::query("SELECT id,profile_id,attribute_data,manufacturer_id,model,part_number,weight::float8 AS weight,weight_unit,airflow,cooling_method,end_of_life,(SELECT COUNT(*) FROM dcim_coolingintaketemplate WHERE module_type_id=dcim_moduletype.id) AS cooling_intake_template_count,(SELECT COUNT(*) FROM dcim_coolingoutflowtemplate WHERE module_type_id=dcim_moduletype.id) AS cooling_outflow_template_count,description,comments,custom_field_data,ARRAY(SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=dcim_moduletype.id ORDER BY modulebaytype_id) AS module_bay_types FROM dcim_moduletype WHERE id=$1")
            .bind(id).fetch_one(&mut *tx).await?;
        let item = Self::parse_module_type_row(&row)?;
        crate::contexts::secrets::application::SecretFieldIntentService::new(std::sync::Arc::new(
            crate::contexts::secrets::infrastructure::SqlxSecretRepository::new(self.pool.clone()),
        ))
        .apply_intents_in_transaction(&mut tx, inputs)
        .await?;
        tx.commit().await?;
        Ok(Some(item))
    }

    fn parse_module_type_row(row: &PgRow) -> Result<ModuleType, AppError> {
        Ok(ModuleType {
            profile_id: row.try_get("profile_id")?,
            attribute_data: row.try_get("attribute_data")?,
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
                .map_err(crate::errors::classify_sqlx_error_owned)?
                .filter(|s| !s.is_empty()),
            // NUMERIC in the schema, f64 here: without the projection cast this
            // read always failed and `.ok()` hid it as an absent weight.
            weight: row.try_get::<Option<f64>, _>("weight").map_err(|e| {
                AppError::Database(format!("Failed to read module type weight: {e}"))
            })?,
            weight_unit: row
                .try_get::<Option<String>, _>("weight_unit")
                .map_err(crate::errors::classify_sqlx_error_owned)?,
            airflow: row.try_get::<Option<String>, _>("airflow")?,
            cooling_method: row.try_get("cooling_method")?,
            end_of_life: row.try_get("end_of_life")?,
            cooling_intake_template_count: row.try_get("cooling_intake_template_count")?,
            cooling_outflow_template_count: row.try_get("cooling_outflow_template_count")?,
            description: row
                .try_get::<Option<String>, _>("description")
                .map_err(crate::errors::classify_sqlx_error_owned)?
                .filter(|s| !s.is_empty()),
            comments: row
                .try_get::<Option<String>, _>("comments")
                .map_err(crate::errors::classify_sqlx_error_owned)?
                .filter(|s| !s.is_empty()),
            custom_fields: row
                .try_get::<Option<serde_json::Value>, _>("custom_field_data")
                .map_err(crate::errors::classify_sqlx_error_owned)?,
            module_bay_types: row.try_get("module_bay_types")?,
        })
    }
}

impl ModuleTypeRepository for SqlxModuleTypeRepository {
    async fn get_by_id(&self, module_type_id: Uuid) -> Result<Option<ModuleType>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, profile_id, attribute_data, manufacturer_id, model, part_number, weight::float8 AS weight, weight_unit,
                   airflow, cooling_method, end_of_life,
                   (SELECT COUNT(*) FROM dcim_coolingintaketemplate WHERE module_type_id=dcim_moduletype.id) AS cooling_intake_template_count,
                   (SELECT COUNT(*) FROM dcim_coolingoutflowtemplate WHERE module_type_id=dcim_moduletype.id) AS cooling_outflow_template_count,
                   description, comments, custom_field_data,
                   ARRAY(SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=dcim_moduletype.id ORDER BY modulebaytype_id) AS module_bay_types
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
            SELECT id, profile_id, attribute_data, manufacturer_id, model, part_number, weight::float8 AS weight, weight_unit,
                   airflow, cooling_method, end_of_life,
                   (SELECT COUNT(*) FROM dcim_coolingintaketemplate WHERE module_type_id=dcim_moduletype.id) AS cooling_intake_template_count,
                   (SELECT COUNT(*) FROM dcim_coolingoutflowtemplate WHERE module_type_id=dcim_moduletype.id) AS cooling_outflow_template_count,
                   description, comments, custom_field_data,
                   ARRAY(SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=dcim_moduletype.id ORDER BY modulebaytype_id) AS module_bay_types
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
        crate::contexts::inventory::domain::dcim::cooling::validate_cooling_method(
            module_type.cooling_method.as_deref(),
        )?;
        let mut tx = self.pool.begin().await?;
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::lock_field_catalog_for_usage(&mut tx).await?;
        let attributes = resolve_profile_attributes(
            &mut tx,
            module_type.profile_id,
            module_type.attribute_data.clone(),
        )
        .await?;
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
        .execute(&mut *tx)
        .await
        .map_err(crate::errors::classify_sqlx_error_owned)?;
        sqlx::query("UPDATE dcim_moduletype SET cooling_method=$2,end_of_life=$3,profile_id=$4,attribute_data=$5 WHERE id=$1")
            .bind(module_type.id)
            .bind(&module_type.cooling_method)
            .bind(module_type.end_of_life)
            .bind(module_type.profile_id)
            .bind(attributes)
            .execute(&mut *tx)
            .await
            .map_err(crate::errors::classify_sqlx_error_owned)?;
        super::module_bay_membership::replace(
            &mut tx,
            super::module_bay_membership::ModuleBayCategoryOwner::ModuleType,
            module_type.id,
            Some(&Some(module_type.module_bay_types.clone())),
        )
        .await?;
        tx.commit().await?;
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

async fn resolve_profile_attributes(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    profile_id: Option<Uuid>,
    attributes: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, AppError> {
    let Some(id) = profile_id else {
        return Ok(None);
    };
    // Share-lock the profile until the entity commit so a concurrent schema
    // change or deletion cannot race this validation.
    let (schema,): (Option<serde_json::Value>,) =
        sqlx::query_as("SELECT schema FROM dcim_moduletypeprofile WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| AppError::Validation("profile_id does not exist".into()))?;
    let schema = schema.filter(|value| value.as_object().is_none_or(|object| !object.is_empty()));
    if schema.is_none() {
        return Ok(None);
    }
    crate::contexts::inventory::domain::dcim::module_type_profile::validate_profile_attributes(
        schema.as_ref(),
        attributes.as_ref(),
    )?;
    Ok(attributes)
}

fn push_module_type_filters(
    builder: &mut QueryBuilder<sqlx::Postgres>,
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
