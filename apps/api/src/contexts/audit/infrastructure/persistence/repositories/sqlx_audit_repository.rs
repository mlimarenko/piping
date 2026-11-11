use async_trait::async_trait;
use serde::Serialize;
use sqlx::{Executor, PgPool, Postgres, QueryBuilder, Row, postgres::PgRow};
use std::collections::HashMap;
use uuid::Uuid;

use super::audit_repository::{AUDIT_EVENT_COLUMNS, AuditEventFilter, AuditRepository};
use crate::contexts::audit::domain::{
    AuditEvent,
    safe_projection::{
        SEARCHABLE_IDENTITY_EXPRESSIONS, canonical_context_sql, outcome_sql, searchable_display_sql,
    },
};
use crate::contexts::shared::list_query::{
    WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxAuditRepository {
    pool: PgPool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AuditObjectRef {
    pub kind: String,
    pub id: Option<String>,
    pub label: String,
    pub secondary: Option<String>,
}
impl AuditObjectRef {
    pub(crate) fn new(kind: impl Into<String>, id: Uuid, label: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: Some(id.to_string()),
            label: label.into(),
            secondary: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AuditTargetOption {
    pub target_type: String,
    pub kind: String,
    pub id: String,
    pub label: String,
    pub secondary: Option<String>,
}

// Labels are public identity attributes only. Never select credentials, custom
// field values, session metadata or arbitrary event metadata for presentation.
fn current_actor_label_sql() -> &'static str {
    "CASE WHEN actor_kind IN ('user','api_token','mcp_agent') THEN (SELECT username FROM identity_user WHERE identity_user.id = audit_event.actor_id) WHEN actor_kind = 'service_principal' THEN (SELECT name FROM playbook_service_principal WHERE playbook_service_principal.id = audit_event.actor_id) END"
}

fn current_target_label_sql() -> &'static str {
    "CASE (CASE target_type WHEN 'inventory:dcim.device' THEN 'device' WHEN 'inventory:virtualization.virtualmachine' THEN 'virtual_machine' ELSE target_type END)
       WHEN 'user' THEN (SELECT username FROM identity_user WHERE identity_user.id = audit_event.target_id)
       WHEN 'user_api_token' THEN (SELECT name FROM auth_user_api_token WHERE auth_user_api_token.id = audit_event.target_id)
       WHEN 'device' THEN (SELECT name FROM dcim_device WHERE dcim_device.id = audit_event.target_id)
       WHEN 'virtual_machine' THEN (SELECT name FROM virtualization_virtualmachine WHERE virtualization_virtualmachine.id = audit_event.target_id)
       WHEN 'connection_target' THEN COALESCE((SELECT name FROM dcim_device WHERE dcim_device.id = audit_event.target_id), (SELECT name FROM virtualization_virtualmachine WHERE virtualization_virtualmachine.id = audit_event.target_id))
       WHEN 'connection_session' THEN (SELECT upper(protocol) || ' · ' || COALESCE((SELECT name FROM dcim_device WHERE dcim_device.id = s.target_id), (SELECT name FROM virtualization_virtualmachine WHERE virtualization_virtualmachine.id = s.target_id), s.target_id::text) FROM connections_session s WHERE s.id = audit_event.target_id)
       WHEN 'secret_variable' THEN (SELECT key FROM secrets_variable WHERE secrets_variable.id = audit_event.target_id AND kind = 'named')
       WHEN 'playbook_job_template' THEN (SELECT name FROM playbook_job_template WHERE playbook_job_template.id = audit_event.target_id)
       WHEN 'playbook_target_set' THEN (SELECT name FROM playbook_target_set WHERE playbook_target_set.id = audit_event.target_id)
       WHEN 'service_principal' THEN (SELECT name FROM playbook_service_principal WHERE playbook_service_principal.id = audit_event.target_id)
       WHEN 'tenant' THEN (SELECT name FROM tenancy_tenant WHERE tenancy_tenant.id = audit_event.target_id)
       WHEN 'config_context' THEN (SELECT name FROM config_context WHERE config_context.id = audit_event.target_id)
       END"
}

fn bounded_label_sql(expression: &str) -> String {
    format!(
        "(SELECT label FROM (SELECT ({expression})::text AS label) audit_label WHERE octet_length(label) BETWEEN 1 AND 256 AND label !~ '[[:cntrl:]]')"
    )
}

/// Recorded labels win; current identity attributes supplement records that did
/// not capture a name. The stored event-time context itself is not rewritten.
fn display_label_sql() -> [String; 2] {
    let recorded = searchable_display_sql();
    std::array::from_fn(|index| {
        let current = [current_actor_label_sql(), current_target_label_sql()][index];
        format!(
            "COALESCE({}, {})",
            recorded[index],
            bounded_label_sql(current)
        )
    })
}

impl SqlxAuditRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn event_labels(
        &self,
        events: &[AuditEvent],
    ) -> Result<HashMap<Uuid, (Option<String>, Option<String>)>, AppError> {
        if events.is_empty() {
            return Ok(HashMap::new());
        }
        let [actor, target] = display_label_sql();
        let sql = format!(
            "SELECT id, {actor} AS actor_label, {target} AS target_label FROM audit_event WHERE id = ANY($1)"
        );
        let ids: Vec<_> = events.iter().map(|event| event.id).collect();
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to resolve audit identities: {error}"))
            })?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("id")?,
                    (row.try_get("actor_label")?, row.try_get("target_label")?),
                ))
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(|error| {
                AppError::Database(format!("Failed to read audit identities: {error}"))
            })
    }
    /// Search recorded identities only; never enumerate current resources.
    pub async fn lookup_targets(
        &self,
        target_type: Option<&str>,
        search: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AuditTargetOption>, AppError> {
        let target_label = &display_label_sql()[1];
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT DISTINCT ON (target_type, target_id) target_type, target_id, {target_label} AS label FROM audit_event WHERE target_type IS NOT NULL AND target_id IS NOT NULL"
        ));
        if let Some(target_type) = target_type {
            query
                .push(" AND lower(replace(replace(target_type, '-', '_'), '.', '_')) = ")
                .push_bind(target_type.to_owned());
        }
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            query
                .push(" AND (target_type ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR target_id::text ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR ")
                .push(target_label)
                .push(" ILIKE ")
                .push_bind(pattern)
                .push(")");
        }
        query
            .push(format!(" ORDER BY target_type, target_id, ({target_label}) IS NULL, created_at DESC, id DESC LIMIT "))
            .push_bind(i64::from(limit.clamp(1, 50)));
        let rows = query.build().fetch_all(&self.pool).await.map_err(|error| {
            AppError::Database(format!("Failed to look up audit targets: {error}"))
        })?;
        rows.iter()
            .map(|row| {
                let target_type: String = row.try_get("target_type")?;
                let id: Uuid = row.try_get("target_id")?;
                Ok(AuditTargetOption {
                    kind: target_type.clone(),
                    target_type,
                    id: id.to_string(),
                    label: row
                        .try_get::<Option<String>, _>("label")?
                        .unwrap_or_else(|| id.to_string()),
                    secondary: None,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(|error| AppError::Database(format!("Failed to read audit target: {error}")))
    }
    fn parse_event_row(row: &PgRow) -> Result<AuditEvent, AppError> {
        let read = || -> Result<AuditEvent, sqlx::Error> {
            Ok(AuditEvent {
                id: row.try_get("id")?,
                event_type: row.try_get("event_type")?,
                actor_id: row.try_get("actor_id")?,
                actor_kind: row.try_get("actor_kind")?,
                target_type: row.try_get("target_type")?,
                target_id: row.try_get("target_id")?,
                metadata: row.try_get("metadata")?,
                created_at: row.try_get("created_at")?,
            })
        };
        read().map_err(|error| AppError::Database(format!("Failed to read audit event: {error}")))
    }
    fn push_filter(builder: &mut QueryBuilder<Postgres>, filter: &AuditEventFilter) {
        let mut state = WhereState::new();
        if let Some(query) = &filter.query {
            let pattern = format!("%{query}%");
            state.and(builder);
            builder.push("(");
            for (index, expression) in SEARCHABLE_IDENTITY_EXPRESSIONS.iter().enumerate() {
                if index > 0 {
                    builder.push(" OR ");
                }
                builder
                    .push(*expression)
                    .push(" ILIKE ")
                    .push_bind(pattern.clone());
            }
            for expression in display_label_sql() {
                builder
                    .push(" OR ")
                    .push(expression)
                    .push(" ILIKE ")
                    .push_bind(pattern.clone());
            }
            builder.push(")");
        }
        push_string_in(builder, &mut state, "event_type", &filter.event_type);
        push_string_in(builder, &mut state, "actor_kind", &filter.actor_kind);
        push_uuid_in(builder, &mut state, "actor_id", &filter.actor_id);
        push_string_in(builder, &mut state, "target_type", &filter.target_type);
        push_uuid_in(builder, &mut state, "target_id", &filter.target_id);
        if !filter.outcome.is_empty() {
            state.and(builder);
            builder.push(outcome_sql()).push(" IN (");
            let mut values = builder.separated(", ");
            for value in &filter.outcome {
                values.push_bind(value.clone());
            }
            builder.push(")");
        }
        if !filter.state_domain.is_empty()
            || !filter.state_value.is_empty()
            || !filter.session_id.is_empty()
            || !filter.request_id.is_empty()
            || !filter.run_id.is_empty()
            || !filter.run_group_id.is_empty()
        {
            state.and(builder);
            builder.push(canonical_context_sql());
        }
        for (path, values) in [
            ("{audit_context,state,domain}", &filter.state_domain),
            ("{audit_context,state,value}", &filter.state_value),
        ] {
            if !values.is_empty() {
                state.and(builder);
                builder.push(format!("metadata #>> '{path}' IN ("));
                let mut separated = builder.separated(", ");
                for value in values {
                    separated.push_bind(value.clone());
                }
                builder.push(")");
            }
        }
        for (path, values) in [
            ("{audit_context,correlation,session_id}", &filter.session_id),
            ("{audit_context,correlation,request_id}", &filter.request_id),
            ("{audit_context,correlation,run_id}", &filter.run_id),
            (
                "{audit_context,correlation,run_group_id}",
                &filter.run_group_id,
            ),
        ] {
            if !values.is_empty() {
                state.and(builder);
                builder.push(format!("metadata #>> '{path}' IN ("));
                let mut separated = builder.separated(", ");
                for value in values {
                    separated.push_bind(value.to_string());
                }
                builder.push(")");
            }
        }
        if let Some(from) = filter.created_from {
            state.and(builder);
            builder.push("created_at >= ").push_bind(from);
        }
        if let Some(to) = filter.created_to {
            state.and(builder);
            builder.push("created_at <= ").push_bind(to);
        }
    }
}

pub(crate) fn normalize_audit_type(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace(['-', '.'], "_").to_ascii_lowercase())
}

pub(crate) async fn insert_audit_event<'e, E>(
    executor: E,
    event: &AuditEvent,
) -> Result<(), AppError>
where
    E: Executor<'e, Database = Postgres>,
{
    event.parse_audit_context()?;
    let canonical_event = event.clone().with_context(event.audit_context())?;
    let context = serde_json::to_value(event.audit_context()).map_err(|error| {
        AppError::Internal(format!("Failed to serialize audit context: {error}"))
    })?;
    let [actor, target] = display_label_sql();
    let sql = format!("WITH audit_input AS (SELECT $1::uuid AS id, $2::text AS event_type, $3::uuid AS actor_id, $4::text AS actor_kind, $5::text AS target_type, $6::uuid AS target_id, $7::jsonb AS metadata, $8::timestamptz AS created_at)
        INSERT INTO audit_event (id,event_type,actor_id,actor_kind,target_type,target_id,metadata,created_at)
        SELECT id,event_type,actor_id,actor_kind,target_type,target_id,
        metadata || jsonb_build_object('audit_context', jsonb_set($9::jsonb, '{{display}}', jsonb_build_object('actor', {actor}, 'target', {target}))), created_at FROM audit_input AS audit_event");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(event.id)
        .bind(&event.event_type)
        .bind(event.actor_id)
        .bind(&event.actor_kind)
        .bind(&event.target_type)
        .bind(event.target_id)
        .bind(&canonical_event.metadata)
        .bind(event.created_at)
        .bind(context)
        .execute(executor)
        .await
        .map_err(|error| AppError::Database(format!("Failed to insert audit event: {error}")))?;
    Ok(())
}

#[async_trait]
impl AuditRepository for SqlxAuditRepository {
    async fn insert(&self, event: AuditEvent) -> Result<(), AppError> {
        insert_audit_event(&self.pool, &event).await
    }
    async fn get_event(&self, id: Uuid) -> Result<Option<AuditEvent>, AppError> {
        let row = sqlx::query("SELECT id,event_type,actor_id,actor_kind,target_type,target_id,metadata,created_at FROM audit_event WHERE id=$1").bind(id).fetch_optional(&self.pool).await.map_err(|error| AppError::Database(format!("Failed to get audit event: {error}")))?;
        row.as_ref().map(Self::parse_event_row).transpose()
    }
    async fn list_events(
        &self,
        filter: AuditEventFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<AuditEvent>, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT id,event_type,actor_id,actor_kind,target_type,target_id,metadata,created_at FROM audit_event",
        );
        Self::push_filter(&mut builder, &filter);
        if let Some(sort) = filter
            .sort
            .as_ref()
            .filter(|sort| sort.key == "outcome" || sort.key == "actor")
        {
            builder
                .push(" ORDER BY ")
                .push(if sort.key == "outcome" {
                    outcome_sql()
                } else {
                    format!("COALESCE({}, actor_id::text, '')", display_label_sql()[0])
                })
                .push(sort.sql_direction());
        } else {
            push_order_by(
                &mut builder,
                AUDIT_EVENT_COLUMNS,
                filter.sort.as_ref(),
                "created_at DESC, id",
            );
        }
        if filter.sort.is_some() {
            builder.push(", created_at DESC, id");
        }
        builder
            .push(" LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to list audit events: {error}")))?;
        rows.iter().map(Self::parse_event_row).collect()
    }
    async fn count_events(&self, filter: AuditEventFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM audit_event");
        Self::push_filter(&mut builder, &filter);
        let count = builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to count audit events: {error}"))
            })?;
        Ok(count.max(0) as u64)
    }
}
