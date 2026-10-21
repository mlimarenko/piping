use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::Query;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::audit::infrastructure::persistence::{
    AUDIT_EVENT_COLUMNS, AuditEventFilter, AuditHydration, AuditObjectRef, AuditTargetOption,
    SessionHydration, metadata_uuid, normalize_audit_type,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AuditEventListQuery {
    pub q: Option<String>,
    /// Comma-separated audit event types; matches any (`IN`).
    pub event_type: Option<String>,
    /// Comma-separated actor channels (`actor_kind`); matches any (`IN`).
    pub actor_kind: Option<String>,
    /// Comma-separated actor UUIDs; matches any (`IN`).
    pub actor_id: Option<String>,
    /// Comma-separated target types; matches any (`IN`).
    pub target_type: Option<String>,
    /// Comma-separated target UUIDs; matches any (`IN`).
    pub target_id: Option<String>,
    /// Comma-separated result values; matches any metadata decision/status/result.
    pub result: Option<String>,
    /// Comma-separated MCP tool names; matches any (case-insensitive).
    pub mcp_tool: Option<String>,
    /// Comma-separated MCP tool groups; matches any (case-insensitive).
    pub mcp_tool_group: Option<String>,
    /// Comma-separated MCP protocols; matches any (case-insensitive).
    pub mcp_protocol: Option<String>,
    /// Comma-separated MCP risk levels; matches any (case-insensitive).
    pub mcp_risk: Option<String>,
    pub created_from: Option<String>,
    pub created_to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditEventResponse {
    pub id: String,
    pub event_type: String,
    pub actor_id: Option<String>,
    pub actor_kind: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub actor: Option<AuditObjectRef>,
    pub actor_via: Option<AuditObjectRef>,
    pub target: Option<AuditObjectRef>,
    pub resource: Option<AuditObjectRef>,
    pub metadata: Value,
    pub created_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditEventListResponse {
    pub items: Vec<AuditEventResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AuditTargetLookupQuery {
    pub target_type: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditTargetLookupResponse {
    pub items: Vec<AuditTargetOption>,
    pub limit: u32,
}

/// Search hydrated audit targets for filter pickers.
#[utoipa::path(
    get,
    path = "/api/v1/audit/targets",
    tag = "audit",
    params(
        ("target_type" = Option<String>, Query, description = "Optional audit target type to search within"),
        ("search" = Option<String>, Query, description = "Case-insensitive search by name, user, token prefix, status, or UUID"),
        ("limit" = Option<u32>, Query, description = "Maximum options to return"),
    ),
    responses(
        (status = 200, description = "Audit target lookup options", body = AuditTargetLookupResponse),
        (status = 403, description = "Audit access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_audit_targets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AuditTargetLookupQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::ViewAudit,
            ResourceType::AuditEvent,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }

    let limit = query.limit.unwrap_or(25).clamp(1, 50);
    let target_type = normalize_optional_string(query.target_type.clone())
        .and_then(|value| normalize_audit_type(Some(&value)))
        .filter(|value| value != "all");
    let search = normalize_optional_string(query.search.clone());
    let items = match state
        .repositories
        .audit
        .lookup_targets(target_type.as_deref(), search.as_deref(), limit)
        .await
    {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    (
        StatusCode::OK,
        Json(AuditTargetLookupResponse { items, limit }),
    )
        .into_response()
}

/// List audit-log events.
#[utoipa::path(
    get,
    path = "/api/v1/audit/events",
    tag = "audit",
    params(
        ("q" = Option<String>, Query, description = "Search audit event type, target type, ids, and metadata"),
        ("event_type" = Option<String>, Query, description = "Filter by audit event type (comma-separated, matches any)"),
        ("actor_kind" = Option<String>, Query, description = "Filter by actor channel/kind (comma-separated, matches any)"),
        ("actor_id" = Option<String>, Query, description = "Filter by actor UUID (comma-separated, matches any)"),
        ("target_type" = Option<String>, Query, description = "Filter by target type (comma-separated, matches any)"),
        ("target_id" = Option<String>, Query, description = "Filter by target UUID (comma-separated, matches any)"),
        ("result" = Option<String>, Query, description = "Filter by metadata decision/status/result (comma-separated, matches any)"),
        ("mcp_tool" = Option<String>, Query, description = "Filter MCP events by metadata.tool (comma-separated, matches any, case-insensitive)"),
        ("mcp_tool_group" = Option<String>, Query, description = "Filter MCP events by metadata.tool_group (comma-separated, matches any, case-insensitive)"),
        ("mcp_protocol" = Option<String>, Query, description = "Filter MCP events by metadata.protocol (comma-separated, matches any, case-insensitive)"),
        ("mcp_risk" = Option<String>, Query, description = "Filter MCP events by metadata.risk (comma-separated, matches any, case-insensitive)"),
        ("created_from" = Option<String>, Query, description = "Inclusive RFC3339 lower bound"),
        ("created_to" = Option<String>, Query, description = "Inclusive RFC3339 upper bound"),
        ("limit" = Option<u32>, Query, description = "Maximum events to return"),
        ("offset" = Option<u32>, Query, description = "Events to skip"),
        ("sort" = Option<String>, Query, description = "Sort key: created_at, event_type, actor, target_type, result (allowlisted; unknown rejected)"),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc"),
    ),
    responses(
        (status = 200, description = "Audit events", body = AuditEventListResponse),
        (status = 400, description = "Invalid filter", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Audit access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_audit_events(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AuditEventListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::ViewAudit,
            ResourceType::AuditEvent,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }

    let filter = match map_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let effective_limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let effective_offset = query.offset.unwrap_or(0);
    let service = state.services.audit_query.clone();
    let events = match service
        .list_events(
            filter.clone(),
            Some(effective_limit),
            Some(effective_offset),
        )
        .await
    {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };
    let total = match service.count_events(filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let hydration = match state.repositories.audit.load_hydration(&events).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let response = AuditEventListResponse {
        items: events
            .into_iter()
            .map(|event| map_event(event, &hydration))
            .collect(),
        total,
        limit: effective_limit,
        offset: effective_offset,
    };

    (StatusCode::OK, Json(response)).into_response()
}

fn map_filter(query: &AuditEventListQuery) -> Result<AuditEventFilter, AppError> {
    Ok(AuditEventFilter {
        query: normalize_optional_string(query.q.clone()),
        event_type: parse_csv_strings(query.event_type.as_deref()),
        actor_kind: parse_csv_strings(query.actor_kind.as_deref()),
        actor_id: parse_csv_uuids(query.actor_id.as_deref(), "actor_id")?,
        target_type: parse_csv_strings(query.target_type.as_deref()),
        target_id: parse_csv_uuids(query.target_id.as_deref(), "target_id")?,
        result: parse_csv_strings(query.result.as_deref()),
        mcp_tool: parse_csv_strings(query.mcp_tool.as_deref()),
        mcp_tool_group: parse_csv_strings(query.mcp_tool_group.as_deref()),
        mcp_protocol: parse_csv_strings(query.mcp_protocol.as_deref()),
        mcp_risk: parse_csv_strings(query.mcp_risk.as_deref()),
        created_from: match normalize_optional_string(query.created_from.clone()) {
            Some(value) => Some(parse_datetime(&value, "created_from")?),
            None => None,
        },
        created_to: match normalize_optional_string(query.created_to.clone()) {
            Some(value) => Some(parse_datetime(&value, "created_to")?),
            None => None,
        },
        sort: SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            AUDIT_EVENT_COLUMNS,
        )?,
    })
}

/// Metadata larger than this (serialized byte length) is replaced with a
/// truncation marker in the list response to keep the payload small. Bulk
/// operations (e.g. inventory imports) stash whole manifests in `metadata`,
/// which bloats the list endpoint without being useful at the collection level.
const METADATA_LIST_BYTE_BUDGET: usize = 2048;

/// Replace oversized `metadata` blobs with a `{ "_truncated": true, "_bytes": n }`
/// marker so the audit list endpoint stays lightweight. The field type is
/// unchanged (`serde_json::Value`), so this does not alter the OpenAPI contract.
fn truncate_metadata(metadata: Value) -> Value {
    let byte_len = serde_json::to_string(&metadata)
        .map(|serialized| serialized.len())
        .unwrap_or(0);
    if byte_len > METADATA_LIST_BYTE_BUDGET {
        serde_json::json!({ "_truncated": true, "_bytes": byte_len })
    } else {
        metadata
    }
}

fn map_event(event: AuditEvent, hydration: &AuditHydration) -> AuditEventResponse {
    let actor = event
        .actor_id
        .and_then(|id| match event.actor_kind.as_str() {
            "service_principal" => hydration
                .playbook_service_principals
                .get(&id)
                .cloned()
                .or_else(|| Some(fallback_ref("playbook_service_principal", id))),
            "playbook_job_token" => hydration
                .playbook_job_tokens
                .get(&id)
                .cloned()
                .or_else(|| Some(fallback_ref("playbook_job_token", id))),
            _ => hydration
                .users
                .get(&id)
                .cloned()
                .or_else(|| Some(fallback_ref("user", id))),
        });
    let actor_via = actor_via_ref(&event, hydration);
    let target = target_ref(&event, hydration);
    let resource = resource_ref(&event, hydration);

    AuditEventResponse {
        id: event.id.to_string(),
        event_type: event.event_type,
        actor_id: event.actor_id.map(|value| value.to_string()),
        actor_kind: event.actor_kind,
        target_type: event.target_type,
        target_id: event.target_id.map(|value| value.to_string()),
        actor,
        actor_via,
        target,
        resource,
        metadata: truncate_metadata(event.metadata),
        created_at: event.created_at.to_rfc3339(),
    }
}

fn actor_via_ref(event: &AuditEvent, hydration: &AuditHydration) -> Option<AuditObjectRef> {
    if event.actor_kind == "service_principal"
        || metadata_string(&event.metadata, "auth_method").as_deref() == Some("service_principal")
        || metadata_string(&event.metadata, "actor_kind").as_deref() == Some("service_principal")
    {
        let service_principal_id = event
            .actor_id
            .or_else(|| metadata_uuid(&event.metadata, "service_principal_id"))?;
        return hydration
            .playbook_service_principals
            .get(&service_principal_id)
            .cloned()
            .or_else(|| {
                Some(fallback_ref(
                    "playbook_service_principal",
                    service_principal_id,
                ))
            });
    }

    if event.actor_kind == "playbook_job_token"
        || metadata_string(&event.metadata, "auth_method").as_deref() == Some("playbook_job_token")
    {
        let job_token_id = event
            .actor_id
            .or_else(|| metadata_uuid(&event.metadata, "job_token_id"))?;
        return hydration
            .playbook_job_tokens
            .get(&job_token_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_job_token", job_token_id)));
    }

    let token_id = metadata_uuid(&event.metadata, "token_id")
        .or_else(|| metadata_uuid(&event.metadata, "api_token_id"))
        .or_else(|| {
            if normalize_audit_type(event.target_type.as_deref()).as_deref()
                == Some("user_api_token")
            {
                event.target_id
            } else {
                None
            }
        })?;
    let token = hydration
        .tokens
        .get(&token_id)
        .cloned()
        .unwrap_or_else(|| fallback_ref("api_token", token_id));
    if event.event_type.starts_with("mcp.") {
        Some(token.with_kind("mcp_agent"))
    } else if metadata_string(&event.metadata, "auth_method").as_deref() == Some("api_token") {
        Some(token)
    } else {
        None
    }
}

fn target_ref(event: &AuditEvent, hydration: &AuditHydration) -> Option<AuditObjectRef> {
    let target_id = event.target_id?;
    match normalize_audit_type(event.target_type.as_deref()).as_deref() {
        Some(kind) if kind.starts_with("playbook") => {
            ref_for_type_and_id(kind, target_id, hydration)
                .or_else(|| Some(fallback_ref(kind, target_id)))
        }
        Some("user_api_token") => hydration
            .tokens
            .get(&target_id)
            .cloned()
            .or_else(|| Some(fallback_ref("api_token", target_id))),
        Some("connection_session") => hydration
            .sessions
            .get(&target_id)
            .map(session_ref)
            .or_else(|| Some(fallback_ref("connection_session", target_id))),
        Some("connection_target")
        | Some("device")
        | Some("dcim_device")
        | Some("vm")
        | Some("virtual_machine")
        | Some("virtualization_virtualmachine") => hydration
            .resources
            .get(&target_id)
            .cloned()
            .or_else(|| Some(fallback_ref("connection_target", target_id))),
        Some(kind) => Some(fallback_ref(kind, target_id)),
        None => Some(fallback_ref("target", target_id)),
    }
}

fn resource_ref(event: &AuditEvent, hydration: &AuditHydration) -> Option<AuditObjectRef> {
    if let Some(resource_id) = metadata_uuid(&event.metadata, "resource_id")
        && let Some(resource_type) = metadata_string(&event.metadata, "resource_type")
        && let Some(reference) = ref_for_type_and_id(resource_type.as_str(), resource_id, hydration)
    {
        return Some(reference);
    }
    if let Some(target_id) = metadata_uuid(&event.metadata, "target_id")
        && let Some(reference) = hydration.resources.get(&target_id).cloned()
    {
        return Some(reference);
    }
    if normalize_audit_type(event.target_type.as_deref()).as_deref() == Some("connection_session")
        && let Some(target_id) = event
            .target_id
            .and_then(|id| hydration.sessions.get(&id).map(|session| session.target_id))
    {
        return hydration.resources.get(&target_id).cloned();
    }
    None
}

fn ref_for_type_and_id(
    resource_type: &str,
    resource_id: Uuid,
    hydration: &AuditHydration,
) -> Option<AuditObjectRef> {
    match normalize_audit_type(Some(resource_type)).as_deref() {
        Some("playbook") => hydration
            .playbooks
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook", resource_id))),
        Some("playbook_version") => hydration
            .playbook_versions
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_version", resource_id))),
        Some("playbook_target_set") => hydration
            .playbook_target_sets
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_target_set", resource_id))),
        Some("playbook_assignment") => hydration
            .playbook_assignments
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_assignment", resource_id))),
        Some("playbook_run_group") => hydration
            .playbook_run_groups
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_run_group", resource_id))),
        Some("playbook_run") => hydration
            .playbook_runs
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_run", resource_id))),
        Some("playbook_schedule") => hydration
            .playbook_schedules
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_schedule", resource_id))),
        Some("playbook_artifact") => hydration
            .playbook_artifacts
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_artifact", resource_id))),
        Some("playbook_run_event") => hydration
            .playbook_run_events
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_run_event", resource_id))),
        Some("playbook_job_token") => hydration
            .playbook_job_tokens
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_job_token", resource_id))),
        Some("playbook_service_principal") => hydration
            .playbook_service_principals
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("playbook_service_principal", resource_id))),
        Some("user_api_token") => hydration
            .tokens
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("api_token", resource_id))),
        Some("connection_session") => hydration
            .sessions
            .get(&resource_id)
            .map(session_ref)
            .or_else(|| Some(fallback_ref("connection_session", resource_id))),
        Some("connection_target")
        | Some("device")
        | Some("dcim_device")
        | Some("vm")
        | Some("virtual_machine")
        | Some("virtualization_virtualmachine") => hydration
            .resources
            .get(&resource_id)
            .cloned()
            .or_else(|| Some(fallback_ref("connection_target", resource_id))),
        Some(kind) => Some(fallback_ref(kind, resource_id)),
        None => None,
    }
}

fn session_ref(session: &SessionHydration) -> AuditObjectRef {
    AuditObjectRef::new(
        "connection_session",
        session.id,
        session.protocol.to_ascii_uppercase(),
    )
    .with_secondary(Some(session.status.clone()))
}

fn fallback_ref(kind: &str, id: Uuid) -> AuditObjectRef {
    AuditObjectRef::new(kind, id, short_uuid(id))
}

fn short_uuid(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_datetime(value: &str, label: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map_err(|error| {
            AppError::Validation(format!("Invalid {label} (RFC3339 expected): {error}"))
        })
        .map(|value| value.with_timezone(&Utc))
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}
