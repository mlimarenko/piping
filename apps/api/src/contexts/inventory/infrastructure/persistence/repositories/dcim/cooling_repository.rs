//! One transactional persistence owner for the cooling graph and its templates.

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::dcim::cooling::{
    CoolingKind, CoolingMeasurement, DIAMETER_UNITS, FLOW_RATE_UNITS,
};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::SqlxSecretRepository;
use crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const COOLING_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
];

#[derive(Default)]
pub struct CoolingFilter {
    pub references: Vec<(&'static str, Vec<Uuid>)>,
    pub status: Vec<String>,
    pub types: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Clone)]
pub struct SqlxCoolingRepository {
    pool: PgPool,
    secret_intents: Arc<SecretFieldIntentService<SqlxSecretRepository>>,
}

impl SqlxCoolingRepository {
    pub async fn feed_count(&self, source: Uuid) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_coolingfeed WHERE cooling_source_id=$1")
            .bind(source)
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }
    pub fn new(pool: PgPool) -> Self {
        Self {
            secret_intents: Arc::new(SecretFieldIntentService::new(Arc::new(
                SqlxSecretRepository::new(pool.clone()),
            ))),
            pool,
        }
    }

    pub async fn get(&self, kind: CoolingKind, id: Uuid) -> Result<Option<Value>, AppError> {
        let sql = format!("SELECT to_jsonb(t) FROM {} t WHERE id=$1", kind.table());
        sqlx::query_scalar(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn list(
        &self,
        kind: CoolingKind,
        filter: CoolingFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<Value>, i64), AppError> {
        for (column, _) in &filter.references {
            if *column != "id" && !kind.fields().contains(column) {
                return Err(AppError::Validation(format!(
                    "{column} is not a filter for this cooling model"
                )));
            }
        }
        if !filter.status.is_empty() && !kind.fields().contains(&"status") {
            return Err(AppError::Validation(
                "status is not a filter for this cooling model".into(),
            ));
        }
        if !filter.types.is_empty() && !kind.fields().contains(&"type") {
            return Err(AppError::Validation(
                "type is not a filter for this cooling model".into(),
            ));
        }
        fn filters(q: &mut QueryBuilder<'_, Postgres>, filter: &CoolingFilter) {
            let mut state = WhereState::new();
            for (column, ids) in &filter.references {
                push_uuid_in(q, &mut state, column, ids);
            }
            push_string_in(q, &mut state, "status", &filter.status);
            push_string_in(q, &mut state, "type", &filter.types);
            push_search_ilike(
                q,
                &mut state,
                &["name", "description"],
                filter.search.as_deref(),
            );
        }
        let mut count = QueryBuilder::new(format!("SELECT COUNT(*) FROM {}", kind.table()));
        filters(&mut count, &filter);
        let total = count
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(format!("SELECT to_jsonb(t) FROM {} t", kind.table()));
        filters(&mut query, &filter);
        push_order_by(&mut query, COOLING_COLUMNS, filter.sort.as_ref(), "name");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_scalar()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok((items, total))
    }

    /// Merge a presence-preserving patch while holding the row lock. Metadata
    /// and secret bindings either both commit or both roll back.
    pub async fn save(
        &self,
        kind: CoolingKind,
        id: Uuid,
        mut changes: Value,
        create: bool,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Option<Value>, AppError> {
        let object = changes
            .as_object_mut()
            .ok_or_else(|| AppError::Validation("cooling payload must be an object".into()))?;
        if object
            .keys()
            .any(|key| !kind.fields().contains(&key.as_str()))
        {
            return Err(AppError::Validation("unknown cooling field".into()));
        }
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let mut record = if create {
            defaults(kind)
        } else {
            let sql = format!(
                "SELECT to_jsonb(t) FROM {} t WHERE id=$1 FOR UPDATE",
                kind.table()
            );
            let Some(record) = sqlx::query_scalar::<_, Value>(&sql)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?
            else {
                return Ok(None);
            };
            record
        };
        if create {
            // Optional create fields use the model defaults, whereas an explicit
            // null in a patch remains a null and is validated below.
            for field in [
                "status",
                "label",
                "description",
                "comments",
                "tags",
                "custom_field_data",
            ] {
                if object.get(field).is_some_and(Value::is_null) {
                    object.remove(field);
                }
            }
        } else if matches!(kind, CoolingKind::Intake | CoolingKind::Outflow)
            && object
                .get("device_id")
                .is_some_and(|value| value != &record["device_id"])
        {
            return Err(AppError::Validation(
                "Cooling components cannot be moved to another device".into(),
            ));
        }
        let current = record
            .as_object_mut()
            .ok_or_else(|| AppError::Internal("Invalid cooling record".into()))?;
        for (key, value) in object.iter() {
            if key == "custom_field_data" {
                let fields = value.as_object().ok_or_else(|| {
                    AppError::Validation("custom_fields must be an object".into())
                })?;
                let stored = current
                    .entry(key.clone())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| AppError::Internal("Invalid stored custom fields".into()))?;
                stored.extend(fields.clone());
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
        current.insert("id".into(), json!(id));
        validate(kind, &mut record)?;
        let table = kind.table();
        // Identifiers come exclusively from CoolingKind. All values, including
        // the composite record, remain bound parameters.
        let fields = kind.fields().join(",");
        let sql = if create {
            format!(
                "INSERT INTO {table} (id,{fields},created,last_updated) SELECT r.id,{fields},NOW(),NOW() FROM jsonb_populate_record(NULL::{table},$1) r RETURNING to_jsonb({table}.*)"
            )
        } else {
            let values = kind
                .fields()
                .iter()
                .map(|field| format!("r.{field}"))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "UPDATE {table} t SET ({fields},last_updated)=(SELECT {values},NOW() FROM jsonb_populate_record(NULL::{table},$1) r) WHERE t.id=$2 RETURNING to_jsonb(t.*)"
            )
        };
        let row = if create {
            sqlx::query_scalar(&sql)
                .bind(&record)
                .fetch_one(&mut *tx)
                .await
        } else {
            sqlx::query_scalar(&sql)
                .bind(&record)
                .bind(id)
                .fetch_one(&mut *tx)
                .await
        }
        .map_err(classify_sqlx_error_owned)?;
        self.secret_intents
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(row))
    }

    pub async fn delete(&self, kind: CoolingKind, id: Uuid) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let count = super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            kind.table(),
            id,
        )
        .await?;
        if count == 0 {
            return Err(AppError::NotFound("Cooling record not found".into()));
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(())
    }
}

fn defaults(kind: CoolingKind) -> Value {
    let mut value = json!({});
    for field in kind.fields() {
        value[*field] = Value::Null;
    }
    for field in ["name", "label", "description", "comments"] {
        if kind.fields().contains(&field) {
            value[field] = json!("");
        }
    }
    if kind.fields().contains(&"status") {
        value["status"] = json!("active");
    }
    if kind.has_custom_fields() {
        value["tags"] = json!([]);
        value["custom_field_data"] = json!({});
    }
    value
}

fn choice(value: &Value, field: &str, choices: &[&str], required: bool) -> Result<(), AppError> {
    if value[field].is_null() && !required {
        return Ok(());
    }
    if !value[field].as_str().is_some_and(|v| choices.contains(&v)) {
        return Err(AppError::Validation(format!("Invalid {field}")));
    }
    Ok(())
}

fn validate(kind: CoolingKind, record: &mut Value) -> Result<(), AppError> {
    let name_limit = if matches!(kind, CoolingKind::Source | CoolingKind::Feed) {
        100
    } else {
        64
    };
    for (field, limit, required) in [
        ("name", name_limit, true),
        ("label", 64, false),
        ("description", 200, false),
    ] {
        if !kind.fields().contains(&field) {
            continue;
        }
        let valid = record[field]
            .as_str()
            .is_some_and(|s| s.chars().count() <= limit && (!required || !s.trim().is_empty()));
        if !valid {
            return Err(AppError::Validation(format!(
                "Invalid {field}: maximum {limit} characters"
            )));
        }
    }
    if kind.has_custom_fields()
        && (!record["tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().all(Value::is_string))
            || !record["custom_field_data"].is_object())
    {
        return Err(AppError::Validation(
            "tags must be strings and custom_fields must be an object".into(),
        ));
    }
    if kind.fields().contains(&"comments") && !record["comments"].is_string() {
        return Err(AppError::Validation("comments must be a string".into()));
    }
    for field in kind.fields().iter().filter(|field| field.ends_with("_id")) {
        let required = matches!(*field, "site_id" | "cooling_source_id" | "device_id");
        if record[*field].is_null() && !required {
            continue;
        }
        if !record[*field]
            .as_str()
            .is_some_and(|s| Uuid::parse_str(s).is_ok())
        {
            return Err(AppError::Validation(format!("Invalid {field}")));
        }
    }
    if kind.fields().contains(&"status") {
        choice(
            record,
            "status",
            &["offline", "active", "planned", "failed"],
            true,
        )?;
    }
    if kind == CoolingKind::Source {
        choice(
            record,
            "type",
            &["chiller", "cooling-tower", "dry-cooler", "crac", "crah"],
            true,
        )?;
        choice(
            record,
            "fluid_type",
            &["water", "water-glycol", "dielectric", "refrigerant"],
            false,
        )?;
    } else if kind.fields().contains(&"type") {
        choice(
            record,
            "type",
            &["uqd", "uqdb", "qdc", "camlock", "npt", "bsp", "proprietary"],
            false,
        )?;
    }
    for (field, measurement) in [
        ("cooling_capacity", CoolingMeasurement::Capacity),
        ("diameter", CoolingMeasurement::Diameter),
        ("max_flow", CoolingMeasurement::MaxFlow),
    ] {
        if let Some(value) = record.get(field).filter(|v| !v.is_null()) {
            let number = value
                .as_number()
                .ok_or_else(|| AppError::Validation(format!("{field} must be numeric")))?;
            // Normalize only after checking the original lexeme. Valid padding
            // and exponents must not overflow PostgreSQL's JSON numeric parser.
            record[field] = json!(measurement.validate(number)?);
        }
    }
    for (field, unit, choices) in [
        ("diameter", "diameter_unit", DIAMETER_UNITS),
        ("max_flow", "max_flow_unit", FLOW_RATE_UNITS),
    ] {
        if !kind.fields().contains(&field) {
            continue;
        }
        choice(record, unit, choices, false)?;
        if record[field].is_null() != record[unit].is_null() {
            return Err(AppError::Validation(format!(
                "{field} and {unit} must be supplied or cleared together"
            )));
        }
    }
    if matches!(
        kind,
        CoolingKind::IntakeTemplate | CoolingKind::OutflowTemplate
    ) && record["device_type_id"].is_null() == record["module_type_id"].is_null()
    {
        return Err(AppError::Validation(
            "Exactly one of device_type_id or module_type_id is required".into(),
        ));
    }
    Ok(())
}
