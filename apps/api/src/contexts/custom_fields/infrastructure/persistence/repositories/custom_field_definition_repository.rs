use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, QueryBuilder, Row};
use uuid::Uuid;

/// A seeded native scope whose installed table actually stores custom fields.
#[derive(Debug)]
pub(crate) struct NativeFieldSubject {
    pub(crate) content_type_id: Uuid,
    pub(crate) table_name: String,
}

pub(crate) async fn lock_field_catalog_for_usage(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock_shared(hashtextextended('inventory-secret-field-catalog', 0))"
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn lock_field_catalog_for_definition(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    // Definition mutations touch many subjects. Lock before any definition or
    // resource row to avoid parent-cascade/definition lock-order inversions.
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('inventory-secret-field-catalog', 0))"
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn native_field_subjects(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Vec<NativeFieldSubject>, AppError> {
    // Subject identifiers come from the intersection of the installed seeded
    // scope catalogue and actual public native UUID/entity storage, never from
    // a caller-supplied table/model. Object-reference label metadata is not the
    // full native custom-field scope catalogue.
    Ok(sqlx::query_as!(NativeFieldSubject, "SELECT c.id AS content_type_id, r.relname::text AS \"table_name!\" FROM inventory_content_type c JOIN pg_class r ON r.relname = c.app_label || '_' || c.model JOIN pg_namespace n ON n.oid = r.relnamespace WHERE n.nspname = 'public' AND r.relkind IN ('r', 'p') AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = r.oid AND a.attname = 'custom_field_data' AND a.atttypid = 'jsonb'::regtype AND NOT a.attisdropped) AND EXISTS (SELECT 1 FROM pg_attribute a JOIN pg_index i ON i.indrelid = r.oid AND i.indisprimary AND i.indnatts = 1 AND a.attnum = ANY(i.indkey) WHERE a.attrelid = r.oid AND a.attname = 'id' AND a.atttypid = 'uuid'::regtype AND NOT a.attisdropped) ORDER BY r.relname, c.id")
        .fetch_all(&mut **transaction).await?)
}

/// A seeded content type is a valid object-reference target, not necessarily a
/// custom-field scope. AdminModel explicitly excludes custom-field storage.
async fn validate_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<(), AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT app_label,model FROM inventory_content_type WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
    if let Some((app, model)) = row
        && crate::contexts::inventory::domain::content_type::by_app_model(&app, &model)
            .is_some_and(|def| def.is_admin_model())
    {
        return Err(AppError::Validation(
            "AdminModel does not support custom fields".into(),
        ));
    }
    Ok(())
}

use crate::contexts::custom_fields::domain::{
    CustomFieldDefinition, CustomFieldType, FilterLogic, NewCustomFieldDefinition, UiEditable,
    UiVisible, UpdateCustomFieldDefinition,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::ContentTypeRow;
use crate::contexts::shared::list_query::{
    ListColumn, ListColumnFilter, SortSpec, WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const CUSTOM_FIELD_DEFINITION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "cf.name", "customFields.def.name"),
    ListColumn::sortable("label", "cf.label", "customFields.def.label"),
    ListColumn::enum_filter(
        "field_type",
        "cf.field_type",
        "cf.field_type",
        "customFields.def.fieldType",
    ),
    ListColumn::enum_filter(
        "group_name",
        "cf.group_name",
        "cf.group_name",
        "customFields.def.group",
    ),
    ListColumn::fk_filter(
        "scope",
        "COUNT(DISTINCT cfot_filter.content_type_id)",
        "object_type_id",
        "cfot_filter.content_type_id",
        "content-types",
        "customFields.def.scope",
    ),
    ListColumn::sortable("weight", "cf.weight", "customFields.def.weight"),
    ListColumn::sortable(
        "flags",
        "(CASE WHEN cf.is_secret THEN 2 ELSE 0 END + CASE WHEN cf.required THEN 1 ELSE 0 END)",
        "customFields.def.flags",
    ),
    ListColumn::enum_filter(
        "ui_visible",
        "cf.ui_visible",
        "cf.ui_visible",
        "customFields.def.uiVisible",
    ),
    ListColumn::enum_filter(
        "ui_editable",
        "cf.ui_editable",
        "cf.ui_editable",
        "customFields.def.uiEditable",
    ),
    ListColumn::enum_filter(
        "filter_logic",
        "cf.filter_logic",
        "cf.filter_logic",
        "customFields.def.filterLogic",
    ),
    ListColumn::fk_filter(
        "choice_set",
        "cs.name",
        "choice_set_id",
        "cf.choice_set_id",
        "custom-field-choice-sets",
        "customFields.def.choiceSet",
    ),
    ListColumn::fk_filter(
        "related_object_type",
        "related_ct.app_label, related_ct.model",
        "related_object_type_id",
        "cf.related_object_type_id",
        "content-types",
        "customFields.def.relatedObjectType",
    ),
    ListColumn::sortable("created_at", "cf.created_at", "customFields.def.createdAt"),
    ListColumn::sortable("updated_at", "cf.updated_at", "customFields.def.updatedAt"),
];

#[derive(Debug, Clone, Default)]
pub struct CustomFieldDefinitionListFilter {
    pub search: Option<String>,
    pub field_type: Vec<String>,
    pub group_name: Vec<String>,
    pub ui_visible: Vec<String>,
    pub ui_editable: Vec<String>,
    pub filter_logic: Vec<String>,
    pub choice_set_id: Vec<Uuid>,
    pub related_object_type_id: Vec<Uuid>,
    pub object_type_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

/// CRUD persistence for `custom_field` plus its `custom_field_object_type`
/// scope M2M. All SQL for custom-field definitions lives here.
#[async_trait]
pub trait CustomFieldDefinitionRepository: Send + Sync {
    async fn list_page(
        &self,
        filter: &CustomFieldDefinitionListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CustomFieldDefinition>, AppError>;
    async fn count(&self, filter: &CustomFieldDefinitionListFilter) -> Result<u64, AppError>;
    async fn list_for_content_type(
        &self,
        content_type_id: Uuid,
    ) -> Result<Vec<CustomFieldDefinition>, AppError>;
    async fn get(&self, id: Uuid) -> Result<Option<CustomFieldDefinition>, AppError>;
    async fn get_by_name(&self, name: &str) -> Result<Option<CustomFieldDefinition>, AppError>;
    /// Resolve a seeded `inventory_content_type` id from its
    /// `(app_label, model)` token (e.g. `("dcim", "site")`). `None` when no
    /// seeded row exists. Used to translate the static content-type tokens the
    /// inventory handlers pass (`dcim.site`, ...) into the M2M-scope id the
    /// definition/value reads key off.
    async fn resolve_content_type_id(
        &self,
        app_label: &str,
        model: &str,
    ) -> Result<Option<Uuid>, AppError>;
    /// List every seeded `inventory_content_type` row so the scope picker can
    /// offer labeled, install-portable types instead of raw uuids.
    async fn list_content_types(
        &self,
        sort: Option<SortSpec>,
        app_labels: &[String],
        models: &[String],
    ) -> Result<Vec<ContentTypeRow>, AppError>;
    async fn create(
        &self,
        input: &NewCustomFieldDefinition,
    ) -> Result<CustomFieldDefinition, AppError>;
    async fn update(
        &self,
        id: Uuid,
        input: &UpdateCustomFieldDefinition,
    ) -> Result<Option<CustomFieldDefinition>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<bool, AppError>;
}

#[derive(Clone)]
pub struct SqlxCustomFieldDefinitionRepository {
    pool: PgPool,
}

impl SqlxCustomFieldDefinitionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// SELECT order is identical to the try_get keys in `map_row` — no aliasing.
//
// `validation_minimum`/`validation_maximum` are NUMERIC but `f64` in the domain,
// and sqlx cannot decode NUMERIC into f64. Without the cast the loud read in
// `map_row` failed — and because this same list backs `RETURNING`, create and
// update ABORTED before commit: setting a Minimum or Maximum on a custom field
// could never succeed, and the operator saw an opaque 500.
const SELECT_COLUMNS: &str = "id, name, label, field_type, description, group_name, required, \
    enforce_unique, is_secret, default_value, weight, \
    validation_minimum::float8 AS validation_minimum, \
    validation_maximum::float8 AS validation_maximum, \
    validation_regex, validation_schema, choice_set_id, related_object_type_id, \
    related_object_filter, ui_visible, ui_editable, is_cloneable, search_weight, filter_logic, \
    created_at, updated_at";
const SELECT_COLUMNS_CF: &str = "cf.id, cf.name, cf.label, cf.field_type, cf.description, cf.group_name, cf.required, \
    cf.enforce_unique, cf.is_secret, cf.default_value, cf.weight, \
    cf.validation_minimum::float8 AS validation_minimum, \
    cf.validation_maximum::float8 AS validation_maximum, \
    cf.validation_regex, cf.validation_schema, cf.choice_set_id, \
    cf.related_object_type_id, cf.related_object_filter, cf.ui_visible, cf.ui_editable, \
    cf.is_cloneable, cf.search_weight, cf.filter_logic, cf.created_at, cf.updated_at";

fn map_row(row: &sqlx::postgres::PgRow) -> Result<CustomFieldDefinition, AppError> {
    let field_type_raw: String = try_column(row, "field_type")?;
    let field_type = CustomFieldType::parse(&field_type_raw).ok_or_else(|| {
        AppError::Database(format!(
            "custom_field.field_type has unknown value '{field_type_raw}'"
        ))
    })?;
    let ui_visible_raw: String = try_column(row, "ui_visible")?;
    let ui_visible = UiVisible::parse(&ui_visible_raw).ok_or_else(|| {
        AppError::Database(format!(
            "custom_field.ui_visible has unknown value '{ui_visible_raw}'"
        ))
    })?;
    let ui_editable_raw: String = try_column(row, "ui_editable")?;
    let ui_editable = UiEditable::parse(&ui_editable_raw).ok_or_else(|| {
        AppError::Database(format!(
            "custom_field.ui_editable has unknown value '{ui_editable_raw}'"
        ))
    })?;
    let filter_logic_raw: String = try_column(row, "filter_logic")?;
    let filter_logic = FilterLogic::parse(&filter_logic_raw).ok_or_else(|| {
        AppError::Database(format!(
            "custom_field.filter_logic has unknown value '{filter_logic_raw}'"
        ))
    })?;

    Ok(CustomFieldDefinition {
        id: try_column(row, "id")?,
        name: try_column(row, "name")?,
        label: try_column(row, "label")?,
        field_type,
        description: try_column(row, "description")?,
        group_name: try_column(row, "group_name")?,
        required: try_column(row, "required")?,
        enforce_unique: try_column(row, "enforce_unique")?,
        is_secret: try_column(row, "is_secret")?,
        default_value: try_column(row, "default_value")?,
        weight: try_column(row, "weight")?,
        validation_minimum: try_column(row, "validation_minimum")?,
        validation_maximum: try_column(row, "validation_maximum")?,
        validation_regex: try_column(row, "validation_regex")?,
        validation_schema: try_column(row, "validation_schema")?,
        choice_set_id: try_column(row, "choice_set_id")?,
        related_object_type_id: try_column(row, "related_object_type_id")?,
        related_object_filter: try_column(row, "related_object_filter")?,
        ui_visible,
        ui_editable,
        is_cloneable: try_column(row, "is_cloneable")?,
        search_weight: try_column(row, "search_weight")?,
        filter_logic,
        object_types: Vec::new(),
        created_at: try_column::<DateTime<Utc>>(row, "created_at")?,
        updated_at: try_column::<DateTime<Utc>>(row, "updated_at")?,
    })
}

fn try_column<'r, T>(row: &'r sqlx::postgres::PgRow, column: &str) -> Result<T, AppError>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column)
        .map_err(|e| AppError::Database(format!("custom_field.{column}: {e}")))
}

impl SqlxCustomFieldDefinitionRepository {
    async fn retire_definition_fields(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        removed_scopes: Option<&[Uuid]>,
    ) -> Result<bool, AppError> {
        use crate::contexts::secrets::domain::SecretUsageBinding;
        use crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::{lock_secrets, retire_usage};
        let Some(definition) =
            sqlx::query!("SELECT name FROM custom_field WHERE id = $1 FOR UPDATE", id)
                .fetch_optional(&mut **transaction)
                .await?
        else {
            return Ok(false);
        };
        let scopes = sqlx::query!("SELECT content_type_id FROM custom_field_object_type WHERE custom_field_id = $1 ORDER BY content_type_id", id)
            .fetch_all(&mut **transaction).await?;
        let subjects = native_field_subjects(transaction).await?;
        let mut entity_ids = Vec::new();
        let scope_ids: Vec<Uuid> = scopes
            .into_iter()
            .map(|row| row.content_type_id)
            .filter(|scope| removed_scopes.is_none_or(|ids| ids.contains(scope)))
            .collect();
        // Reference-only/template scopes have no native writable subjects.
        // Preserve ambiguous historical usage instead of inventing an owner.
        for native in subjects
            .iter()
            .filter(|native| scope_ids.contains(&native.content_type_id))
        {
            // Table identifiers are from the native registry; only this definition's persisted field usages join.
            let statement = format!(
                "SELECT e.id FROM public.\"{}\" e JOIN secret_usage_binding u ON u.consumer_id = e.id WHERE u.consumer_type = 'inventory_custom_field' AND u.owner_scope_type = 'inventory_custom_field' AND u.owner_scope_id = e.id AND u.field_key = $1 ORDER BY e.id FOR UPDATE OF e",
                native.table_name.replace('"', "\"\"")
            );
            let rows = sqlx::query(&statement)
                .bind(&definition.name)
                .fetch_all(&mut **transaction)
                .await?;
            entity_ids.extend(
                rows.into_iter()
                    .map(|row| row.try_get::<Uuid, _>("id"))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        let usages = sqlx::query_as!(SecretUsageBinding, "SELECT id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id FROM secret_usage_binding WHERE consumer_type = 'inventory_custom_field' AND owner_scope_type = 'inventory_custom_field' AND owner_scope_id = consumer_id AND consumer_id = ANY($1) AND field_key = $2 ORDER BY secret_id", &entity_ids, definition.name)
            .fetch_all(&mut **transaction).await?;
        lock_secrets(
            transaction,
            &usages.iter().map(|row| row.secret_id).collect::<Vec<_>>(),
        )
        .await?;
        for usage in usages {
            sqlx::query!("DELETE FROM secret_usage_binding WHERE id = $1", usage.id)
                .execute(&mut **transaction)
                .await?;
            retire_usage(transaction, &usage).await?;
        }
        Ok(true)
    }
}

impl SqlxCustomFieldDefinitionRepository {
    async fn load_object_types(
        &self,
        executor: impl sqlx::PgExecutor<'_>,
        custom_field_id: Uuid,
    ) -> Result<Vec<Uuid>, AppError> {
        let rows = sqlx::query(
            "SELECT content_type_id FROM custom_field_object_type \
             WHERE custom_field_id = $1 ORDER BY content_type_id",
        )
        .bind(custom_field_id)
        .fetch_all(executor)
        .await
        .map_err(classify_sqlx_error_owned)?;
        rows.iter()
            .map(|row| {
                row.try_get("content_type_id").map_err(|e| {
                    AppError::Database(format!("custom_field_object_type.content_type_id: {e}"))
                })
            })
            .collect()
    }

    async fn hydrate(
        &self,
        mut definition: CustomFieldDefinition,
    ) -> Result<CustomFieldDefinition, AppError> {
        definition.object_types = self.load_object_types(&self.pool, definition.id).await?;
        Ok(definition)
    }

    /// Hydrate `object_types` for a whole page in ONE query.
    ///
    /// The per-row [`Self::hydrate`] in a list loop was a nested N+1: every
    /// definition on the page cost its own `custom_field_object_type` SELECT
    /// (16 extra round-trips on the live `dcim.device` page). One `= ANY($1)`
    /// grouped in memory replaces all of them.
    async fn hydrate_all(
        &self,
        mut definitions: Vec<CustomFieldDefinition>,
    ) -> Result<Vec<CustomFieldDefinition>, AppError> {
        if definitions.is_empty() {
            return Ok(definitions);
        }
        let ids: Vec<Uuid> = definitions.iter().map(|definition| definition.id).collect();
        let rows = sqlx::query(
            "SELECT custom_field_id, content_type_id FROM custom_field_object_type \
             WHERE custom_field_id = ANY($1) ORDER BY content_type_id",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        let mut by_field: std::collections::HashMap<Uuid, Vec<Uuid>> =
            std::collections::HashMap::with_capacity(definitions.len());
        for row in &rows {
            let field_id: Uuid = row.try_get("custom_field_id").map_err(|e| {
                AppError::Database(format!("custom_field_object_type.custom_field_id: {e}"))
            })?;
            let content_type_id: Uuid = row.try_get("content_type_id").map_err(|e| {
                AppError::Database(format!("custom_field_object_type.content_type_id: {e}"))
            })?;
            by_field.entry(field_id).or_default().push(content_type_id);
        }
        for definition in &mut definitions {
            definition.object_types = by_field.remove(&definition.id).unwrap_or_default();
        }
        Ok(definitions)
    }

    fn filter_column(param: &str) -> Result<ListColumnFilter, AppError> {
        CUSTOM_FIELD_DEFINITION_COLUMNS
            .iter()
            .find_map(|column| column.filter.filter(|filter| filter.param == param))
            .ok_or_else(|| {
                AppError::Database(format!(
                    "custom_field list filter {param:?} is not registered"
                ))
            })
    }

    fn push_list_filters(
        builder: &mut QueryBuilder<'_, sqlx::Postgres>,
        filter: &CustomFieldDefinitionListFilter,
    ) -> Result<(), AppError> {
        let mut where_state = WhereState::new();

        if let Some(search) = filter.search.as_deref() {
            let trimmed = search.trim();
            if !trimmed.is_empty() {
                let pattern = format!("%{trimmed}%");
                where_state.and(builder);
                builder.push("(cf.name ILIKE ");
                builder.push_bind(pattern.clone());
                builder.push(" OR cf.label ILIKE ");
                builder.push_bind(pattern.clone());
                builder.push(" OR cf.group_name ILIKE ");
                builder.push_bind(pattern);
                builder.push(")");
            }
        }

        let field_type = Self::filter_column("field_type")?;
        push_string_in(
            builder,
            &mut where_state,
            field_type.sql,
            &filter.field_type,
        );
        let group_name = Self::filter_column("group_name")?;
        push_string_in(
            builder,
            &mut where_state,
            group_name.sql,
            &filter.group_name,
        );
        let ui_visible = Self::filter_column("ui_visible")?;
        push_string_in(
            builder,
            &mut where_state,
            ui_visible.sql,
            &filter.ui_visible,
        );
        let ui_editable = Self::filter_column("ui_editable")?;
        push_string_in(
            builder,
            &mut where_state,
            ui_editable.sql,
            &filter.ui_editable,
        );
        let filter_logic = Self::filter_column("filter_logic")?;
        push_string_in(
            builder,
            &mut where_state,
            filter_logic.sql,
            &filter.filter_logic,
        );
        let choice_set_id = Self::filter_column("choice_set_id")?;
        push_uuid_in(
            builder,
            &mut where_state,
            choice_set_id.sql,
            &filter.choice_set_id,
        );
        let related_object_type_id = Self::filter_column("related_object_type_id")?;
        push_uuid_in(
            builder,
            &mut where_state,
            related_object_type_id.sql,
            &filter.related_object_type_id,
        );
        let object_type_id = Self::filter_column("object_type_id")?;
        push_uuid_in(
            builder,
            &mut where_state,
            object_type_id.sql,
            &filter.object_type_id,
        );

        Ok(())
    }
}

#[async_trait]
impl CustomFieldDefinitionRepository for SqlxCustomFieldDefinitionRepository {
    async fn list_page(
        &self,
        filter: &CustomFieldDefinitionListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CustomFieldDefinition>, AppError> {
        let mut builder = QueryBuilder::new("SELECT ");
        builder.push(SELECT_COLUMNS_CF);
        builder.push(
            " FROM custom_field cf \
             LEFT JOIN custom_field_choice_set cs ON cs.id = cf.choice_set_id \
             LEFT JOIN inventory_content_type related_ct ON related_ct.id = cf.related_object_type_id \
             LEFT JOIN custom_field_object_type cfot_filter ON cfot_filter.custom_field_id = cf.id",
        );
        Self::push_list_filters(&mut builder, filter)?;
        builder.push(
            " GROUP BY cf.id, cf.name, cf.label, cf.field_type, cf.description, cf.group_name, \
             cf.required, cf.enforce_unique, cf.is_secret, cf.default_value, cf.weight, \
             cf.validation_minimum, cf.validation_maximum, cf.validation_regex, cf.validation_schema, \
             cf.choice_set_id, cf.related_object_type_id, cf.related_object_filter, cf.ui_visible, \
             cf.ui_editable, cf.is_cloneable, cf.search_weight, cf.filter_logic, cf.created_at, \
             cf.updated_at, cs.name, related_ct.app_label, related_ct.model",
        );
        push_order_by(
            &mut builder,
            CUSTOM_FIELD_DEFINITION_COLUMNS,
            filter.sort.as_ref(),
            "cf.weight, cf.name",
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
        let definitions = rows.iter().map(map_row).collect::<Result<Vec<_>, _>>()?;
        self.hydrate_all(definitions).await
    }

    async fn count(&self, filter: &CustomFieldDefinitionListFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::new(
            "SELECT COUNT(DISTINCT cf.id) FROM custom_field cf \
             LEFT JOIN custom_field_choice_set cs ON cs.id = cf.choice_set_id \
             LEFT JOIN inventory_content_type related_ct ON related_ct.id = cf.related_object_type_id \
             LEFT JOIN custom_field_object_type cfot_filter ON cfot_filter.custom_field_id = cf.id",
        );
        Self::push_list_filters(&mut builder, filter)?;
        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.try_get::<i64, _>(0)
            .map(|value| value as u64)
            .map_err(|error| AppError::Database(format!("custom_field.count: {error}")))
    }

    async fn list_for_content_type(
        &self,
        content_type_id: Uuid,
    ) -> Result<Vec<CustomFieldDefinition>, AppError> {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM custom_field \
             WHERE id IN ( \
                 SELECT custom_field_id FROM custom_field_object_type \
                 WHERE content_type_id = $1 \
             ) \
             ORDER BY weight, name"
        );
        let rows = sqlx::query(&query)
            .bind(content_type_id)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let definitions = rows.iter().map(map_row).collect::<Result<Vec<_>, _>>()?;
        self.hydrate_all(definitions).await
    }

    async fn resolve_content_type_id(
        &self,
        app_label: &str,
        model: &str,
    ) -> Result<Option<Uuid>, AppError> {
        crate::contexts::inventory::infrastructure::persistence::repositories::lookup_content_type_id(
            &self.pool, app_label, model,
        )
        .await
    }

    async fn list_content_types(
        &self,
        sort: Option<SortSpec>,
        app_labels: &[String],
        models: &[String],
    ) -> Result<Vec<ContentTypeRow>, AppError> {
        crate::contexts::inventory::infrastructure::persistence::repositories::list_content_types(
            &self.pool, sort, app_labels, models,
        )
        .await
    }

    async fn get(&self, id: Uuid) -> Result<Option<CustomFieldDefinition>, AppError> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM custom_field WHERE id = $1");
        let row = sqlx::query(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        match row {
            Some(row) => Ok(Some(self.hydrate(map_row(&row)?).await?)),
            None => Ok(None),
        }
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<CustomFieldDefinition>, AppError> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM custom_field WHERE name = $1");
        let row = sqlx::query(&query)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        match row {
            Some(row) => Ok(Some(self.hydrate(map_row(&row)?).await?)),
            None => Ok(None),
        }
    }

    async fn create(
        &self,
        input: &NewCustomFieldDefinition,
    ) -> Result<CustomFieldDefinition, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;

        let insert = format!(
            "INSERT INTO custom_field ( \
                 name, label, field_type, description, group_name, required, enforce_unique, \
                 is_secret, default_value, weight, validation_minimum, validation_maximum, \
                 validation_regex, validation_schema, choice_set_id, related_object_type_id, \
                 related_object_filter, ui_visible, ui_editable, is_cloneable, search_weight, \
                 filter_logic \
             ) VALUES ( \
                 $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
                 $18, $19, $20, $21, $22 \
             ) RETURNING {SELECT_COLUMNS}"
        );
        let row = sqlx::query(&insert)
            .bind(&input.name)
            .bind(&input.label)
            .bind(input.field_type.as_str())
            .bind(&input.description)
            .bind(&input.group_name)
            .bind(input.required)
            .bind(input.enforce_unique)
            .bind(input.is_secret)
            .bind(&input.default_value)
            .bind(input.weight)
            .bind(input.validation_minimum)
            .bind(input.validation_maximum)
            .bind(&input.validation_regex)
            .bind(&input.validation_schema)
            .bind(input.choice_set_id)
            .bind(input.related_object_type_id)
            .bind(&input.related_object_filter)
            .bind(input.ui_visible.as_str())
            .bind(input.ui_editable.as_str())
            .bind(input.is_cloneable)
            .bind(input.search_weight)
            .bind(input.filter_logic.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut definition = map_row(&row)?;

        for content_type_id in &input.object_types {
            validate_scope(&mut tx, *content_type_id).await?;
            sqlx::query(
                "INSERT INTO custom_field_object_type (custom_field_id, content_type_id) \
                 VALUES ($1, $2)",
            )
            .bind(definition.id)
            .bind(content_type_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        definition.object_types = input.object_types.clone();
        Ok(definition)
    }

    async fn update(
        &self,
        id: Uuid,
        input: &UpdateCustomFieldDefinition,
    ) -> Result<Option<CustomFieldDefinition>, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        lock_field_catalog_for_definition(&mut tx).await?;
        if sqlx::query!("SELECT id FROM custom_field WHERE id = $1 FOR UPDATE", id)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        if input.is_secret == Some(false) {
            Self::retire_definition_fields(&mut tx, id, None).await?;
        } else if let Some(retained) = &input.object_types {
            let old = self.load_object_types(&mut *tx, id).await?;
            let removed: Vec<Uuid> = old
                .into_iter()
                .filter(|scope| !retained.contains(scope))
                .collect();
            if !removed.is_empty() {
                Self::retire_definition_fields(&mut tx, id, Some(&removed)).await?;
            }
        }

        // COALESCE applies only provided scalar fields. The nested
        // `Option<Option<_>>` columns use a CASE/`$set` boolean so callers can
        // explicitly clear them to NULL.
        let update = format!(
            "UPDATE custom_field SET \
                 label = COALESCE($2, label), \
                 field_type = COALESCE($3, field_type), \
                 description = COALESCE($4, description), \
                 group_name = COALESCE($5, group_name), \
                 required = COALESCE($6, required), \
                 enforce_unique = COALESCE($7, enforce_unique), \
                 is_secret = COALESCE($8, is_secret), \
                 default_value = CASE WHEN $9::boolean THEN $10 ELSE default_value END, \
                 weight = COALESCE($11, weight), \
                 validation_minimum = CASE WHEN $12::boolean THEN $13 ELSE validation_minimum END, \
                 validation_maximum = CASE WHEN $14::boolean THEN $15 ELSE validation_maximum END, \
                 validation_regex = COALESCE($16, validation_regex), \
                 validation_schema = CASE WHEN $17::boolean THEN $18 ELSE validation_schema END, \
                 choice_set_id = CASE WHEN $19::boolean THEN $20 ELSE choice_set_id END, \
                 related_object_type_id = \
                     CASE WHEN $21::boolean THEN $22 ELSE related_object_type_id END, \
                 related_object_filter = \
                     CASE WHEN $23::boolean THEN $24 ELSE related_object_filter END, \
                 ui_visible = COALESCE($25, ui_visible), \
                 ui_editable = COALESCE($26, ui_editable), \
                 is_cloneable = COALESCE($27, is_cloneable), \
                 search_weight = COALESCE($28, search_weight), \
                 filter_logic = COALESCE($29, filter_logic), \
                 updated_at = now() \
             WHERE id = $1 \
             RETURNING {SELECT_COLUMNS}"
        );

        let (default_set, default_value) = nullable_set(&input.default_value);
        let (min_set, min_value) = nullable_set(&input.validation_minimum);
        let (max_set, max_value) = nullable_set(&input.validation_maximum);
        let (schema_set, schema_value) = nullable_set(&input.validation_schema);
        let (choice_set, choice_value) = nullable_set(&input.choice_set_id);
        let (related_type_set, related_type_value) = nullable_set(&input.related_object_type_id);
        let (related_filter_set, related_filter_value) = nullable_set(&input.related_object_filter);

        let row = sqlx::query(&update)
            .bind(id)
            .bind(input.label.as_deref())
            .bind(input.field_type.map(CustomFieldType::as_str))
            .bind(input.description.as_deref())
            .bind(input.group_name.as_deref())
            .bind(input.required)
            .bind(input.enforce_unique)
            .bind(input.is_secret)
            .bind(default_set)
            .bind(default_value)
            .bind(input.weight)
            .bind(min_set)
            .bind(min_value)
            .bind(max_set)
            .bind(max_value)
            .bind(input.validation_regex.as_deref())
            .bind(schema_set)
            .bind(schema_value)
            .bind(choice_set)
            .bind(choice_value)
            .bind(related_type_set)
            .bind(related_type_value)
            .bind(related_filter_set)
            .bind(related_filter_value)
            .bind(input.ui_visible.map(UiVisible::as_str))
            .bind(input.ui_editable.map(UiEditable::as_str))
            .bind(input.is_cloneable)
            .bind(input.search_weight)
            .bind(input.filter_logic.map(FilterLogic::as_str))
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;

        let Some(row) = row else {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let mut definition = map_row(&row)?;

        if let Some(object_types) = &input.object_types {
            sqlx::query("DELETE FROM custom_field_object_type WHERE custom_field_id = $1")
                .bind(definition.id)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            for content_type_id in object_types {
                validate_scope(&mut tx, *content_type_id).await?;
                sqlx::query(
                    "INSERT INTO custom_field_object_type (custom_field_id, content_type_id) \
                     VALUES ($1, $2)",
                )
                .bind(definition.id)
                .bind(content_type_id)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            }
            definition.object_types = object_types.clone();
        } else {
            definition.object_types = self.load_object_types(&mut *tx, definition.id).await?;
        }

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(definition))
    }

    async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        let mut transaction = self.pool.begin().await?;
        lock_field_catalog_for_definition(&mut transaction).await?;
        if !Self::retire_definition_fields(&mut transaction, id, None).await? {
            return Ok(false);
        }
        // The `custom_field_object_type` rows cascade on the FK.
        let result = sqlx::query("DELETE FROM custom_field WHERE id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Maps an `Option<Option<T>>` patch field to a `(set, value)` pair the
/// CASE/COALESCE SQL consumes: `Some(inner)` sets the column (clearing to NULL
/// when `inner` is `None`); `None` leaves it untouched.
fn nullable_set<T: Clone>(field: &Option<Option<T>>) -> (bool, Option<T>) {
    match field {
        Some(inner) => (true, inner.clone()),
        None => (false, None),
    }
}
