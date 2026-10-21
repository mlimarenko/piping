use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::custom_fields::domain::{
    Choice, CustomFieldChoiceSet, NewCustomFieldChoiceSet, UpdateCustomFieldChoiceSet,
};
use crate::contexts::shared::list_query::{ListColumn, SortSpec, push_order_by};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const CUSTOM_FIELD_CHOICE_SET_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "customFields.choiceSet.name"),
    ListColumn::sortable(
        "choices",
        "jsonb_array_length(extra_choices)",
        "customFields.choiceSet.choices",
    ),
    ListColumn::sortable(
        "order_alphabetically",
        "order_alphabetically",
        "customFields.choiceSet.orderAlphabetically",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "customFields.choiceSet.createdAt",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "customFields.choiceSet.updatedAt",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct CustomFieldChoiceSetListFilter {
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

/// CRUD persistence for `custom_field_choice_set`. All SQL for choice sets
/// lives here.
#[async_trait]
pub trait CustomFieldChoiceSetRepository: Send + Sync {
    async fn list_page(
        &self,
        filter: &CustomFieldChoiceSetListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CustomFieldChoiceSet>, AppError>;
    async fn count(&self, filter: &CustomFieldChoiceSetListFilter) -> Result<u64, AppError>;
    async fn get(&self, id: Uuid) -> Result<Option<CustomFieldChoiceSet>, AppError>;
    async fn create(
        &self,
        input: &NewCustomFieldChoiceSet,
    ) -> Result<CustomFieldChoiceSet, AppError>;
    async fn update(
        &self,
        id: Uuid,
        input: &UpdateCustomFieldChoiceSet,
    ) -> Result<Option<CustomFieldChoiceSet>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<bool, AppError>;
}

#[derive(Clone)]
pub struct SqlxCustomFieldChoiceSetRepository {
    pool: PgPool,
}

impl SqlxCustomFieldChoiceSetRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, sqlx::Postgres>,
        filter: &CustomFieldChoiceSetListFilter,
    ) {
        if let Some(search) = filter.search.as_deref() {
            let trimmed = search.trim();
            if !trimmed.is_empty() {
                let pattern = format!("%{trimmed}%");
                builder.push(" WHERE (name ILIKE ");
                builder.push_bind(pattern.clone());
                builder.push(" OR description ILIKE ");
                builder.push_bind(pattern);
                builder.push(")");
            }
        }
    }
}

const SELECT_COLUMNS: &str = "id, name, description, extra_choices, order_alphabetically, \
    choice_colors, created_at, updated_at";

fn map_row(row: &sqlx::postgres::PgRow) -> Result<CustomFieldChoiceSet, AppError> {
    let extra_choices_raw: Value = row
        .try_get("extra_choices")
        .map_err(|e| AppError::Database(format!("custom_field_choice_set.extra_choices: {e}")))?;
    let extra_choices: Vec<Choice> = serde_json::from_value(extra_choices_raw).map_err(|e| {
        AppError::Database(format!(
            "custom_field_choice_set.extra_choices is not an array of {{value,label}}: {e}"
        ))
    })?;

    let choice_colors_raw: Value = row
        .try_get("choice_colors")
        .map_err(|e| AppError::Database(format!("custom_field_choice_set.choice_colors: {e}")))?;
    let choice_colors: BTreeMap<String, String> = serde_json::from_value(choice_colors_raw)
        .map_err(|e| {
            AppError::Database(format!(
                "custom_field_choice_set.choice_colors is not a string map: {e}"
            ))
        })?;

    Ok(CustomFieldChoiceSet {
        id: row
            .try_get("id")
            .map_err(|e| AppError::Database(format!("custom_field_choice_set.id: {e}")))?,
        name: row
            .try_get("name")
            .map_err(|e| AppError::Database(format!("custom_field_choice_set.name: {e}")))?,
        description: row
            .try_get("description")
            .map_err(|e| AppError::Database(format!("custom_field_choice_set.description: {e}")))?,
        extra_choices,
        order_alphabetically: row.try_get("order_alphabetically").map_err(|e| {
            AppError::Database(format!("custom_field_choice_set.order_alphabetically: {e}"))
        })?,
        choice_colors,
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(|e| AppError::Database(format!("custom_field_choice_set.created_at: {e}")))?,
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .map_err(|e| AppError::Database(format!("custom_field_choice_set.updated_at: {e}")))?,
    })
}

fn choices_to_json(choices: &[Choice]) -> Result<Value, AppError> {
    serde_json::to_value(choices)
        .map_err(|e| AppError::Database(format!("failed to encode extra_choices: {e}")))
}

fn colors_to_json(colors: &BTreeMap<String, String>) -> Result<Value, AppError> {
    serde_json::to_value(colors)
        .map_err(|e| AppError::Database(format!("failed to encode choice_colors: {e}")))
}

#[async_trait]
impl CustomFieldChoiceSetRepository for SqlxCustomFieldChoiceSetRepository {
    async fn list_page(
        &self,
        filter: &CustomFieldChoiceSetListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CustomFieldChoiceSet>, AppError> {
        let mut builder = QueryBuilder::new(format!(
            "SELECT {SELECT_COLUMNS} FROM custom_field_choice_set"
        ));
        Self::push_list_filters(&mut builder, filter);
        push_order_by(
            &mut builder,
            CUSTOM_FIELD_CHOICE_SET_COLUMNS,
            filter.sort.as_ref(),
            "name",
        );
        builder.push(" LIMIT ");
        builder.push_bind(i64::from(limit));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(offset));
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        rows.iter().map(map_row).collect()
    }

    async fn count(&self, filter: &CustomFieldChoiceSetListFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) FROM custom_field_choice_set");
        Self::push_list_filters(&mut builder, filter);
        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.try_get::<i64, _>(0)
            .map(|value| value as u64)
            .map_err(|error| AppError::Database(format!("custom_field_choice_set.count: {error}")))
    }

    async fn get(&self, id: Uuid) -> Result<Option<CustomFieldChoiceSet>, AppError> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM custom_field_choice_set WHERE id = $1");
        let row = sqlx::query(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(map_row).transpose()
    }

    async fn create(
        &self,
        input: &NewCustomFieldChoiceSet,
    ) -> Result<CustomFieldChoiceSet, AppError> {
        let query = format!(
            "INSERT INTO custom_field_choice_set \
             (name, description, extra_choices, order_alphabetically, choice_colors) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(&input.name)
            .bind(&input.description)
            .bind(choices_to_json(&input.extra_choices)?)
            .bind(input.order_alphabetically)
            .bind(colors_to_json(&input.choice_colors)?)
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        map_row(&row)
    }

    async fn update(
        &self,
        id: Uuid,
        input: &UpdateCustomFieldChoiceSet,
    ) -> Result<Option<CustomFieldChoiceSet>, AppError> {
        let extra_choices = input
            .extra_choices
            .as_deref()
            .map(choices_to_json)
            .transpose()?;
        let choice_colors = input
            .choice_colors
            .as_ref()
            .map(colors_to_json)
            .transpose()?;
        let query = format!(
            "UPDATE custom_field_choice_set SET \
                 name = COALESCE($2, name), \
                 description = COALESCE($3, description), \
                 extra_choices = COALESCE($4, extra_choices), \
                 order_alphabetically = COALESCE($5, order_alphabetically), \
                 choice_colors = COALESCE($6, choice_colors), \
                 updated_at = now() \
             WHERE id = $1 \
             RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(id)
            .bind(input.name.as_deref())
            .bind(input.description.as_deref())
            .bind(extra_choices)
            .bind(input.order_alphabetically)
            .bind(choice_colors)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(map_row).transpose()
    }

    async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        // `custom_field.choice_set_id` is ON DELETE RESTRICT: while any field
        // still references the set, the delete must surface as 409
        // DELETE_BLOCKED naming those fields — not as the generic FK 400 that
        // claims the record "does not exist".
        let deleted = crate::contexts::shared::delete_guard::delete_record_guarded(
            &self.pool,
            "custom_field_choice_set",
            id,
            crate::contexts::shared::delete_guard::no_blocker_metadata,
        )
        .await?;
        Ok(deleted > 0)
    }
}
