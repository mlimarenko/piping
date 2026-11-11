use std::sync::Arc;

use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::inventory::domain::ipam::{
    vlan_translation_policy::VlanTranslationPolicy, vlan_translation_rule::VlanTranslationRule,
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

macro_rules! policy_select { () => { "SELECT p.id,p.name,p.description,p.comments,p.owner_id,o.name AS owner_name,p.tags,p.custom_field_data AS custom_fields,p.created,p.last_updated FROM ipam_vlantranslationpolicy p LEFT JOIN users_owner o ON o.id=p.owner_id" }; }
macro_rules! rule_select { () => { "SELECT r.id,r.policy_id,p.name AS policy_name,r.local_vid,r.remote_vid,r.description,r.tags,r.custom_field_data AS custom_fields,r.created,r.last_updated FROM ipam_vlantranslationrule r JOIN ipam_vlantranslationpolicy p ON p.id=r.policy_id" }; }

/// Exact supported policy sort keys; no provider timestamps or synthetic aliases.
pub const VLAN_TRANSLATION_POLICY_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "p.name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "description",
        "p.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable("created", "p.created", "inventory.resources.fields.created"),
    ListColumn::sortable(
        "last_updated",
        "p.last_updated",
        "inventory.resources.fields.last_updated",
    ),
];
pub const VLAN_TRANSLATION_RULE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "policy_id",
        "r.policy_id",
        "inventory.resources.fields.policy",
    ),
    ListColumn::sortable(
        "local_vid",
        "r.local_vid",
        "inventory.resources.fields.local_vid",
    ),
    ListColumn::sortable(
        "remote_vid",
        "r.remote_vid",
        "inventory.resources.fields.remote_vid",
    ),
    ListColumn::sortable(
        "description",
        "r.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable("created", "r.created", "inventory.resources.fields.created"),
    ListColumn::sortable(
        "last_updated",
        "r.last_updated",
        "inventory.resources.fields.last_updated",
    ),
];

#[derive(Debug, Default)]
pub struct VlanTranslationPolicyFilter {
    pub search: Option<String>,
    pub name: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Default)]
pub struct VlanTranslationRuleFilter {
    pub search: Option<String>,
    pub policy_id: Vec<Uuid>,
    pub local_vid: Option<i16>,
    pub remote_vid: Option<i16>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Default)]
pub struct VlanTranslationPolicyChanges {
    pub name: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

impl VlanTranslationPolicyChanges {
    pub fn validate(&self) -> Result<(), AppError> {
        VlanTranslationPolicy {
            name: self.name.clone().unwrap_or_else(|| "unchanged".into()),
            description: self.description.clone().unwrap_or_default(),
            custom_fields: self
                .custom_fields
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
            ..Default::default()
        }
        .validate()
    }
}

#[derive(Debug, Default)]
pub struct VlanTranslationRuleChanges {
    pub policy_id: Option<Uuid>,
    pub local_vid: Option<i16>,
    pub remote_vid: Option<i16>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

impl VlanTranslationRuleChanges {
    pub fn validate(&self) -> Result<(), AppError> {
        VlanTranslationRule {
            local_vid: self.local_vid.unwrap_or(1),
            remote_vid: self.remote_vid.unwrap_or(1),
            description: self.description.clone().unwrap_or_default(),
            custom_fields: self
                .custom_fields
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
            ..Default::default()
        }
        .validate()
    }
}

#[derive(Clone)]
pub struct SqlxVlanTranslationPolicyRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct SqlxVlanTranslationRuleRepository {
    pool: PgPool,
}

fn policy_filters(query: &mut QueryBuilder<Postgres>, filter: &VlanTranslationPolicyFilter) {
    let mut state = WhereState::new();
    push_search_ilike(
        query,
        &mut state,
        &["p.name", "p.description", "p.comments"],
        filter.search.as_deref(),
    );
    push_string_in(query, &mut state, "p.name", &filter.name);
}

fn rule_filters(query: &mut QueryBuilder<Postgres>, filter: &VlanTranslationRuleFilter) {
    let mut state = WhereState::new();
    push_search_ilike(
        query,
        &mut state,
        &[
            "r.description",
            "p.name",
            "r.local_vid::text",
            "r.remote_vid::text",
        ],
        filter.search.as_deref(),
    );
    push_uuid_in(query, &mut state, "r.policy_id", &filter.policy_id);
    if let Some(vid) = filter.local_vid {
        state.and(query);
        query.push("r.local_vid = ").push_bind(vid);
    }
    if let Some(vid) = filter.remote_vid {
        state.and(query);
        query.push("r.remote_vid = ").push_bind(vid);
    }
}

impl SqlxVlanTranslationPolicyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<VlanTranslationPolicy>, AppError> {
        sqlx::query_as(concat!(policy_select!(), " WHERE p.id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: VlanTranslationPolicyFilter,
    ) -> Result<(Vec<VlanTranslationPolicy>, i64), AppError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM ipam_vlantranslationpolicy p");
        policy_filters(&mut count, &filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(policy_select!());
        policy_filters(&mut query, &filter);
        push_order_by(
            &mut query,
            VLAN_TRANSLATION_POLICY_COLUMNS,
            filter.sort.as_ref(),
            "p.name ASC",
        );
        query.push(", p.id ASC");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await?;
        Ok((items, total))
    }

    pub async fn create_with_secret_intents(
        &self,
        item: &VlanTranslationPolicy,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<VlanTranslationPolicy, AppError> {
        item.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("INSERT INTO ipam_vlantranslationpolicy (id,name,description,comments,owner_id,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),NOW())")
            .bind(item.id).bind(&item.name).bind(&item.description).bind(&item.comments).bind(item.owner_id).bind(&item.tags).bind(&item.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = sqlx::query_as(concat!(policy_select!(), " WHERE p.id=$1"))
            .bind(item.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn update_with_secret_intents(
        &self,
        id: Uuid,
        changes: &VlanTranslationPolicyChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Option<VlanTranslationPolicy>, AppError> {
        changes.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let changed = sqlx::query("UPDATE ipam_vlantranslationpolicy SET name=COALESCE($2,name), description=COALESCE($3,description), comments=COALESCE($4,comments), owner_id=CASE WHEN $5 THEN $6 ELSE owner_id END, tags=COALESCE($7,tags), custom_field_data=custom_field_data || COALESCE($8,'{}'::jsonb), last_updated=NOW() WHERE id=$1")
            .bind(id).bind(&changes.name).bind(&changes.description).bind(&changes.comments)
            .bind(changes.owner_id.is_some()).bind(changes.owner_id.flatten()).bind(&changes.tags).bind(&changes.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if changed.rows_affected() == 0 {
            return Ok(None);
        }
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = sqlx::query_as(concat!(policy_select!(), " WHERE p.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await?;
        Ok(Some(result))
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        // The canonical guard owns rule cascade secret cleanup and blockers in
        // this same transaction, before the FK's ON DELETE CASCADE executes.
        if super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "ipam_vlantranslationpolicy",
            id,
        )
        .await?
            == 0
        {
            return Err(AppError::NotFound(
                "VLAN translation policy not found".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }
}

impl SqlxVlanTranslationRuleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<VlanTranslationRule>, AppError> {
        sqlx::query_as(concat!(rule_select!(), " WHERE r.id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: VlanTranslationRuleFilter,
    ) -> Result<(Vec<VlanTranslationRule>, i64), AppError> {
        for vid in [filter.local_vid, filter.remote_vid].into_iter().flatten() {
            if !(1..=4094).contains(&vid) {
                return Err(AppError::Validation(
                    "VID filters must be between 1 and 4094".into(),
                ));
            }
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new(
            "SELECT COUNT(*) FROM ipam_vlantranslationrule r JOIN ipam_vlantranslationpolicy p ON p.id=r.policy_id",
        );
        rule_filters(&mut count, &filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(rule_select!());
        rule_filters(&mut query, &filter);
        push_order_by(
            &mut query,
            VLAN_TRANSLATION_RULE_COLUMNS,
            filter.sort.as_ref(),
            "p.name ASC, r.local_vid ASC",
        );
        query.push(", r.id ASC");
        query
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await?;
        Ok((items, total))
    }

    pub async fn create_with_secret_intents(
        &self,
        item: &VlanTranslationRule,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<VlanTranslationRule, AppError> {
        item.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("INSERT INTO ipam_vlantranslationrule (id,policy_id,local_vid,remote_vid,description,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),NOW())")
            .bind(item.id).bind(item.policy_id).bind(item.local_vid).bind(item.remote_vid).bind(&item.description).bind(&item.tags).bind(&item.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = sqlx::query_as(concat!(rule_select!(), " WHERE r.id=$1"))
            .bind(item.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn update_with_secret_intents(
        &self,
        id: Uuid,
        changes: &VlanTranslationRuleChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Option<VlanTranslationRule>, AppError> {
        changes.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let changed = sqlx::query("UPDATE ipam_vlantranslationrule SET policy_id=COALESCE($2,policy_id), local_vid=COALESCE($3,local_vid), remote_vid=COALESCE($4,remote_vid), description=COALESCE($5,description), tags=COALESCE($6,tags), custom_field_data=custom_field_data || COALESCE($7,'{}'::jsonb), last_updated=NOW() WHERE id=$1")
            .bind(id).bind(changes.policy_id).bind(changes.local_vid).bind(changes.remote_vid).bind(&changes.description).bind(&changes.tags).bind(&changes.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if changed.rows_affected() == 0 {
            return Ok(None);
        }
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = sqlx::query_as(concat!(rule_select!(), " WHERE r.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await?;
        Ok(Some(result))
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        if super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "ipam_vlantranslationrule",
            id,
        )
        .await?
            == 0
        {
            return Err(AppError::NotFound("VLAN translation rule not found".into()));
        }
        tx.commit().await?;
        Ok(())
    }
}
