use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{Executor, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::audit_repository::{AUDIT_EVENT_COLUMNS, AuditEventFilter, AuditRepository};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::shared::list_query::{
    WhereState, push_order_by, push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxAuditRepository {
    pool: PgPool,
}

impl SqlxAuditRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn load_hydration(&self, events: &[AuditEvent]) -> Result<AuditHydration, AppError> {
        AuditHydration::load(&self.pool, events).await
    }

    pub async fn lookup_targets(
        &self,
        target_type: Option<&str>,
        search: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AuditTargetOption>, AppError> {
        lookup_audit_targets(&self.pool, target_type, search, limit).await
    }

    fn parse_event_row(row: &PgRow) -> Result<AuditEvent, AppError> {
        Ok(AuditEvent {
            id: row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit event id: {error}"))
            })?,
            event_type: row.try_get("event_type").map_err(|error| {
                AppError::Database(format!("Failed to read audit event type: {error}"))
            })?,
            actor_id: row.try_get("actor_id").map_err(|error| {
                AppError::Database(format!("Failed to read audit event actor_id: {error}"))
            })?,
            actor_kind: row.try_get("actor_kind").map_err(|error| {
                AppError::Database(format!("Failed to read audit event actor_kind: {error}"))
            })?,
            target_type: row.try_get("target_type").map_err(|error| {
                AppError::Database(format!("Failed to read audit event target_type: {error}"))
            })?,
            target_id: row.try_get("target_id").map_err(|error| {
                AppError::Database(format!("Failed to read audit event target_id: {error}"))
            })?,
            metadata: row.try_get("metadata").map_err(|error| {
                AppError::Database(format!("Failed to read audit event metadata: {error}"))
            })?,
            created_at: row.try_get("created_at").map_err(|error| {
                AppError::Database(format!("Failed to read audit event created_at: {error}"))
            })?,
        })
    }

    fn push_filter(builder: &mut QueryBuilder<'_, Postgres>, filter: &AuditEventFilter) {
        let mut where_state = WhereState::new();

        if let Some(query) = &filter.query {
            let pattern = format!("%{query}%");
            where_state.and(builder);
            builder.push("(");
            builder.push("event_type ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR COALESCE(target_type, '') ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR COALESCE(actor_id::text, '') ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR COALESCE(target_id::text, '') ILIKE ");
            builder.push_bind(pattern.clone());
            builder.push(" OR metadata::text ILIKE ");
            builder.push_bind(pattern);
            builder.push(")");
        }

        // Exact-match, multi-value (`IN`) filters on plain columns. Values are
        // matched verbatim (no case folding) to preserve current semantics.
        push_string_in(builder, &mut where_state, "event_type", &filter.event_type);
        push_string_in(builder, &mut where_state, "actor_kind", &filter.actor_kind);
        push_uuid_in(builder, &mut where_state, "actor_id", &filter.actor_id);
        push_string_in(
            builder,
            &mut where_state,
            "target_type",
            &filter.target_type,
        );
        push_uuid_in(builder, &mut where_state, "target_id", &filter.target_id);

        // `result` is not a column: it matches a lowered value against any of the
        // three metadata decision/status/result keys. Multi-value means "any of
        // the supplied (lowered) values matches any of the three keys".
        if !filter.result.is_empty() {
            let lowered: Vec<String> = filter
                .result
                .iter()
                .map(|value| value.to_lowercase())
                .collect();
            where_state.and(builder);
            builder.push("(");
            for (index, key) in ["decision", "status", "result"].iter().enumerate() {
                if index > 0 {
                    builder.push(" OR ");
                }
                builder.push(format!("lower(COALESCE(metadata ->> '{key}', '')) IN ("));
                let mut separated = builder.separated(", ");
                for value in &lowered {
                    separated.push_bind(value.clone());
                }
                builder.push(")");
            }
            builder.push(")");
        }

        // MCP metadata filters: each is a case-insensitive multi-value `IN`
        // over a single JSON metadata key ("any of the supplied values").
        for (key, values) in [
            ("tool", &filter.mcp_tool),
            ("tool_group", &filter.mcp_tool_group),
            ("protocol", &filter.mcp_protocol),
            ("risk", &filter.mcp_risk),
        ] {
            if values.is_empty() {
                continue;
            }
            where_state.and(builder);
            builder.push(format!("lower(COALESCE(metadata ->> '{key}', '')) IN ("));
            let mut separated = builder.separated(", ");
            for value in values.iter() {
                separated.push_bind(value.to_lowercase());
            }
            builder.push(")");
        }

        if let Some(created_from) = filter.created_from {
            where_state.and(builder);
            builder.push("created_at >= ");
            builder.push_bind(created_from);
        }

        if let Some(created_to) = filter.created_to {
            where_state.and(builder);
            builder.push("created_at <= ");
            builder.push_bind(created_to);
        }
    }
}

pub(crate) async fn insert_audit_event<'e, E>(
    executor: E,
    event: &AuditEvent,
) -> Result<(), AppError>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO audit_event (
            id,
            event_type,
            actor_id,
            actor_kind,
            target_type,
            target_id,
            metadata,
            created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(event.id)
    .bind(&event.event_type)
    .bind(event.actor_id)
    .bind(&event.actor_kind)
    .bind(&event.target_type)
    .bind(event.target_id)
    .bind(&event.metadata)
    .bind(event.created_at)
    .execute(executor)
    .await
    .map_err(|error| AppError::Database(format!("Failed to insert audit event: {error}")))?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AuditObjectRef {
    pub kind: String,
    pub id: Option<String>,
    pub label: String,
    pub secondary: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AuditTargetOption {
    pub target_type: String,
    pub kind: String,
    pub id: String,
    pub label: String,
    pub secondary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionHydration {
    pub(crate) id: Uuid,
    pub(crate) target_id: Uuid,
    pub(crate) protocol: String,
    pub(crate) status: String,
}

#[derive(Debug, Default)]
pub struct AuditHydration {
    pub(crate) users: HashMap<Uuid, AuditObjectRef>,
    pub(crate) tokens: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_service_principals: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_job_tokens: HashMap<Uuid, AuditObjectRef>,
    pub(crate) sessions: HashMap<Uuid, SessionHydration>,
    pub(crate) resources: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbooks: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_versions: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_target_sets: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_assignments: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_run_groups: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_runs: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_schedules: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_artifacts: HashMap<Uuid, AuditObjectRef>,
    pub(crate) playbook_run_events: HashMap<Uuid, AuditObjectRef>,
}

#[derive(Debug, Default)]
struct AuditHydrationIds {
    user_ids: HashSet<Uuid>,
    token_ids: HashSet<Uuid>,
    playbook_service_principal_ids: HashSet<Uuid>,
    playbook_job_token_ids: HashSet<Uuid>,
    session_ids: HashSet<Uuid>,
    resource_ids: HashSet<Uuid>,
    playbook_ids: HashSet<Uuid>,
    playbook_version_ids: HashSet<Uuid>,
    playbook_target_set_ids: HashSet<Uuid>,
    playbook_assignment_ids: HashSet<Uuid>,
    playbook_run_group_ids: HashSet<Uuid>,
    playbook_run_ids: HashSet<Uuid>,
    playbook_schedule_ids: HashSet<Uuid>,
    playbook_artifact_ids: HashSet<Uuid>,
    playbook_run_event_ids: HashSet<Uuid>,
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

    pub(crate) fn with_secondary(mut self, secondary: Option<String>) -> Self {
        self.secondary = secondary;
        self
    }

    pub(crate) fn with_kind(&self, kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: self.id.clone(),
            label: self.label.clone(),
            secondary: self.secondary.clone(),
        }
    }
}

impl AuditHydration {
    async fn load(pool: &PgPool, events: &[AuditEvent]) -> Result<Self, AppError> {
        let mut ids = AuditHydrationIds::from_events(events);
        let mut hydration = Self::default();
        hydration.load_identity_refs(pool, &mut ids).await?;
        hydration.load_playbook_refs(pool, &ids).await?;
        Ok(hydration)
    }

    /// Hydrate the actor/token/session/resource references, extending the
    /// resource id set with each session's target before loading resources.
    async fn load_identity_refs(
        &mut self,
        pool: &PgPool,
        ids: &mut AuditHydrationIds,
    ) -> Result<(), AppError> {
        self.users = load_users(pool, &ids.user_ids).await?;
        self.tokens = load_tokens(pool, &ids.token_ids).await?;
        self.playbook_service_principals =
            load_playbook_service_principals(pool, &ids.playbook_service_principal_ids).await?;
        self.playbook_job_tokens =
            load_playbook_job_tokens(pool, &ids.playbook_job_token_ids).await?;
        self.sessions = load_sessions(pool, &ids.session_ids).await?;
        ids.resource_ids
            .extend(self.sessions.values().map(|session| session.target_id));
        self.resources = load_resources(pool, &ids.resource_ids).await?;
        Ok(())
    }

    /// Hydrate the playbook-family references.
    async fn load_playbook_refs(
        &mut self,
        pool: &PgPool,
        ids: &AuditHydrationIds,
    ) -> Result<(), AppError> {
        self.playbooks = load_playbooks(pool, &ids.playbook_ids).await?;
        self.playbook_versions = load_playbook_versions(pool, &ids.playbook_version_ids).await?;
        self.playbook_target_sets =
            load_playbook_target_sets(pool, &ids.playbook_target_set_ids).await?;
        self.playbook_assignments =
            load_playbook_assignments(pool, &ids.playbook_assignment_ids).await?;
        self.playbook_run_groups =
            load_playbook_run_groups(pool, &ids.playbook_run_group_ids).await?;
        self.playbook_runs = load_playbook_runs(pool, &ids.playbook_run_ids).await?;
        self.playbook_schedules = load_playbook_schedules(pool, &ids.playbook_schedule_ids).await?;
        self.playbook_artifacts = load_playbook_artifacts(pool, &ids.playbook_artifact_ids).await?;
        self.playbook_run_events =
            load_playbook_run_events(pool, &ids.playbook_run_event_ids).await?;
        Ok(())
    }
}

impl AuditHydrationIds {
    fn add_typed_reference(&mut self, resource_type: &str, id: Uuid) {
        match normalize_audit_type(Some(resource_type)).as_deref() {
            Some("user_api_token") | Some("api_token") => {
                self.token_ids.insert(id);
            }
            Some("playbook_job_token") => {
                self.playbook_job_token_ids.insert(id);
            }
            Some("playbook_service_principal") => {
                self.playbook_service_principal_ids.insert(id);
            }
            Some("connection_session") => {
                self.session_ids.insert(id);
            }
            Some("connection_target")
            | Some("device")
            | Some("dcim_device")
            | Some("vm")
            | Some("virtual_machine")
            | Some("virtualization_virtualmachine") => {
                self.resource_ids.insert(id);
            }
            Some("playbook") => {
                self.playbook_ids.insert(id);
            }
            Some("playbook_version") => {
                self.playbook_version_ids.insert(id);
            }
            Some("playbook_target_set") => {
                self.playbook_target_set_ids.insert(id);
            }
            Some("playbook_assignment") => {
                self.playbook_assignment_ids.insert(id);
            }
            Some("playbook_run_group") => {
                self.playbook_run_group_ids.insert(id);
            }
            Some("playbook_run") => {
                self.playbook_run_ids.insert(id);
            }
            Some("playbook_schedule") => {
                self.playbook_schedule_ids.insert(id);
            }
            Some("playbook_artifact") => {
                self.playbook_artifact_ids.insert(id);
            }
            Some("playbook_run_event") => {
                self.playbook_run_event_ids.insert(id);
            }
            _ => {}
        }
    }

    fn from_events(events: &[AuditEvent]) -> Self {
        let mut ids = Self::default();
        for event in events {
            if let Some(actor_id) = event.actor_id {
                match event.actor_kind.as_str() {
                    "service_principal" => {
                        ids.playbook_service_principal_ids.insert(actor_id);
                    }
                    "playbook_job_token" => {
                        ids.playbook_job_token_ids.insert(actor_id);
                    }
                    _ => {
                        ids.user_ids.insert(actor_id);
                    }
                }
            }

            if let Some(target_id) = event.target_id
                && let Some(target_type) = event.target_type.as_deref()
            {
                ids.add_typed_reference(target_type, target_id);
            }

            if let Some(token_id) = metadata_uuid(&event.metadata, "token_id")
                .or_else(|| metadata_uuid(&event.metadata, "api_token_id"))
            {
                ids.token_ids.insert(token_id);
            }
            if let Some(job_token_id) = metadata_uuid(&event.metadata, "job_token_id") {
                ids.playbook_job_token_ids.insert(job_token_id);
            }
            if let Some(service_principal_id) =
                metadata_uuid(&event.metadata, "service_principal_id")
            {
                ids.playbook_service_principal_ids
                    .insert(service_principal_id);
            }
            if let Some(session_id) = metadata_uuid(&event.metadata, "session_id") {
                ids.session_ids.insert(session_id);
            }
            if let Some(target_id) = metadata_uuid(&event.metadata, "target_id") {
                ids.resource_ids.insert(target_id);
            }
            if let Some(resource_id) = metadata_uuid(&event.metadata, "resource_id") {
                if let Some(resource_type) = metadata_string(&event.metadata, "resource_type") {
                    ids.add_typed_reference(resource_type.as_str(), resource_id);
                } else {
                    ids.resource_ids.insert(resource_id);
                }
            }
            for (metadata_key, resource_type) in [
                ("playbook_id", "playbook"),
                ("version_id", "playbook_version"),
                ("target_set_id", "playbook_target_set"),
                ("assignment_id", "playbook_assignment"),
                ("run_group_id", "playbook_run_group"),
                ("run_id", "playbook_run"),
                ("schedule_id", "playbook_schedule"),
                ("artifact_id", "playbook_artifact"),
                ("event_id", "playbook_run_event"),
                ("service_principal_id", "playbook_service_principal"),
            ] {
                if let Some(id) = metadata_uuid(&event.metadata, metadata_key) {
                    ids.add_typed_reference(resource_type, id);
                }
            }
        }
        ids
    }
}

async fn load_users(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, username, email
        FROM identity_user
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to hydrate audit users: {error}")))?;

    let mut users = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit user id: {error}"))
        })?;
        let username: String = row.try_get("username").map_err(|error| {
            AppError::Database(format!("Failed to read audit user username: {error}"))
        })?;
        let email: String = row.try_get("email").map_err(|error| {
            AppError::Database(format!("Failed to read audit user email: {error}"))
        })?;
        let label = first_non_empty([username.as_str(), email.as_str()])
            .unwrap_or("unknown user")
            .to_string();
        let secondary = if email.trim().is_empty() || email == label {
            None
        } else {
            Some(email)
        };
        users.insert(
            id,
            AuditObjectRef::new("user", id, label).with_secondary(secondary),
        );
    }
    Ok(users)
}

async fn load_tokens(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, name, token_prefix, status
        FROM auth_user_api_token
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to hydrate audit API tokens: {error}")))?;

    let mut tokens = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit API token id: {error}"))
        })?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!("Failed to read audit API token name: {error}"))
        })?;
        let token_prefix: String = row.try_get("token_prefix").map_err(|error| {
            AppError::Database(format!("Failed to read audit API token prefix: {error}"))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!("Failed to read audit API token status: {error}"))
        })?;
        let secondary = first_non_empty([token_prefix.as_str(), status.as_str()])
            .map(|_| format!("{token_prefix} · {status}"));
        tokens.insert(
            id,
            AuditObjectRef::new("api_token", id, name).with_secondary(secondary),
        );
    }
    Ok(tokens)
}

async fn load_sessions(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, SessionHydration>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, target_id, protocol, status::text AS status
        FROM connections_session
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit connection sessions: {error}"
        ))
    })?;

    let mut sessions = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit session id: {error}"))
        })?;
        let target_id: Uuid = row.try_get("target_id").map_err(|error| {
            AppError::Database(format!("Failed to read audit session target_id: {error}"))
        })?;
        let protocol: String = row.try_get("protocol").map_err(|error| {
            AppError::Database(format!("Failed to read audit session protocol: {error}"))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!("Failed to read audit session status: {error}"))
        })?;
        sessions.insert(
            id,
            SessionHydration {
                id,
                target_id,
                protocol,
                status,
            },
        );
    }
    Ok(sessions)
}

async fn load_resources(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let device_rows = sqlx::query(
        r#"
        SELECT id, name, status
        FROM dcim_device
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to hydrate audit devices: {error}")))?;
    let vm_rows = sqlx::query(
        r#"
        SELECT id, name, status
        FROM virtualization_virtualmachine
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to hydrate audit virtual machines: {error}"))
    })?;

    let mut resources = HashMap::with_capacity(device_rows.len() + vm_rows.len());
    for row in device_rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit device id: {error}"))
        })?;
        let name: Option<String> = row.try_get("name").map_err(|error| {
            AppError::Database(format!("Failed to read audit device name: {error}"))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!("Failed to read audit device status: {error}"))
        })?;
        let label = name
            .as_deref()
            .and_then(|value| first_non_empty([value]))
            .unwrap_or("unnamed device")
            .to_string();
        resources.insert(
            id,
            AuditObjectRef::new("device", id, label).with_secondary(Some(status)),
        );
    }
    for row in vm_rows {
        let id: Uuid = row
            .try_get("id")
            .map_err(|error| AppError::Database(format!("Failed to read audit VM id: {error}")))?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!("Failed to read audit VM name: {error}"))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!("Failed to read audit VM status: {error}"))
        })?;
        resources.insert(
            id,
            AuditObjectRef::new("virtual_machine", id, name).with_secondary(Some(status)),
        );
    }
    Ok(resources)
}

async fn load_playbooks(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, slug, name, status, runtime, risk
        FROM playbook
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to hydrate audit playbooks: {error}")))?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook id: {error}"))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook slug: {error}"))
        })?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook name: {error}"))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook status: {error}"))
        })?;
        let runtime: String = row.try_get("runtime").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook runtime: {error}"))
        })?;
        let risk: String = row.try_get("risk").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook risk: {error}"))
        })?;
        refs.insert(
            id,
            AuditObjectRef::new("playbook", id, name).with_secondary(lookup_secondary([
                Some(slug),
                Some(status),
                Some(runtime),
                Some(risk),
            ])),
        );
    }
    Ok(refs)
}

async fn load_playbook_versions(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT v.id, v.version, v.language, v.runtime, p.slug
        FROM playbook_version v
        JOIN playbook p ON p.id = v.playbook_id
        WHERE v.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook versions: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook version id: {error}"))
        })?;
        let version: i32 = row.try_get("version").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook version number: {error}"
            ))
        })?;
        let language: String = row.try_get("language").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook version language: {error}"
            ))
        })?;
        let runtime: String = row.try_get("runtime").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook version runtime: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook version slug: {error}"
            ))
        })?;
        refs.insert(
            id,
            AuditObjectRef::new("playbook_version", id, format!("{slug} v{version}"))
                .with_secondary(lookup_secondary([Some(runtime), Some(language)])),
        );
    }
    Ok(refs)
}

async fn load_playbook_target_sets(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, name, mode, entity_kind
        FROM playbook_target_set
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook target sets: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook target set id: {error}"
            ))
        })?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook target set name: {error}"
            ))
        })?;
        let mode: String = row.try_get("mode").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook target set mode: {error}"
            ))
        })?;
        let entity_kind: String = row.try_get("entity_kind").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook target set entity_kind: {error}"
            ))
        })?;
        refs.insert(
            id,
            AuditObjectRef::new("playbook_target_set", id, name)
                .with_secondary(lookup_secondary([Some(mode), Some(entity_kind)])),
        );
    }
    Ok(refs)
}

async fn load_playbook_assignments(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT a.id, COALESCE(a.name, p.name, p.slug) AS label, p.slug,
               a.scope_kind, a.enabled, a.visible, a.executable
        FROM playbook_assignment a
        JOIN playbook p ON p.id = a.playbook_id
        WHERE a.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook assignments: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment id: {error}"
            ))
        })?;
        let label: String = row.try_get("label").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment label: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment slug: {error}"
            ))
        })?;
        let scope_kind: String = row.try_get("scope_kind").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment scope: {error}"
            ))
        })?;
        let enabled: bool = row.try_get("enabled").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment enabled flag: {error}"
            ))
        })?;
        let visible: bool = row.try_get("visible").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment visible flag: {error}"
            ))
        })?;
        let executable: bool = row.try_get("executable").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook assignment executable flag: {error}"
            ))
        })?;
        let state = format!(
            "{}{}{}",
            if enabled { "enabled" } else { "disabled" },
            if visible { " · visible" } else { " · hidden" },
            if executable {
                " · executable"
            } else {
                " · not executable"
            }
        );
        refs.insert(
            id,
            AuditObjectRef::new("playbook_assignment", id, label).with_secondary(lookup_secondary(
                [Some(slug), Some(scope_kind), Some(state)],
            )),
        );
    }
    Ok(refs)
}

async fn load_playbook_run_groups(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT g.id, p.slug, g.trigger_kind, g.status, g.total_runs
        FROM playbook_run_group g
        JOIN playbook p ON p.id = g.playbook_id
        WHERE g.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook run groups: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run group id: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run group slug: {error}"
            ))
        })?;
        let trigger_kind: String = row.try_get("trigger_kind").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run group trigger: {error}"
            ))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run group status: {error}"
            ))
        })?;
        let total_runs: i32 = row.try_get("total_runs").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run group total_runs: {error}"
            ))
        })?;
        refs.insert(
            id,
            AuditObjectRef::new("playbook_run_group", id, format!("{slug} run group"))
                .with_secondary(lookup_secondary([
                    Some(trigger_kind),
                    Some(status),
                    Some(format!("{total_runs} runs")),
                ])),
        );
    }
    Ok(refs)
}

async fn load_playbook_runs(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT r.id, p.slug, r.target_name, r.status, r.attempt
        FROM playbook_run r
        JOIN playbook p ON p.id = r.playbook_id
        WHERE r.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to hydrate audit playbook runs: {error}"))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook run id: {error}"))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook run slug: {error}"))
        })?;
        let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook run target: {error}"))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!("Failed to read audit playbook run status: {error}"))
        })?;
        let attempt: i32 = row.try_get("attempt").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run attempt: {error}"
            ))
        })?;
        let label = target_name
            .as_deref()
            .and_then(|value| first_non_empty([value]))
            .map(|target| format!("{slug} · {target}"))
            .unwrap_or_else(|| format!("{slug} run"));
        refs.insert(
            id,
            AuditObjectRef::new("playbook_run", id, label).with_secondary(lookup_secondary([
                Some(status),
                Some(format!("attempt {attempt}")),
            ])),
        );
    }
    Ok(refs)
}

async fn load_playbook_schedules(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.name, s.enabled, s.cron_expression, s.timezone, p.slug
        FROM playbook_schedule s
        JOIN playbook p ON p.id = s.playbook_id
        WHERE s.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook schedules: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook schedule id: {error}"
            ))
        })?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook schedule name: {error}"
            ))
        })?;
        let enabled: bool = row.try_get("enabled").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook schedule enabled flag: {error}"
            ))
        })?;
        let cron_expression: String = row.try_get("cron_expression").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook schedule cron: {error}"
            ))
        })?;
        let timezone: String = row.try_get("timezone").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook schedule timezone: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook schedule slug: {error}"
            ))
        })?;
        let state = if enabled { "enabled" } else { "disabled" }.to_string();
        refs.insert(
            id,
            AuditObjectRef::new("playbook_schedule", id, name).with_secondary(lookup_secondary([
                Some(slug),
                Some(state),
                Some(cron_expression),
                Some(timezone),
            ])),
        );
    }
    Ok(refs)
}

async fn load_playbook_artifacts(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.name, a.artifact_type, a.content_type, a.content_size_bytes, p.slug
        FROM playbook_artifact a
        JOIN playbook_run r ON r.id = a.run_id
        JOIN playbook p ON p.id = r.playbook_id
        WHERE a.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook artifacts: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook artifact id: {error}"
            ))
        })?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook artifact name: {error}"
            ))
        })?;
        let artifact_type: String = row.try_get("artifact_type").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook artifact type: {error}"
            ))
        })?;
        let content_type: String = row.try_get("content_type").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook artifact content type: {error}"
            ))
        })?;
        let content_size_bytes: i64 = row.try_get("content_size_bytes").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook artifact size: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook artifact slug: {error}"
            ))
        })?;
        refs.insert(
            id,
            AuditObjectRef::new("playbook_artifact", id, name).with_secondary(lookup_secondary([
                Some(slug),
                Some(artifact_type),
                Some(content_type),
                Some(format!("{content_size_bytes} bytes")),
            ])),
        );
    }
    Ok(refs)
}

async fn load_playbook_run_events(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT e.id, e.event_sequence, e.event_kind, e.event_type, p.slug, r.target_name
        FROM playbook_run_event e
        JOIN playbook_run r ON r.id = e.run_id
        JOIN playbook p ON p.id = r.playbook_id
        WHERE e.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook run events: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run event id: {error}"
            ))
        })?;
        let event_sequence: i64 = row.try_get("event_sequence").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run event sequence: {error}"
            ))
        })?;
        let event_kind: String = row.try_get("event_kind").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run event kind: {error}"
            ))
        })?;
        let event_type: String = row.try_get("event_type").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run event type: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run event slug: {error}"
            ))
        })?;
        let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook run event target: {error}"
            ))
        })?;
        let label = target_name
            .as_deref()
            .and_then(|value| first_non_empty([value]))
            .map(|target| format!("{slug} · {target} #{event_sequence}"))
            .unwrap_or_else(|| format!("{slug} event #{event_sequence}"));
        refs.insert(
            id,
            AuditObjectRef::new("playbook_run_event", id, label)
                .with_secondary(lookup_secondary([Some(event_kind), Some(event_type)])),
        );
    }
    Ok(refs)
}

async fn load_playbook_job_tokens(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT jt.id, jt.token_prefix, jt.status, p.slug, r.target_name
        FROM playbook_job_token jt
        JOIN playbook_run r ON r.id = jt.run_id
        JOIN playbook p ON p.id = r.playbook_id
        WHERE jt.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook job tokens: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook job token id: {error}"
            ))
        })?;
        let token_prefix: String = row.try_get("token_prefix").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook job token prefix: {error}"
            ))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook job token status: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook job token slug: {error}"
            ))
        })?;
        let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook job token target: {error}"
            ))
        })?;
        let label = target_name
            .as_deref()
            .and_then(|value| first_non_empty([value]))
            .map(|target| format!("{slug} · {target}"))
            .unwrap_or(slug);
        refs.insert(
            id,
            AuditObjectRef::new("playbook_job_token", id, label)
                .with_secondary(lookup_secondary([Some(token_prefix), Some(status)])),
        );
    }
    Ok(refs)
}

async fn load_playbook_service_principals(
    pool: &PgPool,
    ids: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, AuditObjectRef>, AppError> {
    let ids = sorted_uuids(ids);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, name, slug, status
        FROM playbook_service_principal
        WHERE id = ANY($1)
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to hydrate audit playbook service principals: {error}"
        ))
    })?;

    let mut refs = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook service principal id: {error}"
            ))
        })?;
        let name: String = row.try_get("name").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook service principal name: {error}"
            ))
        })?;
        let slug: String = row.try_get("slug").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook service principal slug: {error}"
            ))
        })?;
        let status: String = row.try_get("status").map_err(|error| {
            AppError::Database(format!(
                "Failed to read audit playbook service principal status: {error}"
            ))
        })?;
        refs.insert(
            id,
            AuditObjectRef::new("playbook_service_principal", id, name)
                .with_secondary(lookup_secondary([Some(slug), Some(status)])),
        );
    }
    Ok(refs)
}

async fn lookup_audit_targets(
    pool: &PgPool,
    target_type: Option<&str>,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let mut items = Vec::new();
    match target_type {
        None => append_all_audit_targets(&mut items, pool, search, limit).await?,
        Some("organization") => {}
        Some(target_type) => {
            append_named_audit_targets(&mut items, pool, target_type, search, limit).await?;
        }
    }
    items.truncate(limit as usize);
    Ok(items)
}

/// Dispatch a named target type to its lookup family. Unknown types add nothing.
async fn append_named_audit_targets(
    items: &mut Vec<AuditTargetOption>,
    pool: &PgPool,
    target_type: &str,
    search: Option<&str>,
    limit: u32,
) -> Result<(), AppError> {
    match target_type {
        "connection_target" => {
            append_limited(
                items,
                lookup_device_targets(pool, search, limit, "connection_target").await?,
                limit,
            );
            append_limited(
                items,
                lookup_vm_targets(pool, search, limit, "connection_target").await?,
                limit,
            );
        }
        "device" | "dcim_device" => append_limited(
            items,
            lookup_device_targets(pool, search, limit, "device").await?,
            limit,
        ),
        "vm" | "virtual_machine" | "virtualization_virtualmachine" => append_limited(
            items,
            lookup_vm_targets(pool, search, limit, "virtual_machine").await?,
            limit,
        ),
        _ => {
            append_identity_audit_targets(items, pool, target_type, search, limit).await?;
            append_playbook_core_audit_targets(items, pool, target_type, search, limit).await?;
            append_playbook_artifact_audit_targets(items, pool, target_type, search, limit).await?;
        }
    }
    Ok(())
}

/// Append the identity/token/session/connection-config lookups for a named
/// target type; unknown types add nothing.
async fn append_identity_audit_targets(
    items: &mut Vec<AuditTargetOption>,
    pool: &PgPool,
    target_type: &str,
    search: Option<&str>,
    limit: u32,
) -> Result<(), AppError> {
    match target_type {
        "user" => append_limited(
            items,
            lookup_user_targets(pool, search, limit).await?,
            limit,
        ),
        "user_api_token" | "api_token" => append_limited(
            items,
            lookup_token_targets(pool, search, limit).await?,
            limit,
        ),
        "connection_session" => append_limited(
            items,
            lookup_session_targets(pool, search, limit).await?,
            limit,
        ),
        "connection_config:record" | "connection_config_record" => append_limited(
            items,
            lookup_connconf_record_targets(pool, search, limit).await?,
            limit,
        ),
        _ => {}
    }
    Ok(())
}

/// Append the core playbook-family lookups for a named target type; unknown
/// types add nothing.
async fn append_playbook_core_audit_targets(
    items: &mut Vec<AuditTargetOption>,
    pool: &PgPool,
    target_type: &str,
    search: Option<&str>,
    limit: u32,
) -> Result<(), AppError> {
    match target_type {
        "playbook" => append_limited(
            items,
            lookup_playbook_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_version" => append_limited(
            items,
            lookup_playbook_version_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_target_set" => append_limited(
            items,
            lookup_playbook_target_set_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_assignment" => append_limited(
            items,
            lookup_playbook_assignment_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_run_group" => append_limited(
            items,
            lookup_playbook_run_group_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_run" => append_limited(
            items,
            lookup_playbook_run_targets(pool, search, limit).await?,
            limit,
        ),
        _ => {}
    }
    Ok(())
}

/// Append the artifact/schedule/token playbook-family lookups for a named
/// target type; unknown types add nothing.
async fn append_playbook_artifact_audit_targets(
    items: &mut Vec<AuditTargetOption>,
    pool: &PgPool,
    target_type: &str,
    search: Option<&str>,
    limit: u32,
) -> Result<(), AppError> {
    match target_type {
        "playbook_schedule" => append_limited(
            items,
            lookup_playbook_schedule_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_artifact" => append_limited(
            items,
            lookup_playbook_artifact_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_run_event" => append_limited(
            items,
            lookup_playbook_run_event_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_job_token" => append_limited(
            items,
            lookup_playbook_job_token_targets(pool, search, limit).await?,
            limit,
        ),
        "playbook_service_principal" => append_limited(
            items,
            lookup_playbook_service_principal_targets(pool, search, limit).await?,
            limit,
        ),
        _ => {}
    }
    Ok(())
}

/// Append every default target lookup used when no target type is selected.
async fn append_all_audit_targets(
    items: &mut Vec<AuditTargetOption>,
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<(), AppError> {
    append_limited(
        items,
        lookup_user_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_token_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_session_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_connconf_record_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_playbook_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_playbook_run_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_playbook_schedule_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_playbook_job_token_targets(pool, search, limit).await?,
        limit,
    );
    append_limited(
        items,
        lookup_device_targets(pool, search, limit, "device").await?,
        limit,
    );
    append_limited(
        items,
        lookup_vm_targets(pool, search, limit, "virtual_machine").await?,
        limit,
    );
    Ok(())
}

fn append_limited(items: &mut Vec<AuditTargetOption>, next: Vec<AuditTargetOption>, limit: u32) {
    let remaining = limit as usize - items.len().min(limit as usize);
    items.extend(next.into_iter().take(remaining));
}

fn lookup_pattern(search: Option<&str>) -> (String, bool) {
    match search.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => (format!("%{value}%"), false),
        None => ("%".to_string(), true),
    }
}

fn lookup_secondary(parts: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let parts: Vec<String> = parts
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

async fn lookup_user_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT id, username, email, status
        FROM identity_user
        WHERE $2 OR id::text ILIKE $1 OR username ILIKE $1 OR email ILIKE $1 OR status ILIKE $1
        ORDER BY username ASC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to search audit users: {error}")))?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup user id: {error}"))
            })?;
            let username: String = row.try_get("username").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup username: {error}"))
            })?;
            let email: String = row.try_get("email").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup user email: {error}"))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup user status: {error}"))
            })?;
            let label = first_non_empty([username.as_str(), email.as_str()])
                .unwrap_or("unknown user")
                .to_string();
            let secondary = lookup_secondary([
                if email == label { None } else { Some(email) },
                Some(status),
            ]);
            Ok(AuditTargetOption {
                target_type: "user".to_string(),
                kind: "user".to_string(),
                id: id.to_string(),
                label,
                secondary,
            })
        })
        .collect()
}

async fn lookup_token_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT t.id, t.name, t.token_prefix, t.status, u.username
        FROM auth_user_api_token t
        JOIN identity_user u ON u.id = t.user_id
        WHERE $2
           OR t.id::text ILIKE $1
           OR t.name ILIKE $1
           OR t.token_prefix ILIKE $1
           OR t.status ILIKE $1
           OR u.username ILIKE $1
           OR u.email ILIKE $1
        ORDER BY t.updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to search audit API tokens: {error}")))?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup token id: {error}"))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup token name: {error}"))
            })?;
            let token_prefix: String = row.try_get("token_prefix").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup token prefix: {error}"))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup token status: {error}"))
            })?;
            let username: String = row.try_get("username").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup token user: {error}"))
            })?;
            Ok(AuditTargetOption {
                target_type: "user_api_token".to_string(),
                kind: "api_token".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([Some(token_prefix), Some(status), Some(username)]),
            })
        })
        .collect()
}

async fn lookup_session_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT
            s.id,
            s.protocol,
            s.status::text AS status,
            u.username,
            COALESCE(d.name, vm.name) AS target_name
        FROM connections_session s
        JOIN identity_user u ON u.id = s.user_id
        LEFT JOIN dcim_device d ON d.id = s.target_id
        LEFT JOIN virtualization_virtualmachine vm ON vm.id = s.target_id
        WHERE $2
           OR s.id::text ILIKE $1
           OR s.protocol ILIKE $1
           OR s.status::text ILIKE $1
           OR u.username ILIKE $1
           OR u.email ILIKE $1
           OR d.name ILIKE $1
           OR vm.name ILIKE $1
        ORDER BY s.started_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit connection sessions: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup session id: {error}"))
            })?;
            let protocol: String = row.try_get("protocol").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup session protocol: {error}"
                ))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup session status: {error}"
                ))
            })?;
            let username: String = row.try_get("username").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup session user: {error}"))
            })?;
            let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup session target: {error}"
                ))
            })?;
            let protocol_label = protocol.to_ascii_uppercase();
            let label = target_name
                .as_deref()
                .map(|target| format!("{protocol_label} · {target}"))
                .unwrap_or(protocol_label);
            Ok(AuditTargetOption {
                target_type: "connection_session".to_string(),
                kind: "connection_session".to_string(),
                id: id.to_string(),
                label,
                secondary: lookup_secondary([Some(status), Some(username)]),
            })
        })
        .collect()
}

async fn lookup_connconf_record_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT r.id, r.name, r.protocol, r.enabled, u.username
        FROM connconf_record r
        LEFT JOIN identity_user u ON u.id = r.owner_user_id
        WHERE $2
           OR r.id::text ILIKE $1
           OR r.name ILIKE $1
           OR r.description ILIKE $1
           OR r.protocol ILIKE $1
           OR u.username ILIKE $1
           OR u.email ILIKE $1
        ORDER BY r.updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit connection config records: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup connection config id: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup connection config name: {error}"
                ))
            })?;
            let protocol: String = row.try_get("protocol").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup connection config protocol: {error}"
                ))
            })?;
            let enabled: bool = row.try_get("enabled").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup connection config enabled: {error}"
                ))
            })?;
            let username: Option<String> = row.try_get("username").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup connection config owner: {error}"
                ))
            })?;
            let state = if enabled { "enabled" } else { "disabled" }.to_string();
            Ok(AuditTargetOption {
                target_type: "connection_config:record".to_string(),
                kind: "connection_config_record".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([Some(protocol), Some(state), username]),
            })
        })
        .collect()
}

async fn lookup_playbook_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT id, slug, name, status, runtime, risk
        FROM playbook
        WHERE $2
           OR id::text ILIKE $1
           OR slug ILIKE $1
           OR name ILIKE $1
           OR status ILIKE $1
           OR runtime ILIKE $1
           OR risk ILIKE $1
           OR category ILIKE $1
        ORDER BY updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to search audit playbooks: {error}")))?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup playbook id: {error}"))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook slug: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook name: {error}"
                ))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook status: {error}"
                ))
            })?;
            let runtime: String = row.try_get("runtime").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook runtime: {error}"
                ))
            })?;
            let risk: String = row.try_get("risk").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook risk: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook".to_string(),
                kind: "playbook".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([Some(slug), Some(status), Some(runtime), Some(risk)]),
            })
        })
        .collect()
}

async fn lookup_playbook_version_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT v.id, v.version, v.language, v.runtime, p.slug, p.name
        FROM playbook_version v
        JOIN playbook p ON p.id = v.playbook_id
        WHERE $2
           OR v.id::text ILIKE $1
           OR v.version::text ILIKE $1
           OR v.language ILIKE $1
           OR v.runtime ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
        ORDER BY v.created_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to search audit playbook versions: {error}"))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook version id: {error}"
                ))
            })?;
            let version: i32 = row.try_get("version").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook version number: {error}"
                ))
            })?;
            let language: String = row.try_get("language").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook version language: {error}"
                ))
            })?;
            let runtime: String = row.try_get("runtime").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook version runtime: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook version slug: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook version name: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_version".to_string(),
                kind: "playbook_version".to_string(),
                id: id.to_string(),
                label: format!("{slug} v{version}"),
                secondary: lookup_secondary([Some(name), Some(runtime), Some(language)]),
            })
        })
        .collect()
}

async fn lookup_playbook_target_set_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT id, name, mode, entity_kind
        FROM playbook_target_set
        WHERE $2
           OR id::text ILIKE $1
           OR name ILIKE $1
           OR mode ILIKE $1
           OR entity_kind ILIKE $1
        ORDER BY updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook target sets: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook target set id: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook target set name: {error}"
                ))
            })?;
            let mode: String = row.try_get("mode").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook target set mode: {error}"
                ))
            })?;
            let entity_kind: String = row.try_get("entity_kind").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook target set entity_kind: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_target_set".to_string(),
                kind: "playbook_target_set".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([Some(mode), Some(entity_kind)]),
            })
        })
        .collect()
}

async fn lookup_playbook_assignment_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT a.id, COALESCE(a.name, p.name, p.slug) AS label, p.slug, a.scope_kind, a.enabled
        FROM playbook_assignment a
        JOIN playbook p ON p.id = a.playbook_id
        WHERE $2
           OR a.id::text ILIKE $1
           OR a.name ILIKE $1
           OR a.scope_kind ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
        ORDER BY a.updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook assignments: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook assignment id: {error}"
                ))
            })?;
            let label: String = row.try_get("label").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook assignment label: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook assignment slug: {error}"
                ))
            })?;
            let scope_kind: String = row.try_get("scope_kind").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook assignment scope: {error}"
                ))
            })?;
            let enabled: bool = row.try_get("enabled").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook assignment enabled flag: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_assignment".to_string(),
                kind: "playbook_assignment".to_string(),
                id: id.to_string(),
                label,
                secondary: lookup_secondary([
                    Some(slug),
                    Some(scope_kind),
                    Some(if enabled { "enabled" } else { "disabled" }.to_string()),
                ]),
            })
        })
        .collect()
}

async fn lookup_playbook_run_group_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT g.id, p.slug, g.trigger_kind, g.status, g.total_runs
        FROM playbook_run_group g
        JOIN playbook p ON p.id = g.playbook_id
        WHERE $2
           OR g.id::text ILIKE $1
           OR g.trigger_kind ILIKE $1
           OR g.status ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
        ORDER BY g.created_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook run groups: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run group id: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run group slug: {error}"
                ))
            })?;
            let trigger_kind: String = row.try_get("trigger_kind").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run group trigger: {error}"
                ))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run group status: {error}"
                ))
            })?;
            let total_runs: i32 = row.try_get("total_runs").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run group total_runs: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_run_group".to_string(),
                kind: "playbook_run_group".to_string(),
                id: id.to_string(),
                label: format!("{slug} run group"),
                secondary: lookup_secondary([
                    Some(trigger_kind),
                    Some(status),
                    Some(format!("{total_runs} runs")),
                ]),
            })
        })
        .collect()
}

async fn lookup_playbook_run_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT r.id, p.slug, r.target_name, r.status, r.attempt
        FROM playbook_run r
        JOIN playbook p ON p.id = r.playbook_id
        WHERE $2
           OR r.id::text ILIKE $1
           OR r.target_name ILIKE $1
           OR r.status ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
        ORDER BY r.queued_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to search audit playbook runs: {error}"))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run id: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run slug: {error}"
                ))
            })?;
            let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run target: {error}"
                ))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run status: {error}"
                ))
            })?;
            let attempt: i32 = row.try_get("attempt").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run attempt: {error}"
                ))
            })?;
            let label = target_name
                .as_deref()
                .and_then(|value| first_non_empty([value]))
                .map(|target| format!("{slug} · {target}"))
                .unwrap_or_else(|| format!("{slug} run"));
            Ok(AuditTargetOption {
                target_type: "playbook_run".to_string(),
                kind: "playbook_run".to_string(),
                id: id.to_string(),
                label,
                secondary: lookup_secondary([Some(status), Some(format!("attempt {attempt}"))]),
            })
        })
        .collect()
}

async fn lookup_playbook_schedule_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.name, s.enabled, s.cron_expression, s.timezone, p.slug
        FROM playbook_schedule s
        JOIN playbook p ON p.id = s.playbook_id
        WHERE $2
           OR s.id::text ILIKE $1
           OR s.name ILIKE $1
           OR s.cron_expression ILIKE $1
           OR s.timezone ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
        ORDER BY s.updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook schedules: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook schedule id: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook schedule name: {error}"
                ))
            })?;
            let enabled: bool = row.try_get("enabled").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook schedule enabled flag: {error}"
                ))
            })?;
            let cron_expression: String = row.try_get("cron_expression").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook schedule cron: {error}"
                ))
            })?;
            let timezone: String = row.try_get("timezone").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook schedule timezone: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook schedule slug: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_schedule".to_string(),
                kind: "playbook_schedule".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([
                    Some(slug),
                    Some(if enabled { "enabled" } else { "disabled" }.to_string()),
                    Some(cron_expression),
                    Some(timezone),
                ]),
            })
        })
        .collect()
}

async fn lookup_playbook_artifact_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.name, a.artifact_type, a.content_type, a.content_size_bytes, p.slug
        FROM playbook_artifact a
        JOIN playbook_run r ON r.id = a.run_id
        JOIN playbook p ON p.id = r.playbook_id
        WHERE $2
           OR a.id::text ILIKE $1
           OR a.name ILIKE $1
           OR a.artifact_type ILIKE $1
           OR a.content_type ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
        ORDER BY a.created_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook artifacts: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook artifact id: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook artifact name: {error}"
                ))
            })?;
            let artifact_type: String = row.try_get("artifact_type").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook artifact type: {error}"
                ))
            })?;
            let content_type: String = row.try_get("content_type").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook artifact content type: {error}"
                ))
            })?;
            let content_size_bytes: i64 = row.try_get("content_size_bytes").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook artifact size: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook artifact slug: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_artifact".to_string(),
                kind: "playbook_artifact".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([
                    Some(slug),
                    Some(artifact_type),
                    Some(content_type),
                    Some(format!("{content_size_bytes} bytes")),
                ]),
            })
        })
        .collect()
}

async fn lookup_playbook_run_event_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT e.id, e.event_sequence, e.event_kind, e.event_type, p.slug, r.target_name
        FROM playbook_run_event e
        JOIN playbook_run r ON r.id = e.run_id
        JOIN playbook p ON p.id = r.playbook_id
        WHERE $2
           OR e.id::text ILIKE $1
           OR e.event_sequence::text ILIKE $1
           OR e.event_kind ILIKE $1
           OR e.event_type ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
           OR r.target_name ILIKE $1
        ORDER BY e.created_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook run events: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run event id: {error}"
                ))
            })?;
            let event_sequence: i64 = row.try_get("event_sequence").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run event sequence: {error}"
                ))
            })?;
            let event_kind: String = row.try_get("event_kind").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run event kind: {error}"
                ))
            })?;
            let event_type: String = row.try_get("event_type").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run event type: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run event slug: {error}"
                ))
            })?;
            let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook run event target: {error}"
                ))
            })?;
            let label = target_name
                .as_deref()
                .and_then(|value| first_non_empty([value]))
                .map(|target| format!("{slug} · {target} #{event_sequence}"))
                .unwrap_or_else(|| format!("{slug} event #{event_sequence}"));
            Ok(AuditTargetOption {
                target_type: "playbook_run_event".to_string(),
                kind: "playbook_run_event".to_string(),
                id: id.to_string(),
                label,
                secondary: lookup_secondary([Some(event_kind), Some(event_type)]),
            })
        })
        .collect()
}

async fn lookup_playbook_job_token_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT jt.id, jt.token_prefix, jt.status, p.slug, r.target_name
        FROM playbook_job_token jt
        JOIN playbook_run r ON r.id = jt.run_id
        JOIN playbook p ON p.id = r.playbook_id
        WHERE $2
           OR jt.id::text ILIKE $1
           OR jt.token_prefix ILIKE $1
           OR jt.status ILIKE $1
           OR p.slug ILIKE $1
           OR p.name ILIKE $1
           OR r.target_name ILIKE $1
        ORDER BY jt.issued_at DESC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook job tokens: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook job token id: {error}"
                ))
            })?;
            let token_prefix: String = row.try_get("token_prefix").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook job token prefix: {error}"
                ))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook job token status: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook job token slug: {error}"
                ))
            })?;
            let target_name: Option<String> = row.try_get("target_name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook job token target: {error}"
                ))
            })?;
            let label = target_name
                .as_deref()
                .and_then(|value| first_non_empty([value]))
                .map(|target| format!("{slug} · {target}"))
                .unwrap_or(slug);
            Ok(AuditTargetOption {
                target_type: "playbook_job_token".to_string(),
                kind: "playbook_job_token".to_string(),
                id: id.to_string(),
                label,
                secondary: lookup_secondary([Some(token_prefix), Some(status)]),
            })
        })
        .collect()
}

async fn lookup_playbook_service_principal_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT id, name, slug, status
        FROM playbook_service_principal
        WHERE $2
           OR id::text ILIKE $1
           OR name ILIKE $1
           OR slug ILIKE $1
           OR status ILIKE $1
        ORDER BY name ASC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to search audit playbook service principals: {error}"
        ))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook service principal id: {error}"
                ))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook service principal name: {error}"
                ))
            })?;
            let slug: String = row.try_get("slug").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook service principal slug: {error}"
                ))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup playbook service principal status: {error}"
                ))
            })?;
            Ok(AuditTargetOption {
                target_type: "playbook_service_principal".to_string(),
                kind: "playbook_service_principal".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([Some(slug), Some(status)]),
            })
        })
        .collect()
}

async fn lookup_device_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
    target_type: &str,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT id, name, status, serial, asset_tag
        FROM dcim_device
        WHERE $2
           OR id::text ILIKE $1
           OR name ILIKE $1
           OR serial ILIKE $1
           OR asset_tag ILIKE $1
           OR description ILIKE $1
        ORDER BY name ASC NULLS LAST
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to search audit devices: {error}")))?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup device id: {error}"))
            })?;
            let name: Option<String> = row.try_get("name").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup device name: {error}"))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup device status: {error}"
                ))
            })?;
            let serial: String = row.try_get("serial").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup device serial: {error}"
                ))
            })?;
            let asset_tag: Option<String> = row.try_get("asset_tag").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read audit lookup device asset tag: {error}"
                ))
            })?;
            let label = name
                .as_deref()
                .and_then(|value| first_non_empty([value]))
                .unwrap_or("unnamed device")
                .to_string();
            Ok(AuditTargetOption {
                target_type: target_type.to_string(),
                kind: "device".to_string(),
                id: id.to_string(),
                label,
                secondary: lookup_secondary([Some(status), Some(serial), asset_tag]),
            })
        })
        .collect()
}

async fn lookup_vm_targets(
    pool: &PgPool,
    search: Option<&str>,
    limit: u32,
    target_type: &str,
) -> Result<Vec<AuditTargetOption>, AppError> {
    let (pattern, include_all) = lookup_pattern(search);
    let rows = sqlx::query(
        r#"
        SELECT id, name, status, serial
        FROM virtualization_virtualmachine
        WHERE $2
           OR id::text ILIKE $1
           OR name ILIKE $1
           OR status ILIKE $1
           OR serial ILIKE $1
           OR description ILIKE $1
        ORDER BY name ASC
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(include_all)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to search audit virtual machines: {error}"))
    })?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup VM id: {error}"))
            })?;
            let name: String = row.try_get("name").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup VM name: {error}"))
            })?;
            let status: String = row.try_get("status").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup VM status: {error}"))
            })?;
            let serial: String = row.try_get("serial").map_err(|error| {
                AppError::Database(format!("Failed to read audit lookup VM serial: {error}"))
            })?;
            Ok(AuditTargetOption {
                target_type: target_type.to_string(),
                kind: "virtual_machine".to_string(),
                id: id.to_string(),
                label: name,
                secondary: lookup_secondary([Some(status), Some(serial)]),
            })
        })
        .collect()
}

fn sorted_uuids(ids: &HashSet<Uuid>) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = ids.iter().copied().collect();
    ids.sort();
    ids
}

fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn metadata_uuid(metadata: &Value, key: &str) -> Option<Uuid> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .and_then(parse_uuidish)
}

fn parse_uuidish(value: &str) -> Option<Uuid> {
    let trimmed = value.trim();
    Uuid::parse_str(trimmed).ok().or_else(|| {
        trimmed
            .rsplit_once([':', '/'])
            .and_then(|(_, suffix)| Uuid::parse_str(suffix.trim()).ok())
    })
}

pub(crate) fn normalize_audit_type(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace(['-', '.'], "_").to_ascii_lowercase())
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    values
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
}

#[async_trait]
impl AuditRepository for SqlxAuditRepository {
    async fn insert(&self, event: AuditEvent) -> Result<(), AppError> {
        insert_audit_event(&self.pool, &event).await
    }

    async fn list_events(
        &self,
        filter: AuditEventFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<AuditEvent>, AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, event_type, actor_id, actor_kind, target_type, target_id, metadata, created_at
            FROM audit_event
            "#,
        );
        Self::push_filter(&mut builder, &filter);

        push_order_by(
            &mut builder,
            AUDIT_EVENT_COLUMNS,
            filter.sort.as_ref(),
            "created_at DESC, id",
        );
        if filter.sort.is_some() {
            builder.push(", created_at DESC, id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::Database(format!("Failed to list audit events: {error}")))?;

        rows.iter()
            .map(Self::parse_event_row)
            .collect::<Result<Vec<AuditEvent>, AppError>>()
    }

    async fn count_events(&self, filter: AuditEventFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM audit_event");
        Self::push_filter(&mut builder, &filter);
        let count: i64 = builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to count audit events: {error}"))
            })?;
        Ok(count.max(0) as u64)
    }
}
