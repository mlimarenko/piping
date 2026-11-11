use std::sync::Arc;

use sqlx::{Acquire, PgPool, Postgres, QueryBuilder, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::contexts::inventory::domain::dcim::module_bay_type::ModuleBayType;
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::SqlxSecretRepository;
use crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const MODULE_BAY_TYPE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "slug", "inventory.resources.fields.slug"),
    ListColumn::sortable("color", "color", "inventory.resources.fields.color"),
    ListColumn::sortable(
        "description",
        "description",
        "inventory.resources.fields.description",
    ),
    ListColumn::fk_filter(
        "manufacturer_id",
        "manufacturer_id",
        "manufacturer_id",
        "manufacturer_id",
        "manufacturers",
        "inventory.resources.fields.manufacturer",
    ),
];

#[derive(Debug, Clone)]
pub struct NewModuleBayType {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<Uuid>,
    pub color: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateModuleBayType {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub manufacturer_id: Option<Option<Uuid>>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct SqlxModuleBayTypeRepository {
    pool: PgPool,
    secret_intents: Arc<SecretFieldIntentService<SqlxSecretRepository>>,
}

impl SqlxModuleBayTypeRepository {
    pub fn new(pool: PgPool) -> Self {
        let secret_intents = Arc::new(SecretFieldIntentService::new(Arc::new(
            SqlxSecretRepository::new(pool.clone()),
        )));
        Self {
            pool,
            secret_intents,
        }
    }

    fn parse(row: &PgRow) -> Result<ModuleBayType, AppError> {
        Ok(ModuleBayType {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            slug: row.try_get("slug")?,
            manufacturer_id: row.try_get("manufacturer_id")?,
            color: row.try_get("color")?,
            description: row.try_get("description")?,
            comments: row.try_get("comments")?,
            owner_id: row.try_get("owner_id")?,
            tags: row.try_get("tags")?,
            custom_fields: row.try_get("custom_field_data")?,
            created: row.try_get("created")?,
            last_updated: row.try_get("last_updated")?,
            external_id: row.try_get("external_id")?,
            piping_synced_at: row.try_get("piping_synced_at")?,
        })
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleBayType>, AppError> {
        sqlx::query("SELECT * FROM dcim_modulebaytype WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(Self::parse)
            .transpose()
    }

    pub async fn list(
        &self,
        manufacturers: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ModuleBayType>, i64), AppError> {
        fn filters(q: &mut QueryBuilder<Postgres>, manufacturers: &[Uuid], search: Option<&str>) {
            let mut state = WhereState::new();
            push_uuid_in(q, &mut state, "manufacturer_id", manufacturers);
            push_search_ilike(
                q,
                &mut state,
                &["name", "slug", "description", "comments"],
                search,
            );
        }
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM dcim_modulebaytype");
        filters(&mut count, manufacturers, search.as_deref());
        let total = count
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new("SELECT * FROM dcim_modulebaytype");
        filters(&mut query, manufacturers, search.as_deref());
        push_order_by(&mut query, MODULE_BAY_TYPE_COLUMNS, sort.as_ref(), "name");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok((
            rows.iter().map(Self::parse).collect::<Result<_, _>>()?,
            total,
        ))
    }

    pub async fn create(
        &self,
        item: &NewModuleBayType,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<ModuleBayType, AppError> {
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        let item = self.insert(&mut tx, item, inputs).await?;
        tx.commit().await?;
        Ok(item)
    }

    pub async fn bulk_create(
        &self,
        items: Vec<(NewModuleBayType, Vec<ApplySecretFieldIntentInput>)>,
    ) -> Result<Vec<Result<ModuleBayType, AppError>>, AppError> {
        let mut tx = super::super::bulk_insert::begin(&self.pool).await?;
        let mut results = Vec::with_capacity(items.len());
        for (item, inputs) in items {
            let mut savepoint = tx.begin().await?;
            match self.insert(&mut savepoint, &item, inputs).await {
                Ok(record) => {
                    savepoint.commit().await?;
                    results.push(Ok(record));
                }
                Err(error) => {
                    savepoint.rollback().await?;
                    results.push(Err(error));
                }
            }
        }
        tx.commit().await?;
        Ok(results)
    }

    async fn insert(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        item: &NewModuleBayType,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<ModuleBayType, AppError> {
        validate(
            Some(&item.name),
            Some(&item.slug),
            Some(&item.color),
            item.description.as_deref(),
            item.custom_fields.as_ref(),
        )?;
        lock_field_intent_batch(tx, &inputs).await?;
        let row = sqlx::query("INSERT INTO dcim_modulebaytype (id,name,slug,manufacturer_id,color,description,comments,owner_id,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,COALESCE($6,''),COALESCE($7,''),$8,$9,COALESCE($10,'{}'::jsonb),NOW(),NOW()) RETURNING *")
            .bind(item.id).bind(&item.name).bind(&item.slug).bind(item.manufacturer_id).bind(&item.color)
            .bind(&item.description).bind(&item.comments).bind(item.owner_id).bind(&item.tags).bind(&item.custom_fields)
            .fetch_one(&mut **tx).await.map_err(classify_sqlx_error_owned)?;
        self.secret_intents
            .apply_intents_in_transaction(tx, inputs)
            .await?;
        Self::parse(&row)
    }

    pub async fn update(
        &self,
        id: Uuid,
        item: &UpdateModuleBayType,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Option<ModuleBayType>, AppError> {
        validate(
            item.name.as_deref(),
            item.slug.as_deref(),
            item.color.as_deref(),
            item.description.as_deref(),
            item.custom_fields.as_ref(),
        )?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let row = sqlx::query("UPDATE dcim_modulebaytype SET name=COALESCE($2,name),slug=COALESCE($3,slug),manufacturer_id=CASE WHEN $4 THEN $5 ELSE manufacturer_id END,color=COALESCE($6,color),description=COALESCE($7,description),comments=COALESCE($8,comments),owner_id=CASE WHEN $9 THEN $10 ELSE owner_id END,tags=COALESCE($11,tags),custom_field_data=custom_field_data || COALESCE($12,'{}'::jsonb),last_updated=NOW() WHERE id=$1 RETURNING *")
            .bind(id).bind(&item.name).bind(&item.slug).bind(item.manufacturer_id.is_some()).bind(item.manufacturer_id.flatten())
            .bind(&item.color).bind(&item.description).bind(&item.comments).bind(item.owner_id.is_some()).bind(item.owner_id.flatten())
            .bind(&item.tags).bind(&item.custom_fields).fetch_optional(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        self.secret_intents
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = Self::parse(&row)?;
        tx.commit().await?;
        Ok(Some(result))
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let count = super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_modulebaytype",
            id,
        )
        .await?;
        if count == 0 {
            return Err(AppError::NotFound("Module bay type not found".into()));
        }
        Ok(())
    }
}

fn validate(
    name: Option<&str>,
    slug: Option<&str>,
    color: Option<&str>,
    description: Option<&str>,
    custom_fields: Option<&serde_json::Value>,
) -> Result<(), AppError> {
    for (field, value) in [("name", name), ("slug", slug)] {
        if value.is_some_and(|v| v.trim().is_empty() || v.chars().count() > 100) {
            return Err(AppError::Validation(format!(
                "{field} must contain 1 to 100 characters"
            )));
        }
    }
    if slug.is_some_and(|v| {
        !v.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    }) {
        return Err(AppError::Validation(
            "slug must contain only ASCII letters, digits, hyphens or underscores".into(),
        ));
    }
    if color.is_some_and(|v| {
        !v.is_empty() && (v.len() != 6 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
    }) {
        return Err(AppError::Validation(
            "color must be blank or six hexadecimal digits".into(),
        ));
    }
    if description.is_some_and(|v| v.chars().count() > 200) {
        return Err(AppError::Validation(
            "description must be at most 200 characters".into(),
        ));
    }
    if custom_fields.is_some_and(|v| !v.is_object()) {
        return Err(AppError::Validation(
            "custom_fields must be an object".into(),
        ));
    }
    Ok(())
}
