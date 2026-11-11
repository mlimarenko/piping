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

use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::rest::envelope_extractors::{Path, Query};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::audit::application::authorization::{
    ensure_audit_collection_access, ensure_audit_event_access,
};
use crate::contexts::audit::domain::event_context::{
    serialized_lifecycle_states, serialized_values,
};
use crate::contexts::audit::domain::{
    AuditContext, AuditEvent, AuditOutcome,
    safe_projection::{project_context, project_metadata},
};
use crate::contexts::audit::infrastructure::persistence::{
    AUDIT_EVENT_COLUMNS, AuditEventFilter, AuditObjectRef, AuditTargetOption, normalize_audit_type,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditEventListQuery {
    /// Search event and target types, event/actor/target IDs, and recorded actor/target display labels.
    pub q: Option<String>,
    pub event_type: Option<String>,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub outcome: Option<String>,
    pub state_domain: Option<String>,
    pub state_value: Option<String>,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub run_id: Option<String>,
    pub run_group_id: Option<String>,
    pub created_from: Option<String>,
    pub created_to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
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
    pub target: Option<AuditObjectRef>,
    pub context: AuditContext,
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

/// Search recorded targets by identity and display name.
#[utoipa::path(get, path = "/api/v1/audit/targets", tag = "audit", params(
    ("target_type" = Option<String>, Query), ("search" = Option<String>, Query), ("limit" = Option<u32>, Query)
), responses((status = 200, body = AuditTargetLookupResponse), (status = 403, body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_audit_targets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AuditTargetLookupQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_audit_collection_access(&state, &request_context).await {
        return handle_app_error(error);
    }
    let limit = query.limit.unwrap_or(25).clamp(1, 50);
    let target_type = normalize_optional_string(query.target_type)
        .and_then(|value| normalize_audit_type(Some(&value)))
        .filter(|value| value != "all");
    let search = normalize_optional_string(query.search);
    match state
        .repositories
        .audit
        .lookup_targets(target_type.as_deref(), search.as_deref(), limit)
        .await
    {
        Ok(items) => (
            StatusCode::OK,
            Json(AuditTargetLookupResponse { items, limit }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List audit events. Free-text search includes validated recorded display labels, not arbitrary metadata.
#[utoipa::path(get, path = "/api/v1/audit/events", tag = "audit", params(
    ("q" = Option<String>, Query, description = "Search event type, target type, event/actor/target IDs and validated recorded actor/target display labels; other metadata is not searched"),
    ("event_type" = Option<String>, Query), ("actor_kind" = Option<String>, Query), ("actor_id" = Option<String>, Query),
    ("target_type" = Option<String>, Query), ("target_id" = Option<String>, Query),
    ("outcome" = Option<String>, Query), ("state_domain" = Option<String>, Query), ("state_value" = Option<String>, Query),
    ("session_id" = Option<String>, Query), ("request_id" = Option<String>, Query), ("run_id" = Option<String>, Query), ("run_group_id" = Option<String>, Query),
    ("created_from" = Option<String>, Query), ("created_to" = Option<String>, Query), ("limit" = Option<u32>, Query), ("offset" = Option<u32>, Query),
    ("sort" = Option<String>, Query, description = "created_at, event_type, actor, target_type or outcome"), ("order" = Option<String>, Query)
), responses((status = 200, body = AuditEventListResponse), (status = 400, body = crate::api::rest::error_mapping::ApiErrorResponse), (status = 403, body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_audit_events(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AuditEventListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_audit_collection_access(&state, &request_context).await {
        return handle_app_error(error);
    }
    let filter = match map_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let offset = query.offset.unwrap_or(0);
    let service = &state.services.audit_query;
    let events = match service
        .list_events(filter.clone(), Some(limit), Some(offset))
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let total = match service.count_events(filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let labels = match state.repositories.audit.event_labels(&events).await {
        Ok(labels) => labels,
        Err(error) => return handle_app_error(error),
    };
    (
        StatusCode::OK,
        Json(AuditEventListResponse {
            items: events
                .into_iter()
                .map(|event| {
                    let names = labels.get(&event.id).cloned().unwrap_or_default();
                    map_event_with_labels(event, names)
                })
                .collect(),
            total,
            limit,
            offset,
        }),
    )
        .into_response()
}

/// Read one event, authorized against its own ID before lookup.
#[utoipa::path(get, path = "/api/v1/audit/events/{id}", tag = "audit", params(("id" = Uuid, Path)), responses(
    (status = 200, body = AuditEventResponse), (status = 403, body = crate::api::rest::error_mapping::ApiErrorResponse), (status = 404, body = crate::api::rest::error_mapping::ApiErrorResponse)
))]
pub async fn get_audit_event(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_audit_event_access(&state, &request_context, id).await {
        return handle_app_error(error);
    }
    match state.services.audit_query.get_event(id).await {
        Ok(Some(event)) => {
            let labels = match state
                .repositories
                .audit
                .event_labels(std::slice::from_ref(&event))
                .await
            {
                Ok(labels) => labels,
                Err(error) => return handle_app_error(error),
            };
            let names = labels.get(&event.id).cloned().unwrap_or_default();
            (StatusCode::OK, Json(map_event_with_labels(event, names))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Audit event not found".into())),
        Err(error) => handle_app_error(error),
    }
}

fn map_event_with_labels(
    event: AuditEvent,
    labels: (Option<String>, Option<String>),
) -> AuditEventResponse {
    let context = project_context(&event);
    let actor = event.actor_id.map(|id| {
        AuditObjectRef::new(
            &event.actor_kind,
            id,
            context
                .display
                .actor
                .clone()
                .or(labels.0)
                .unwrap_or_else(|| id.to_string()),
        )
    });
    let target = event.target_id.map(|id| {
        AuditObjectRef::new(
            event.target_type.as_deref().unwrap_or("target"),
            id,
            context
                .display
                .target
                .clone()
                .or(labels.1)
                .unwrap_or_else(|| id.to_string()),
        )
    });
    let metadata = project_metadata(&event.event_type, &event.metadata);
    AuditEventResponse {
        id: event.id.to_string(),
        event_type: event.event_type,
        actor_id: event.actor_id.map(|id| id.to_string()),
        actor_kind: event.actor_kind,
        target_type: event.target_type,
        target_id: event.target_id.map(|id| id.to_string()),
        actor,
        target,
        context,
        metadata,
        created_at: event.created_at.to_rfc3339(),
    }
}

fn enum_values(raw: Option<&str>, name: &str, allowed: &[String]) -> Result<Vec<String>, AppError> {
    let values = parse_csv_strings(raw);
    if values.iter().any(|value| !allowed.contains(value)) {
        return Err(AppError::Validation(format!("Invalid {name}")));
    }
    Ok(values)
}
fn map_filter(query: &AuditEventListQuery) -> Result<AuditEventFilter, AppError> {
    let states = serialized_lifecycle_states();
    let domains = states
        .iter()
        .map(|(domain, _)| domain.clone())
        .collect::<Vec<_>>();
    let state_values = states
        .iter()
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    Ok(AuditEventFilter {
        query: normalize_optional_string(query.q.clone()),
        event_type: parse_csv_strings(query.event_type.as_deref()),
        actor_kind: parse_csv_strings(query.actor_kind.as_deref()),
        actor_id: parse_csv_uuids(query.actor_id.as_deref(), "actor_id")?,
        target_type: parse_csv_strings(query.target_type.as_deref()),
        target_id: parse_csv_uuids(query.target_id.as_deref(), "target_id")?,
        outcome: enum_values(
            query.outcome.as_deref(),
            "outcome",
            &serialized_values::<AuditOutcome>(),
        )?,
        state_domain: enum_values(query.state_domain.as_deref(), "state_domain", &domains)?,
        state_value: enum_values(query.state_value.as_deref(), "state_value", &state_values)?,
        session_id: parse_csv_uuids(query.session_id.as_deref(), "session_id")?,
        request_id: parse_csv_uuids(query.request_id.as_deref(), "request_id")?,
        run_id: parse_csv_uuids(query.run_id.as_deref(), "run_id")?,
        run_group_id: parse_csv_uuids(query.run_group_id.as_deref(), "run_group_id")?,
        created_from: parse_optional_datetime(query.created_from.as_deref(), "created_from")?,
        created_to: parse_optional_datetime(query.created_to.as_deref(), "created_to")?,
        sort: SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            AUDIT_EVENT_COLUMNS,
        )?,
    })
}
fn parse_optional_datetime(
    value: Option<&str>,
    name: &str,
) -> Result<Option<DateTime<Utc>>, AppError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|date| date.with_timezone(&Utc))
                .map_err(|_| AppError::Validation(format!("Invalid {name}")))
        })
        .transpose()
}
fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}
