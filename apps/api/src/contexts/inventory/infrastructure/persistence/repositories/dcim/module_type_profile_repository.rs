use sqlx::{PgPool, QueryBuilder};
use std::sync::Arc;
use uuid::Uuid;

use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::SqlxSecretRepository;
use crate::contexts::secrets::infrastructure::persistence::repositories::secret_repository::lock_field_intent_batch;

use crate::contexts::inventory::domain::dcim::module_type_profile::ModuleTypeProfile;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

#[derive(Debug, Default)]
pub struct ModuleTypeProfileChanges {
    pub name: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub schema: Option<Option<serde_json::Value>>,
    pub owner_id: Option<Option<Uuid>>,
    pub custom_fields: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

pub const MODULE_TYPE_PROFILE_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "owner_id",
        "owner_id",
        "owner_id",
        "owner_id",
        "owners",
        "inventory.resources.fields.owner",
    ),
    ListColumn::sortable("name", "name", "inventory.resources.fields.name"),
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

#[derive(Clone)]
pub struct SqlxModuleTypeProfileRepository {
    pool: PgPool,
}

impl SqlxModuleTypeProfileRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<ModuleTypeProfile>, AppError> {
        sqlx::query_as::<_, ModuleTypeProfile>(
            "SELECT id, name, description, comments, schema, owner_id, custom_field_data AS custom_fields, tags, created, last_updated FROM dcim_moduletypeprofile WHERE id = $1",
        ).bind(id)
        .fetch_optional(&self.pool)
        .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn create_with_secret_intents(
        &self,
        item: &ModuleTypeProfile,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<ModuleTypeProfile, AppError> {
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::extras::tag_repository::lock_tag_catalog_for_usage(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let item = sqlx::query_as::<_, ModuleTypeProfile>("INSERT INTO dcim_moduletypeprofile (id,name,description,comments,schema,owner_id,custom_field_data,tags,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,NOW(),NOW()) RETURNING id,name,description,comments,schema,owner_id,custom_field_data AS custom_fields,tags,created,last_updated")
            .bind(item.id).bind(&item.name).bind(&item.description).bind(&item.comments).bind(&item.schema).bind(item.owner_id).bind(&item.custom_fields)
            .bind(&item.tags)
            .fetch_one(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        tx.commit().await?;
        Ok(item)
    }

    pub async fn update_with_secret_intents(
        &self,
        id: Uuid,
        changes: &ModuleTypeProfileChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Option<ModuleTypeProfile>, AppError> {
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::extras::tag_repository::lock_tag_catalog_for_usage(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let mut query = QueryBuilder::new("UPDATE dcim_moduletypeprofile SET last_updated=NOW()");
        macro_rules! assignment {
            ($field:ident) => {
                if let Some(value) = &changes.$field {
                    query
                        .push(concat!(", ", stringify!($field), " = "))
                        .push_bind(value);
                }
            };
        }
        assignment!(name);
        assignment!(description);
        assignment!(comments);
        assignment!(schema);
        assignment!(owner_id);
        assignment!(tags);
        if let Some(value) = &changes.custom_fields {
            query
                .push(", custom_field_data = custom_field_data || ")
                .push_bind(value);
        }
        query.push(" WHERE id = ").push_bind(id).push(" RETURNING id,name,description,comments,schema,owner_id,custom_field_data AS custom_fields,tags,created,last_updated");
        let item = query
            .build_query_as::<ModuleTypeProfile>()
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        if item.is_none() {
            return Ok(None);
        }
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        tx.commit().await?;
        Ok(item)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        if super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_moduletypeprofile",
            id,
        )
        .await?
            == 0
        {
            return Err(AppError::NotFound("Module type profile not found".into()));
        }
        Ok(())
    }

    pub async fn list_paginated(
        &self,
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort: Option<SortSpec>,
        owner_ids: &[Uuid],
    ) -> Result<(Vec<ModuleTypeProfile>, i64), AppError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM dcim_moduletypeprofile");
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut count,
            &mut where_state,
            &["name", "description", "comments"],
            search.as_deref(),
        );
        push_uuid_in(&mut count, &mut where_state, "owner_id", owner_ids);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AppError::Database("Failed to count module type profiles".into()))?;

        let mut query = QueryBuilder::new(
            "SELECT id, name, description, comments, schema, owner_id, custom_field_data AS custom_fields,tags,created,last_updated FROM dcim_moduletypeprofile",
        );
        let mut where_state = WhereState::new();
        push_search_ilike(
            &mut query,
            &mut where_state,
            &["name", "description", "comments"],
            search.as_deref(),
        );
        push_uuid_in(&mut query, &mut where_state, "owner_id", owner_ids);
        push_order_by(
            &mut query,
            MODULE_TYPE_PROFILE_COLUMNS,
            sort.as_ref(),
            "name",
        );
        query.push(", id");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)));
        query
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_as::<ModuleTypeProfile>()
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| AppError::Database("Failed to list module type profiles".into()))?;
        tx.commit().await?;
        Ok((items, total))
    }
}
