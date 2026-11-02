use std::{future::Future, sync::Arc};

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use crate::contexts::access_control::domain::Action;
use crate::contexts::inventory::domain::vpn::{
    l2vpn::{L2Vpn, is_point_to_point},
    l2vpn_termination::{L2VpnTermination, validate_endpoint},
};
use crate::contexts::inventory::domain::{content_type, object_ref::ObjectRefInput};
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

use super::super::{
    content_type_lookup::lookup_content_type_id_tx, object_ref_resolver::SqlxObjectRefResolver,
};

const VPN_SELECT: &str = "SELECT v.id,v.name,v.slug,v.type,v.status,v.identifier,v.tenant_id,t.name AS tenant_name,v.owner_id,o.name AS owner_name,v.description,v.comments,v.tags,v.custom_field_data AS custom_fields,v.created,v.last_updated,ARRAY(SELECT routetarget_id FROM vpn_l2vpn_import_targets WHERE l2vpn_id=v.id ORDER BY routetarget_id) AS import_targets,ARRAY(SELECT routetarget_id FROM vpn_l2vpn_export_targets WHERE l2vpn_id=v.id ORDER BY routetarget_id) AS export_targets FROM vpn_l2vpn v LEFT JOIN tenancy_tenant t ON t.id=v.tenant_id LEFT JOIN users_owner o ON o.id=v.owner_id";
const TERM_FROM: &str = " FROM vpn_l2vpntermination r JOIN vpn_l2vpn v ON v.id=r.l2vpn_id";
const TERM_SELECT: &str = "SELECT r.id,r.l2vpn_id,v.name AS l2vpn_name,r.assigned_object_type_id,r.assigned_object_id,r.tags,r.custom_field_data AS custom_fields,r.created,r.last_updated FROM vpn_l2vpntermination r JOIN vpn_l2vpn v ON v.id=r.l2vpn_id";

pub const L2VPN_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "v.name", "inventory.resources.fields.name"),
    ListColumn::sortable("slug", "v.slug", "inventory.resources.fields.slug"),
    ListColumn::sortable("type", "v.type", "inventory.resources.fields.type"),
    ListColumn::sortable("status", "v.status", "inventory.resources.fields.status"),
    ListColumn::sortable(
        "identifier",
        "v.identifier",
        "inventory.resources.fields.identifier",
    ),
    ListColumn::sortable(
        "description",
        "v.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable("created", "v.created", "inventory.resources.fields.created"),
    ListColumn::sortable(
        "last_updated",
        "v.last_updated",
        "inventory.resources.fields.last_updated",
    ),
];
pub const L2VPN_TERMINATION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("l2vpn_id", "r.l2vpn_id", "inventory.resources.fields.l2vpn"),
    ListColumn::sortable("l2vpn_name", "v.name", "inventory.resources.fields.l2vpn"),
    ListColumn::sortable("created", "r.created", "inventory.resources.fields.created"),
    ListColumn::sortable(
        "last_updated",
        "r.last_updated",
        "inventory.resources.fields.last_updated",
    ),
];

#[derive(Debug, Default)]
pub struct L2VpnFilter {
    pub search: Option<String>,
    pub name: Vec<String>,
    pub vpn_type: Vec<String>,
    pub status: Vec<String>,
    pub tenant_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    pub identifier: Option<i64>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Default)]
pub struct L2VpnTerminationFilter {
    pub search: Option<String>,
    pub l2vpn_id: Vec<Uuid>,
    pub assigned_object_type: Vec<String>,
    pub assigned_object_id: Vec<Uuid>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Default)]
pub struct L2VpnChanges {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub vpn_type: Option<String>,
    pub status: Option<String>,
    pub identifier: Option<Option<i64>>,
    pub tenant_id: Option<Option<Uuid>>,
    pub owner_id: Option<Option<Uuid>>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub import_targets: Option<Vec<Uuid>>,
    pub export_targets: Option<Vec<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

impl L2VpnChanges {
    fn apply(&self, item: &mut L2Vpn) {
        if let Some(value) = &self.name {
            item.name = value.clone();
        }
        if let Some(value) = &self.slug {
            item.slug = value.clone();
        }
        if let Some(value) = &self.vpn_type {
            item.vpn_type = value.clone();
        }
        if let Some(value) = &self.status {
            item.status = value.clone();
        }
        if let Some(value) = self.identifier {
            item.identifier = value;
        }
        if let Some(value) = self.tenant_id {
            item.tenant_id = value;
        }
        if let Some(value) = self.owner_id {
            item.owner_id = value;
        }
        if let Some(value) = &self.description {
            item.description = value.clone();
        }
        if let Some(value) = &self.comments {
            item.comments = value.clone();
        }
        if let Some(value) = &self.import_targets {
            item.import_targets = value.clone();
        }
        if let Some(value) = &self.export_targets {
            item.export_targets = value.clone();
        }
        if let Some(value) = &self.tags {
            item.tags = value.clone();
        }
        if let Some(value) = &self.custom_fields {
            if let (Some(existing), Some(patch)) =
                (item.custom_fields.as_object_mut(), value.as_object())
            {
                existing.extend(patch.clone());
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct L2VpnTerminationChanges {
    pub l2vpn_id: Option<Uuid>,
    pub assigned_object: Option<ObjectRefInput>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct NewL2VpnTermination {
    pub id: Uuid,
    pub l2vpn_id: Uuid,
    pub assigned_object: ObjectRefInput,
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
}

#[derive(sqlx::FromRow)]
struct TerminationRow {
    id: Uuid,
    l2vpn_id: Uuid,
    l2vpn_name: String,
    assigned_object_type_id: Uuid,
    assigned_object_id: Uuid,
    tags: Vec<String>,
    custom_fields: serde_json::Value,
    created: Option<DateTime<Utc>>,
    last_updated: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct SqlxL2VpnRepository {
    pool: PgPool,
}
#[derive(Clone)]
pub struct SqlxL2VpnTerminationRepository {
    pool: PgPool,
}

fn vpn_filters(query: &mut QueryBuilder<'_, Postgres>, filter: &L2VpnFilter) {
    let mut state = WhereState::new();
    push_search_ilike(
        query,
        &mut state,
        &[
            "v.name",
            "v.slug",
            "v.description",
            "v.comments",
            "v.identifier::text",
        ],
        filter.search.as_deref(),
    );
    push_string_in(query, &mut state, "v.name", &filter.name);
    push_string_in(query, &mut state, "v.type", &filter.vpn_type);
    push_string_in(query, &mut state, "v.status", &filter.status);
    push_uuid_in(query, &mut state, "v.tenant_id", &filter.tenant_id);
    push_uuid_in(query, &mut state, "v.owner_id", &filter.owner_id);
    if let Some(value) = filter.identifier {
        state.and(query);
        query.push("v.identifier = ").push_bind(value);
    }
}

fn termination_filters(query: &mut QueryBuilder<'_, Postgres>, filter: &L2VpnTerminationFilter) {
    let mut state = WhereState::new();
    push_search_ilike(
        query,
        &mut state,
        &["v.name", "r.assigned_object_id::text"],
        filter.search.as_deref(),
    );
    push_uuid_in(query, &mut state, "r.l2vpn_id", &filter.l2vpn_id);
    push_uuid_in(
        query,
        &mut state,
        "r.assigned_object_id",
        &filter.assigned_object_id,
    );
    if !filter.assigned_object_type.is_empty() {
        state.and(query);
        query.push("r.assigned_object_type_id IN (SELECT id FROM inventory_content_type WHERE ");
        query
            .push("app_label || '.' || model = ANY(")
            .push_bind(filter.assigned_object_type.clone())
            .push("::text[]))");
    }
}

/// Registry-derived persisted attributes, read under the graph transaction.
/// The callback owns policy evaluation and therefore also PAT action ceilings.
async fn authorize_reference<F, Fut>(
    tx: &mut Transaction<'_, Postgres>,
    token: &str,
    id: Uuid,
    action: Action,
    authorize: &mut F,
) -> Result<serde_json::Value, AppError>
where
    F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    let def = content_type::by_token(token)
        .ok_or_else(|| AppError::Validation(format!("unregistered reference type: {token}")))?;
    let mut attributes: serde_json::Value = sqlx::query_scalar(&format!(
        "SELECT to_jsonb(t) FROM {} t WHERE id=$1",
        def.table
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::Validation(format!("{token} reference does not exist")))?;
    attributes["object_type"] = serde_json::json!(token);
    authorize(action, id, attributes.clone()).await?;
    Ok(attributes)
}

async fn vpn_references<F, Fut>(
    tx: &mut Transaction<'_, Postgres>,
    owner: Option<Uuid>,
    tenant: Option<Uuid>,
    targets: &[Uuid],
    authorize: &mut F,
) -> Result<(), AppError>
where
    F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    if let Some(id) = owner {
        authorize_reference(tx, "users.owner", id, Action::Read, authorize).await?;
    }
    if let Some(id) = tenant {
        authorize_reference(tx, "tenancy.tenant", id, Action::Read, authorize).await?;
    }
    let mut targets = targets.to_vec();
    targets.sort_unstable();
    targets.dedup();
    for id in targets {
        // RouteTarget is a concrete M2M FK, not a registered GFK endpoint.
        let mut attributes: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(rt) FROM ipam_routetarget rt WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?
                .ok_or_else(|| {
                    AppError::Validation("route target reference does not exist".into())
                })?;
        attributes["object_type"] = serde_json::json!("ipam.routetarget");
        authorize(Action::Read, id, attributes).await?;
    }
    Ok(())
}

async fn check_cap(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    vpn_type: &str,
    excluding: Option<Uuid>,
    adding: bool,
) -> Result<(), AppError> {
    if is_point_to_point(vpn_type) {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM vpn_l2vpntermination WHERE l2vpn_id=$1 AND ($2::uuid IS NULL OR id<>$2)")
            .bind(id).bind(excluding).fetch_one(&mut **tx).await.map_err(classify_sqlx_error_owned)?;
        if count + i64::from(adding) > 2 {
            return Err(AppError::Validation(
                "point-to-point L2VPNs cannot have more than two terminations".into(),
            ));
        }
    }
    Ok(())
}

async fn replace_targets(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    import: bool,
    targets: &[Uuid],
) -> Result<(), AppError> {
    let table = if import {
        "vpn_l2vpn_import_targets"
    } else {
        "vpn_l2vpn_export_targets"
    };
    sqlx::query(&format!("DELETE FROM {table} WHERE l2vpn_id=$1"))
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    sqlx::query(&format!("INSERT INTO {table} (l2vpn_id,routetarget_id) SELECT $1, target FROM (SELECT DISTINCT unnest($2::uuid[]) AS target) targets"))
        .bind(id).bind(targets).execute(&mut **tx).await.map_err(classify_sqlx_error_owned)?;
    Ok(())
}

async fn apply_secrets(
    pool: &PgPool,
    tx: &mut Transaction<'_, Postgres>,
    inputs: Vec<ApplySecretFieldIntentInput>,
) -> Result<(), AppError> {
    SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(pool.clone())))
        .apply_intents_in_transaction(tx, inputs)
        .await?;
    Ok(())
}

impl SqlxL2VpnRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<L2Vpn>, AppError> {
        sqlx::query_as(&format!("{VPN_SELECT} WHERE v.id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: L2VpnFilter,
    ) -> Result<(Vec<L2Vpn>, i64), AppError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM vpn_l2vpn v");
        vpn_filters(&mut count, &filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(VPN_SELECT);
        vpn_filters(&mut query, &filter);
        push_order_by(
            &mut query,
            L2VPN_COLUMNS,
            filter.sort.as_ref(),
            "v.name ASC",
        );
        query
            .push(", v.id ASC LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let items = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok((items, total))
    }

    pub async fn create_with_secret_intents<F, Fut>(
        &self,
        item: &L2Vpn,
        inputs: Vec<ApplySecretFieldIntentInput>,
        mut authorize: F,
    ) -> Result<L2Vpn, AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        item.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let targets = [
            item.import_targets.as_slice(),
            item.export_targets.as_slice(),
        ]
        .concat();
        vpn_references(
            &mut tx,
            item.owner_id,
            item.tenant_id,
            &targets,
            &mut authorize,
        )
        .await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("INSERT INTO vpn_l2vpn (id,name,slug,type,status,identifier,tenant_id,owner_id,description,comments,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,NOW(),NOW())")
            .bind(item.id).bind(&item.name).bind(&item.slug).bind(&item.vpn_type).bind(&item.status).bind(item.identifier).bind(item.tenant_id).bind(item.owner_id).bind(&item.description).bind(&item.comments).bind(&item.tags).bind(&item.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        replace_targets(&mut tx, item.id, true, &item.import_targets).await?;
        replace_targets(&mut tx, item.id, false, &item.export_targets).await?;
        apply_secrets(&self.pool, &mut tx, inputs).await?;
        let result = sqlx::query_as(&format!("{VPN_SELECT} WHERE v.id=$1"))
            .bind(item.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(result)
    }

    pub async fn update_with_secret_intents<F, Fut>(
        &self,
        id: Uuid,
        changes: &L2VpnChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
        mut authorize: F,
    ) -> Result<Option<L2Vpn>, AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        if changes
            .custom_fields
            .as_ref()
            .is_some_and(|v| !v.is_object())
        {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let Some(mut item) = sqlx::query_as::<_, L2Vpn>(&format!("{VPN_SELECT} WHERE v.id=$1"))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        authorize_reference(&mut tx, "vpn.l2vpn", id, Action::Update, &mut authorize).await?;
        let targets = [
            changes.import_targets.as_deref().unwrap_or_default(),
            changes.export_targets.as_deref().unwrap_or_default(),
        ]
        .concat();
        vpn_references(
            &mut tx,
            changes.owner_id.flatten(),
            changes.tenant_id.flatten(),
            &targets,
            &mut authorize,
        )
        .await?;
        changes.apply(&mut item);
        item.validate()?;
        check_cap(&mut tx, id, &item.vpn_type, None, false).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("UPDATE vpn_l2vpn SET name=$2,slug=$3,type=$4,status=$5,identifier=$6,tenant_id=$7,owner_id=$8,description=$9,comments=$10,tags=$11,custom_field_data=$12,last_updated=NOW() WHERE id=$1")
            .bind(id).bind(&item.name).bind(&item.slug).bind(&item.vpn_type).bind(&item.status).bind(item.identifier).bind(item.tenant_id).bind(item.owner_id).bind(&item.description).bind(&item.comments).bind(&item.tags).bind(&item.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if let Some(targets) = &changes.import_targets {
            replace_targets(&mut tx, id, true, targets).await?;
        }
        if let Some(targets) = &changes.export_targets {
            replace_targets(&mut tx, id, false, targets).await?;
        }
        apply_secrets(&self.pool, &mut tx, inputs).await?;
        let result = sqlx::query_as(&format!("{VPN_SELECT} WHERE v.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(result))
    }

    pub async fn delete<F, Fut>(&self, id: Uuid, mut authorize: F) -> Result<(), AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let mut tx = self.pool.begin().await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vpn_l2vpn WHERE id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        if !exists {
            return Err(AppError::NotFound("L2VPN not found".into()));
        }
        authorize_reference(&mut tx, "vpn.l2vpn", id, Action::Delete, &mut authorize).await?;
        super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "vpn_l2vpn",
            id,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(())
    }
}

impl SqlxL2VpnTerminationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn hydrate(&self, rows: Vec<TerminationRow>) -> Result<Vec<L2VpnTermination>, AppError> {
        let pairs: Vec<_> = rows
            .iter()
            .map(|r| (r.assigned_object_type_id, r.assigned_object_id))
            .collect();
        let refs = SqlxObjectRefResolver::new(self.pool.clone())
            .resolve(&pairs)
            .await?;
        rows.into_iter()
            .map(|row| {
                let assigned_object = refs
                    .get(&(row.assigned_object_type_id, row.assigned_object_id))
                    .cloned()
                    .ok_or_else(|| {
                        AppError::Database(
                            "L2VPN termination has an unregistered endpoint type".into(),
                        )
                    })?;
                Ok(L2VpnTermination {
                    id: row.id,
                    l2vpn_id: row.l2vpn_id,
                    l2vpn_name: row.l2vpn_name,
                    assigned_object,
                    tags: row.tags,
                    custom_fields: row.custom_fields,
                    created: row.created,
                    last_updated: row.last_updated,
                })
            })
            .collect()
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<L2VpnTermination>, AppError> {
        let rows = sqlx::query_as(&format!("{TERM_SELECT} WHERE r.id=$1"))
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(self.hydrate(rows).await?.pop())
    }

    pub async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: L2VpnTerminationFilter,
    ) -> Result<(Vec<L2VpnTermination>, i64), AppError> {
        if filter.assigned_object_type.iter().any(|token| {
            !crate::contexts::inventory::domain::vpn::l2vpn_termination::ENDPOINT_TYPES
                .contains(&token.as_str())
        }) {
            return Err(AppError::Validation(
                "unsupported assigned_object_type filter".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new(format!("SELECT COUNT(*){TERM_FROM}"));
        termination_filters(&mut count, &filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(TERM_SELECT);
        termination_filters(&mut query, &filter);
        push_order_by(
            &mut query,
            L2VPN_TERMINATION_COLUMNS,
            filter.sort.as_ref(),
            "v.name ASC",
        );
        query
            .push(", r.id ASC LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok((self.hydrate(rows).await?, total))
    }

    pub async fn create_with_secret_intents<F, Fut>(
        &self,
        item: &NewL2VpnTermination,
        inputs: Vec<ApplySecretFieldIntentInput>,
        mut authorize: F,
    ) -> Result<L2VpnTermination, AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let NewL2VpnTermination {
            id,
            l2vpn_id,
            assigned_object: endpoint,
            tags,
            custom_fields,
        } = item;
        let (id, l2vpn_id) = (*id, *l2vpn_id);
        validate_endpoint(endpoint)?;
        if !custom_fields.is_object() {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let parent =
            authorize_reference(&mut tx, "vpn.l2vpn", l2vpn_id, Action::Read, &mut authorize)
                .await?;
        check_cap(
            &mut tx,
            l2vpn_id,
            parent["type"]
                .as_str()
                .ok_or_else(|| AppError::Database("L2VPN type is missing".into()))?,
            None,
            true,
        )
        .await?;
        let type_id = endpoint_reference(&mut tx, endpoint, &mut authorize).await?;
        check_endpoint_unique(&mut tx, type_id, endpoint.object_id, id).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("INSERT INTO vpn_l2vpntermination (id,l2vpn_id,assigned_object_type_id,assigned_object_id,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,NOW(),NOW())")
            .bind(id).bind(l2vpn_id).bind(type_id).bind(endpoint.object_id).bind(tags).bind(custom_fields).execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        apply_secrets(&self.pool, &mut tx, inputs).await?;
        let row = sqlx::query_as(&format!("{TERM_SELECT} WHERE r.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.hydrate(vec![row])
            .await?
            .pop()
            .ok_or_else(|| AppError::Database("created termination missing".into()))
    }

    pub async fn update_with_secret_intents<F, Fut>(
        &self,
        id: Uuid,
        changes: &L2VpnTerminationChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
        mut authorize: F,
    ) -> Result<Option<L2VpnTermination>, AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        if let Some(endpoint) = &changes.assigned_object {
            validate_endpoint(endpoint)?;
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
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let Some(mut row) =
            sqlx::query_as::<_, TerminationRow>(&format!("{TERM_SELECT} WHERE r.id=$1"))
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?
        else {
            return Ok(None);
        };
        authorize_reference(
            &mut tx,
            "vpn.l2vpntermination",
            id,
            Action::Update,
            &mut authorize,
        )
        .await?;
        if let Some(parent_id) = changes.l2vpn_id {
            let parent = authorize_reference(
                &mut tx,
                "vpn.l2vpn",
                parent_id,
                Action::Read,
                &mut authorize,
            )
            .await?;
            check_cap(
                &mut tx,
                parent_id,
                parent["type"]
                    .as_str()
                    .ok_or_else(|| AppError::Database("L2VPN type is missing".into()))?,
                Some(id),
                true,
            )
            .await?;
            row.l2vpn_id = parent_id;
        }
        if let Some(endpoint) = &changes.assigned_object {
            row.assigned_object_type_id =
                endpoint_reference(&mut tx, endpoint, &mut authorize).await?;
            row.assigned_object_id = endpoint.object_id;
            check_endpoint_unique(
                &mut tx,
                row.assigned_object_type_id,
                row.assigned_object_id,
                id,
            )
            .await?;
        }
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("UPDATE vpn_l2vpntermination SET l2vpn_id=$2,assigned_object_type_id=$3,assigned_object_id=$4,tags=COALESCE($5,tags),custom_field_data=custom_field_data || COALESCE($6,'{}'::jsonb),last_updated=NOW() WHERE id=$1")
            .bind(id).bind(row.l2vpn_id).bind(row.assigned_object_type_id).bind(row.assigned_object_id).bind(&changes.tags).bind(&changes.custom_fields).execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        apply_secrets(&self.pool, &mut tx, inputs).await?;
        let row = sqlx::query_as(&format!("{TERM_SELECT} WHERE r.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(self.hydrate(vec![row]).await?.pop())
    }

    pub async fn delete<F, Fut>(&self, id: Uuid, mut authorize: F) -> Result<(), AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let mut tx = self.pool.begin().await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vpn_l2vpntermination WHERE id=$1)")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(AppError::NotFound("L2VPN termination not found".into()));
        }
        authorize_reference(
            &mut tx,
            "vpn.l2vpntermination",
            id,
            Action::Delete,
            &mut authorize,
        )
        .await?;
        super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "vpn_l2vpntermination",
            id,
        )
        .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(())
    }
}

async fn endpoint_reference<F, Fut>(
    tx: &mut Transaction<'_, Postgres>,
    endpoint: &ObjectRefInput,
    authorize: &mut F,
) -> Result<Uuid, AppError>
where
    F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    validate_endpoint(endpoint)?;
    authorize_reference(
        tx,
        &endpoint.object_type,
        endpoint.object_id,
        Action::Read,
        authorize,
    )
    .await?;
    let def = content_type::by_token(&endpoint.object_type)
        .ok_or_else(|| AppError::Validation("unregistered endpoint type".into()))?;
    lookup_content_type_id_tx(tx, def.app_label, def.model)
        .await?
        .ok_or_else(|| AppError::Validation("endpoint content type is not registered".into()))
}

async fn check_endpoint_unique(
    tx: &mut Transaction<'_, Postgres>,
    type_id: Uuid,
    object_id: Uuid,
    excluding: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vpn_l2vpntermination WHERE assigned_object_type_id=$1 AND assigned_object_id=$2 AND id<>$3)")
        .bind(type_id).bind(object_id).bind(excluding).fetch_one(&mut **tx).await.map_err(classify_sqlx_error_owned)?;
    if exists {
        return Err(AppError::Conflict(
            "endpoint is already assigned to an L2VPN termination".into(),
        ));
    }
    Ok(())
}
