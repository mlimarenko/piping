use crate::contexts::inventory::domain::circuits::circuit_group_assignment::{
    CircuitGroupAssignment, normalize_priority, validate_member_type,
};
use crate::contexts::inventory::domain::{
    content_type,
    object_ref::{ObjectRef, ObjectRefInput},
};
use crate::contexts::inventory::infrastructure::persistence::repositories::content_type_lookup::lookup_content_type_id_tx;
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
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction, postgres::PgRow};
use std::sync::Arc;
use uuid::Uuid;

pub const CIRCUIT_GROUP_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "group_id",
        "g.name",
        "group_id",
        "a.group_id",
        "circuit-groups",
        "inventory.resources.fields.circuitGroup",
    ),
    ListColumn::sortable(
        "member_id",
        "a.member_id",
        "inventory.resources.fields.member",
    ),
    ListColumn::enum_filter(
        "member_type",
        "ct.app_label || '.' || ct.model",
        "ct.app_label || '.' || ct.model",
        "inventory.resources.fields.memberType",
    ),
    ListColumn::enum_filter(
        "priority",
        "a.priority",
        "a.priority",
        "inventory.resources.fields.priority",
    ),
];
const SELECT: &str = "SELECT a.id,a.group_id,g.name AS group_name,ct.app_label || '.' || ct.model AS member_type,a.member_id,COALESCE(c.cid,v.cid) AS member_label,a.priority,a.tags,a.custom_field_data,a.created,a.last_updated,a.external_id,a.piping_synced_at";
const FROM: &str = " FROM circuits_circuitgroupassignment a JOIN circuits_circuitgroup g ON g.id=a.group_id JOIN inventory_content_type ct ON ct.id=a.member_type_id LEFT JOIN circuits_circuit c ON ct.app_label='circuits' AND ct.model='circuit' AND c.id=a.member_id LEFT JOIN circuits_virtualcircuit v ON ct.app_label='circuits' AND ct.model='virtualcircuit' AND v.id=a.member_id";

#[derive(Debug, Default)]
pub struct CircuitGroupAssignmentChanges {
    pub group_id: Option<Uuid>,
    pub member: Option<ObjectRefInput>,
    pub priority: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}
#[derive(Debug, Default)]
pub struct CircuitGroupAssignmentFilter {
    pub group_id: Vec<Uuid>,
    pub member_id: Vec<Uuid>,
    pub member_type: Vec<String>,
    pub priority: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

/// Structural writers and target deletion take this before any entity/catalogue
/// row locks. SQL triggers apply the same lock to non-native structural writers.
pub(crate) async fn lock_graph(tx: &mut Transaction<'_, Postgres>) -> Result<(), AppError> {
    super::super::dcim::hardware_graph::lock_hardware_graph(tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('circuits-group-assignments',0))")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn reject_member_deletion(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    id: Uuid,
) -> Result<(), AppError> {
    use crate::errors::app_error::{
        DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup,
    };
    let model = match table {
        "circuits_circuit" => "circuit",
        "circuits_virtualcircuit" => "virtualcircuit",
        _ => return Ok(()),
    };
    let rows: Vec<(Uuid,String,i64)> = sqlx::query_as("SELECT a.id,g.name,COUNT(*) OVER() FROM circuits_circuitgroupassignment a JOIN circuits_circuitgroup g ON g.id=a.group_id JOIN inventory_content_type ct ON ct.id=a.member_type_id WHERE a.member_id=$1 AND ct.app_label='circuits' AND ct.model=$2 ORDER BY a.id LIMIT 3")
        .bind(id).bind(model).fetch_all(&mut **tx).await?;
    if let Some((_, _, count)) = rows.first() {
        return Err(AppError::DeleteBlocked(DeleteBlockedDetails {
            total_count: *count,
            blockers: vec![DeleteBlockedGroup {
                table: "circuits_circuitgroupassignment".into(),
                entity_key: Some("Circuit Group Assignments".into()),
                count: *count,
                examples: rows
                    .iter()
                    .map(|(id, label, _)| DeleteBlockedExample {
                        id: Some(*id),
                        label: label.clone(),
                    })
                    .collect(),
            }],
        }));
    }
    Ok(())
}

fn parse_row(row: &PgRow) -> Result<CircuitGroupAssignment, AppError> {
    Ok(CircuitGroupAssignment {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        group_name: row.try_get("group_name")?,
        member: ObjectRef {
            object_type: row.try_get("member_type")?,
            object_id: row.try_get("member_id")?,
            label: row.try_get("member_label")?,
        },
        priority: row.try_get("priority")?,
        tags: row.try_get("tags")?,
        custom_fields: row.try_get("custom_field_data")?,
        created: row.try_get("created")?,
        last_updated: row.try_get("last_updated")?,
        external_id: row.try_get("external_id")?,
        piping_synced_at: row.try_get("piping_synced_at")?,
    })
}
fn filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &CircuitGroupAssignmentFilter) {
    let mut state = WhereState::new();
    push_uuid_in(builder, &mut state, "a.group_id", &filter.group_id);
    push_uuid_in(builder, &mut state, "a.member_id", &filter.member_id);
    push_string_in(
        builder,
        &mut state,
        "ct.app_label || '.' || ct.model",
        &filter.member_type,
    );
    push_string_in(builder, &mut state, "a.priority", &filter.priority);
    push_search_ilike(
        builder,
        &mut state,
        &["g.name", "c.cid", "v.cid", "a.priority"],
        filter.search.as_deref(),
    );
}

async fn member_type_id(
    tx: &mut Transaction<'_, Postgres>,
    member: &ObjectRefInput,
) -> Result<Uuid, AppError> {
    validate_member_type(&member.object_type)?;
    let definition = content_type::by_token(&member.object_type)
        .ok_or_else(|| AppError::Validation("Member type is not registered".into()))?;
    let id = lookup_content_type_id_tx(tx, definition.app_label, definition.model)
        .await?
        .ok_or_else(|| AppError::Validation("Member type is not seeded".into()))?;
    // Identifiers are supplied only by the canonical, closed content-type registry.
    let found: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT id FROM {} WHERE id=$1 FOR KEY SHARE",
        definition.table
    ))
    .bind(member.object_id)
    .fetch_optional(&mut **tx)
    .await?;
    if found.is_none() {
        return Err(AppError::Validation(
            "member does not exist for the selected type".into(),
        ));
    }
    Ok(id)
}

#[derive(Clone)]
pub struct SqlxCircuitGroupAssignmentRepository {
    pool: PgPool,
}
impl SqlxCircuitGroupAssignmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<CircuitGroupAssignment>, AppError> {
        let row = sqlx::query(&format!("{SELECT}{FROM} WHERE a.id=$1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.as_ref().map(parse_row).transpose()
    }
    pub async fn list(
        &self,
        filter: CircuitGroupAssignmentFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<CircuitGroupAssignment>, i64), AppError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new(format!("SELECT COUNT(*){FROM}"));
        filters(&mut count, &filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await?;
        let mut query = QueryBuilder::new(format!("{SELECT}{FROM}"));
        filters(&mut query, &filter);
        push_order_by(
            &mut query,
            CIRCUIT_GROUP_ASSIGNMENT_COLUMNS,
            filter.sort.as_ref(),
            "g.name,ct.model,a.member_id",
        );
        query
            .push(",a.id LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query.build().fetch_all(&mut *tx).await?;
        let items = rows.iter().map(parse_row).collect::<Result<Vec<_>, _>>()?;
        tx.commit().await?;
        Ok((items, total))
    }
    pub async fn create(
        &self,
        id: Uuid,
        changes: &CircuitGroupAssignmentChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<(), AppError> {
        let group = changes
            .group_id
            .ok_or_else(|| AppError::Validation("group_id is required".into()))?;
        let member = changes
            .member
            .as_ref()
            .ok_or_else(|| AppError::Validation("member is required".into()))?;
        let priority = normalize_priority(changes.priority.clone().flatten())?;
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        lock_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let member_type = member_type_id(&mut tx, member).await?;
        sqlx::query("INSERT INTO circuits_circuitgroupassignment(id,group_id,member_type_id,member_id,priority,tags,custom_field_data,created,last_updated) VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),NOW())")
            .bind(id).bind(group).bind(member_type).bind(member.object_id).bind(priority).bind(changes.tags.clone().unwrap_or_default()).bind(changes.custom_fields.clone().unwrap_or_else(|| serde_json::json!({})))
            .execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(())
    }
    pub async fn update(
        &self,
        id: Uuid,
        changes: &CircuitGroupAssignmentChanges,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<bool, AppError> {
        let inputs = SecretFieldIntentService::<SqlxSecretRepository>::validate_inputs(inputs)?;
        let priority = changes
            .priority
            .clone()
            .map(normalize_priority)
            .transpose()?;
        let mut tx = super::super::extras::tag_repository::begin_native_write(&self.pool).await?;
        lock_graph(&mut tx).await?;
        lock_field_intent_batch(&mut tx, &inputs).await?;
        let member_type = if let Some(member) = &changes.member {
            Some(member_type_id(&mut tx, member).await?)
        } else {
            None
        };
        let mut query =
            QueryBuilder::new("UPDATE circuits_circuitgroupassignment SET last_updated=NOW()");
        if let Some(group) = changes.group_id {
            query.push(",group_id=").push_bind(group);
        }
        if let Some(member) = &changes.member {
            query
                .push(",member_type_id=")
                .push_bind(member_type)
                .push(",member_id=")
                .push_bind(member.object_id);
        }
        if let Some(priority) = &priority {
            query.push(",priority=").push_bind(priority);
        }
        if let Some(tags) = &changes.tags {
            query.push(",tags=").push_bind(tags);
        }
        if let Some(fields) = &changes.custom_fields {
            query
                .push(",custom_field_data=custom_field_data || ")
                .push_bind(fields);
        }
        query.push(" WHERE id=").push_bind(id);
        if query
            .build()
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
            .rows_affected()
            == 0
        {
            return Ok(false);
        }
        SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(self.pool.clone())))
            .apply_intents_in_transaction(&mut tx, inputs)
            .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(true)
    }
    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        if super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "circuits_circuitgroupassignment",
            id,
        )
        .await?
            == 0
        {
            return Err(AppError::NotFound(
                "Circuit group assignment not found".into(),
            ));
        }
        Ok(())
    }
}
