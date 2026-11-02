use std::{future::Future, sync::Arc};

use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::access_control::domain::Action;
use crate::contexts::inventory::domain::circuits::virtual_circuit_termination::{
    VirtualCircuitTermination, validate_role,
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

const FROM: &str = " FROM circuits_virtualcircuittermination t JOIN circuits_virtualcircuit vc ON vc.id=t.virtual_circuit_id JOIN dcim_interface i ON i.id=t.interface_id";
const SELECT: &str = "SELECT t.id,t.virtual_circuit_id,vc.cid AS virtual_circuit_name,t.interface_id,i.name AS interface_name,t.role,t.description,t.tags,t.custom_field_data AS custom_fields,t.created,t.last_updated";

pub const VIRTUAL_CIRCUIT_TERMINATION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "virtual_circuit_id",
        "t.virtual_circuit_id",
        "inventory.resources.fields.virtualCircuit",
    ),
    ListColumn::sortable(
        "interface_id",
        "t.interface_id",
        "inventory.resources.fields.interface",
    ),
    ListColumn::sortable("role", "t.role", "inventory.resources.fields.role"),
    ListColumn::sortable(
        "description",
        "t.description",
        "inventory.resources.fields.description",
    ),
    ListColumn::sortable("created", "t.created", "inventory.resources.fields.created"),
    ListColumn::sortable(
        "last_updated",
        "t.last_updated",
        "inventory.resources.fields.last_updated",
    ),
];

#[derive(Debug, Default)]
pub struct VirtualCircuitTerminationFilter {
    pub search: Option<String>,
    pub virtual_circuit_id: Vec<Uuid>,
    pub interface_id: Vec<Uuid>,
    pub role: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Default)]
pub struct VirtualCircuitTerminationChanges {
    pub virtual_circuit_id: Option<Uuid>,
    pub interface_id: Option<Uuid>,
    pub role: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

impl VirtualCircuitTerminationChanges {
    pub fn validate(&self) -> Result<(), AppError> {
        VirtualCircuitTermination {
            role: self.role.clone().unwrap_or_else(|| "peer".into()),
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

fn filters(query: &mut QueryBuilder<'_, Postgres>, filter: &VirtualCircuitTerminationFilter) {
    let mut state = WhereState::new();
    push_search_ilike(
        query,
        &mut state,
        &["vc.cid", "i.name", "t.role", "t.description"],
        filter.search.as_deref(),
    );
    push_uuid_in(
        query,
        &mut state,
        "t.virtual_circuit_id",
        &filter.virtual_circuit_id,
    );
    push_uuid_in(query, &mut state, "t.interface_id", &filter.interface_id);
    push_string_in(query, &mut state, "t.role", &filter.role);
}

#[derive(Clone)]
pub struct SqlxVirtualCircuitTerminationRepository {
    pool: PgPool,
}

impl SqlxVirtualCircuitTerminationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read and authorize only submitted references after acquiring the graph lock.
    async fn authorize_references<F, Fut>(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        parent: Option<Uuid>,
        interface: Option<Uuid>,
        authorize: &mut F,
    ) -> Result<(), AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        for (id, object_type, query) in [
            (
                parent,
                "circuits.virtualcircuit",
                "SELECT to_jsonb(vc) FROM circuits_virtualcircuit vc WHERE vc.id=$1",
            ),
            (
                interface,
                "dcim.interface",
                "SELECT to_jsonb(i) FROM dcim_interface i WHERE i.id=$1",
            ),
        ] {
            let Some(id) = id else { continue };
            let mut attributes: serde_json::Value = sqlx::query_scalar(query)
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?
                .ok_or_else(|| {
                    AppError::Validation(format!("{object_type} reference does not exist"))
                })?;
            attributes["object_type"] = serde_json::json!(object_type);
            authorize(Action::Read, id, attributes.clone()).await?;
            if object_type == "dcim.interface"
                && !matches!(
                    attributes["type"].as_str(),
                    Some("virtual" | "lag" | "bridge")
                )
            {
                return Err(AppError::Validation(
                    "interface_id must reference a DCIM virtual, LAG or bridge interface".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<VirtualCircuitTermination>, AppError> {
        sqlx::query_as(&format!("{SELECT}{FROM} WHERE t.id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        filter: VirtualCircuitTerminationFilter,
    ) -> Result<(Vec<VirtualCircuitTermination>, i64), AppError> {
        for role in &filter.role {
            validate_role(role)?;
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new(format!("SELECT COUNT(*){FROM}"));
        filters(&mut count, &filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::new(format!("{SELECT}{FROM}"));
        filters(&mut query, &filter);
        push_order_by(
            &mut query,
            VIRTUAL_CIRCUIT_TERMINATION_COLUMNS,
            filter.sort.as_ref(),
            "t.virtual_circuit_id ASC, t.role ASC",
        );
        query
            .push(", t.id ASC LIMIT ")
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
        item: &VirtualCircuitTermination,
        inputs: Vec<ApplySecretFieldIntentInput>,
        mut authorize: F,
    ) -> Result<VirtualCircuitTermination, AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        item.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        Self::authorize_references(
            &mut tx,
            Some(item.virtual_circuit_id),
            Some(item.interface_id),
            &mut authorize,
        )
        .await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        sqlx::query("INSERT INTO circuits_virtualcircuittermination (id,virtual_circuit_id,interface_id,role,description,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),NOW())")
            .bind(item.id).bind(item.virtual_circuit_id).bind(item.interface_id).bind(&item.role).bind(&item.description).bind(&item.tags).bind(&item.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = sqlx::query_as(&format!("{SELECT}{FROM} WHERE t.id=$1"))
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
        changes: &VirtualCircuitTerminationChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
        mut authorize: F,
    ) -> Result<Option<VirtualCircuitTermination>, AppError>
    where
        F: FnMut(Action, Uuid, serde_json::Value) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        changes.validate()?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        Self::authorize_references(
            &mut tx,
            changes.virtual_circuit_id,
            changes.interface_id,
            &mut authorize,
        )
        .await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let changed = sqlx::query("UPDATE circuits_virtualcircuittermination SET virtual_circuit_id=COALESCE($2,virtual_circuit_id), interface_id=COALESCE($3,interface_id), role=COALESCE($4,role), description=COALESCE($5,description), tags=COALESCE($6,tags), custom_field_data=custom_field_data || COALESCE($7,'{}'::jsonb), last_updated=NOW() WHERE id=$1")
            .bind(id).bind(changes.virtual_circuit_id).bind(changes.interface_id).bind(&changes.role).bind(&changes.description).bind(&changes.tags).bind(&changes.custom_fields)
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        if changed.rows_affected() == 0 {
            return Ok(None);
        }
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        let result = sqlx::query_as(&format!("{SELECT}{FROM} WHERE t.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(result))
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        super::super::dcim::hardware_graph::lock_hardware_graph(&mut tx).await?;
        if super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut tx,
            "circuits_virtualcircuittermination",
            id,
        )
        .await?
            == 0
        {
            return Err(AppError::NotFound(
                "Virtual circuit termination not found".into(),
            ));
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(())
    }
}
