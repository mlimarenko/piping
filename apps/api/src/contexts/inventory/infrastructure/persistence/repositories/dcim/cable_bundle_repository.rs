//! Typed CRUD and paginated lists for cable bundles.

use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::SqlxSecretRepository;
use crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch;
use sqlx::postgres::PgRow;
use sqlx::{Acquire, Postgres, Transaction};
use sqlx::{PgPool, QueryBuilder, Row};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub use crate::contexts::inventory::domain::CableBundle as CableBundleRecord;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const CABLE_BUNDLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "external_id",
        "external_id",
        "inventory.cableBundles.columns.externalId",
    ),
    ListColumn::sortable("name", "name", "inventory.cableBundles.columns.name"),
    ListColumn::sortable(
        "comments",
        "comments",
        "inventory.cableBundles.columns.comments",
    ),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.cableBundles.columns.description",
    ),
    ListColumn::sortable("tags", "tags", "inventory.cableBundles.columns.tags"),
    ListColumn::sortable(
        "piping_synced_at",
        "piping_synced_at",
        "inventory.cableBundles.columns.syncedAt",
    ),
];

const FIELDS: &str = "id, external_id, name, description, comments, owner_id, tags, custom_field_data, created, last_updated, piping_synced_at";
const INSERT: &str = "INSERT INTO dcim_cablebundle (id, name, description, comments, owner_id, tags, custom_field_data, created, last_updated) VALUES ($1, $2, COALESCE($3, ''), COALESCE($4, ''), $5, $6, COALESCE($7, '{}'::jsonb), NOW(), NOW()) RETURNING *";

#[derive(Debug, Clone)]
pub struct NewCableBundle {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Absent fields are preserved; `Some(None)` explicitly clears the owner.
#[derive(Debug, Clone, Default)]
pub struct UpdateCableBundle {
    pub name: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    /// Validated key-level delta, merged atomically with the stored JSON object.
    pub custom_fields: Option<serde_json::Value>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async inventory port; callers do not require an implementation-independent Send future"
)]
pub trait CableBundleRepository: Send + Sync {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CableBundleRecord>, AppError>;
    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CableBundleRecord>, i64), AppError>;
    async fn create(&self, item: &NewCableBundle) -> Result<CableBundleRecord, AppError>;
    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCableBundle,
    ) -> Result<Option<CableBundleRecord>, AppError>;
    async fn bulk_create(
        &self,
        items: &[NewCableBundle],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<CableBundleRecord>, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SqlxCableBundleRepository {
    pool: PgPool,
}

impl SqlxCableBundleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_row(row: &PgRow) -> Result<CableBundleRecord, AppError> {
        Ok(CableBundleRecord {
            id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
            external_id: row
                .try_get("external_id")
                .map_err(classify_sqlx_error_owned)?,
            name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
            description: row
                .try_get("description")
                .map_err(classify_sqlx_error_owned)?,
            comments: row.try_get("comments").map_err(classify_sqlx_error_owned)?,
            owner_id: row.try_get("owner_id").map_err(classify_sqlx_error_owned)?,
            tags: row.try_get("tags").map_err(classify_sqlx_error_owned)?,
            custom_fields: row
                .try_get("custom_field_data")
                .map_err(classify_sqlx_error_owned)?,
            created: row.try_get("created").map_err(classify_sqlx_error_owned)?,
            last_updated: row
                .try_get("last_updated")
                .map_err(classify_sqlx_error_owned)?,
            piping_synced_at: row
                .try_get("piping_synced_at")
                .map_err(classify_sqlx_error_owned)?,
        })
    }

    async fn insert(
        connection: &mut sqlx::PgConnection,
        item: &NewCableBundle,
    ) -> Result<CableBundleRecord, AppError> {
        validate(
            Some(&item.name),
            item.description.as_deref(),
            item.custom_fields.as_ref(),
        )?;
        let row = sqlx::query(INSERT)
            .bind(item.id)
            .bind(&item.name)
            .bind(&item.description)
            .bind(&item.comments)
            .bind(item.owner_id)
            .bind(&item.tags)
            .bind(&item.custom_fields)
            .fetch_one(connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Self::parse_row(&row)
    }

    async fn create_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        item: &NewCableBundle,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<CableBundleRecord, AppError> {
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        super::super::extras::tag_repository::lock_tag_catalog_for_usage(tx).await?;
        lock_field_intent_batch(tx, &inputs).await?;
        let record = Self::insert(&mut **tx, item).await?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(tx, inputs)
            .await?;
        Ok(record)
    }

    pub async fn create_with_secret_intents(
        &self,
        item: &NewCableBundle,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<CableBundleRecord, AppError> {
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        let record = self.create_in_transaction(&mut tx, item, inputs).await?;
        tx.commit().await?;
        Ok(record)
    }

    pub async fn bulk_create_with_secret_intents(
        &self,
        items: &[NewCableBundle],
        mut inputs: HashMap<Uuid, Vec<ApplySecretFieldIntentInput>>,
    ) -> Result<super::super::bulk_insert::BulkCreateResult<CableBundleRecord>, AppError> {
        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;
        super::super::extras::tag_repository::lock_tag_catalog_for_usage(&mut tx).await?;
        let batch_inputs: Vec<_> = inputs.values().flatten().cloned().collect();
        lock_field_intent_batch(&mut tx, &batch_inputs).await?;
        let mut created = Vec::with_capacity(items.len());
        let mut errors = Vec::new();
        for item in items {
            let mut savepoint = tx.begin().await?;
            match self
                .create_in_transaction(
                    &mut savepoint,
                    item,
                    inputs.remove(&item.id).unwrap_or_default(),
                )
                .await
            {
                Ok(record) => {
                    savepoint.commit().await?;
                    created.push(record);
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    errors.push(format!(
                        "Failed to insert cable bundle '{}': {error}",
                        item.name
                    ));
                }
            }
        }
        tx.commit().await?;
        Ok(super::super::bulk_insert::BulkCreateResult { created, errors })
    }
}

impl CableBundleRepository for SqlxCableBundleRepository {
    async fn get_by_id(&self, id: Uuid) -> Result<Option<CableBundleRecord>, AppError> {
        let sql = format!("SELECT {FIELDS} FROM dcim_cablebundle WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(Self::parse_row).transpose()
    }

    async fn list(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<String>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<CableBundleRecord>, i64), AppError> {
        let mut query = QueryBuilder::new(format!(
            "SELECT {FIELDS}, COUNT(*) OVER() AS total_count FROM dcim_cablebundle"
        ));
        push_filters(&mut query, search.as_deref());
        push_order_by(&mut query, CABLE_BUNDLE_COLUMNS, sort.as_ref(), "name");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)));
        query
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let total = match rows.first() {
            Some(row) => row
                .try_get("total_count")
                .map_err(classify_sqlx_error_owned)?,
            None => {
                let mut count = QueryBuilder::new("SELECT COUNT(*) FROM dcim_cablebundle");
                push_filters(&mut count, search.as_deref());
                count
                    .build_query_scalar::<i64>()
                    .fetch_one(&self.pool)
                    .await
                    .map_err(classify_sqlx_error_owned)?
            }
        };
        let items = rows
            .iter()
            .map(Self::parse_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn create(&self, item: &NewCableBundle) -> Result<CableBundleRecord, AppError> {
        self.create_with_secret_intents(item, Vec::new()).await
    }

    async fn update(
        &self,
        id: Uuid,
        changes: &UpdateCableBundle,
    ) -> Result<Option<CableBundleRecord>, AppError> {
        self.update_with_secret_intents(id, changes, Vec::new())
            .await
    }

    async fn bulk_create(
        &self,
        items: &[NewCableBundle],
    ) -> Result<super::super::bulk_insert::BulkCreateResult<CableBundleRecord>, AppError> {
        self.bulk_create_with_secret_intents(items, HashMap::new())
            .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let deleted =
            super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_cablebundle", id)
                .await?;
        if deleted == 0 {
            return Err(AppError::NotFound("Cable bundle not found".to_string()));
        }
        Ok(())
    }
}

impl SqlxCableBundleRepository {
    pub async fn update_with_secret_intents(
        &self,
        id: Uuid,
        changes: &UpdateCableBundle,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Option<CableBundleRecord>, AppError> {
        validate(
            changes.name.as_deref(),
            changes.description.as_deref(),
            changes.custom_fields.as_ref(),
        )?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::extras::tag_repository::lock_tag_catalog_for_usage(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let row = sqlx::query("UPDATE dcim_cablebundle SET name = COALESCE($2, name), description = COALESCE($3, description), comments = COALESCE($4, comments), owner_id = CASE WHEN $5 THEN $6 ELSE owner_id END, tags = COALESCE($7, tags), custom_field_data = custom_field_data || COALESCE($8, '{}'::jsonb), last_updated = NOW() WHERE id = $1 RETURNING *")
            .bind(id).bind(&changes.name).bind(&changes.description).bind(&changes.comments)
            .bind(changes.owner_id.is_some()).bind(changes.owner_id.flatten())
            .bind(&changes.tags).bind(&changes.custom_fields)
            .fetch_optional(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let record = Self::parse_row(&row)?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        tx.commit().await?;
        Ok(Some(record))
    }
}

fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, search: Option<&str>) {
    push_search_ilike(
        builder,
        &mut WhereState::new(),
        &["external_id", "name", "description", "comments", "tags"],
        search,
    );
}

fn validate(
    name: Option<&str>,
    description: Option<&str>,
    custom_fields: Option<&serde_json::Value>,
) -> Result<(), AppError> {
    if let Some(name) = name {
        if name.trim().is_empty() || name.chars().count() > 100 {
            return Err(AppError::Validation(
                "name must contain 1 to 100 characters".to_string(),
            ));
        }
    }
    if description.is_some_and(|value| value.chars().count() > 200) {
        return Err(AppError::Validation(
            "description must not exceed 200 characters".to_string(),
        ));
    }
    if custom_fields.is_some_and(|value| !value.is_object()) {
        return Err(AppError::Validation(
            "custom_fields must be an object".to_string(),
        ));
    }
    Ok(())
}
