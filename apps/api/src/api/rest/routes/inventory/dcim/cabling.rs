//! Cabling workstation REST routes.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::Query;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::infrastructure::persistence::{
    CableEndpointKind, CablingEndpointFilter, CablingEndpointItem, CablingLinkState,
    CablingRepository, CablingTrace, CablingTraceState,
};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access};

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CablingEndpointsQuery {
    /// Free-text search over endpoint, parent, and cable labels. Minimum 2 chars unless structured filters are present.
    pub query: Option<String>,
    /// CSV of endpoint kinds: interface, front-port, rear-port, console-port, console-server-port, power-port, power-outlet, power-feed, circuit-termination.
    pub endpoint_kind: Option<String>,
    /// CSV of site UUIDs.
    pub site_id: Option<String>,
    /// CSV of rack UUIDs.
    pub rack_id: Option<String>,
    /// CSV of device UUIDs.
    pub device_id: Option<String>,
    /// CSV of link states: available, cabled, marked-connected, unknown.
    pub link_state: Option<String>,
    /// CSV of trace states: complete, incomplete, split, invalid, position-conflict, unsupported.
    pub trace_state: Option<String>,
    pub issues_only: Option<bool>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CablingTraceQuery {
    pub endpoint_kind: String,
    pub endpoint_id: String,
    #[param(minimum = 1, maximum = 100)]
    pub max_hops: Option<u16>,
}

/// Search physical cabling endpoints.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/cabling/endpoints",
    tag = "inventory",
    params(CablingEndpointsQuery),
    responses(
        (status = 200, description = "Cabling endpoints", body = InventoryListEnvelope<CablingEndpointItem>),
        (status = 400, description = "Invalid cabling search query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_cabling_endpoints(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CablingEndpointsQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let filter = match parse_endpoint_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return handle_app_error(error),
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 100);

    match state
        .repositories
        .cabling
        .search_endpoints(filter, Some(limit))
        .await
    {
        Ok(items) => {
            let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit,
                    offset: 0,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Trace a physical cabling endpoint.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/cabling/trace",
    tag = "inventory",
    params(CablingTraceQuery),
    responses(
        (status = 200, description = "Cabling trace", body = CablingTrace),
        (status = 400, description = "Invalid cabling trace query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Endpoint not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn trace_cabling_endpoint(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CablingTraceQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let kind = match CableEndpointKind::parse(&query.endpoint_kind) {
        Some(kind) => kind,
        None => {
            return handle_app_error(AppError::Validation("invalid endpoint_kind".to_string()));
        }
    };
    let id = match Uuid::parse_str(query.endpoint_id.trim()) {
        Ok(id) => id,
        Err(_) => return handle_app_error(AppError::Validation("invalid endpoint_id".to_string())),
    };
    let max_hops = query.max_hops.unwrap_or(50);
    if !(1..=100).contains(&max_hops) {
        return handle_app_error(AppError::Validation(
            "max_hops must be between 1 and 100".to_string(),
        ));
    }

    match state
        .repositories
        .cabling
        .trace_endpoint(kind, id, max_hops)
        .await
    {
        Ok(Some(trace)) => (StatusCode::OK, Json(trace)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Endpoint not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

fn parse_endpoint_filter(query: &CablingEndpointsQuery) -> Result<CablingEndpointFilter, AppError> {
    let endpoint_kind = parse_csv_enum(
        query.endpoint_kind.as_deref(),
        CableEndpointKind::parse,
        "endpoint_kind",
    )?;
    let site_id = parse_csv_uuids(query.site_id.as_deref(), "site_id")?;
    let rack_id = parse_csv_uuids(query.rack_id.as_deref(), "rack_id")?;
    let device_id = parse_csv_uuids(query.device_id.as_deref(), "device_id")?;
    let link_state = parse_csv_enum(
        query.link_state.as_deref(),
        CablingLinkState::parse,
        "link_state",
    )?;
    let trace_state = parse_csv_enum(
        query.trace_state.as_deref(),
        CablingTraceState::parse,
        "trace_state",
    )?;
    let query_text = query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let has_structured_filter = !endpoint_kind.is_empty()
        || !site_id.is_empty()
        || !rack_id.is_empty()
        || !device_id.is_empty()
        || !link_state.is_empty()
        || !trace_state.is_empty()
        || query.issues_only.unwrap_or(false);
    if let Some(text) = query_text
        && text.chars().count() < 2
        && !has_structured_filter
    {
        return Err(AppError::Validation(
            "query must be at least 2 characters unless structured filters are present".to_string(),
        ));
    }
    if query_text.is_none() && !has_structured_filter {
        return Err(AppError::Validation(
            "query or at least one structured filter is required".to_string(),
        ));
    }

    Ok(CablingEndpointFilter {
        query: query_text.map(ToOwned::to_owned),
        endpoint_kind,
        site_id,
        rack_id,
        device_id,
        link_state,
        trace_state,
        issues_only: query.issues_only.unwrap_or(false),
    })
}

fn parse_csv_enum<T>(
    raw: Option<&str>,
    parse: impl Fn(&str) -> Option<T>,
    label: &str,
) -> Result<Vec<T>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            parse(value).ok_or_else(|| AppError::Validation(format!("invalid {label}: {value}")))
        })
        .collect()
}

fn parse_csv_uuids(raw: Option<&str>, label: &str) -> Result<Vec<Uuid>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Uuid::parse_str(value)
                .map_err(|_| AppError::Validation(format!("invalid {label}: {value}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_unstructured_query() {
        let error = parse_endpoint_filter(&CablingEndpointsQuery {
            query: Some("a".to_string()),
            endpoint_kind: None,
            site_id: None,
            rack_id: None,
            device_id: None,
            link_state: None,
            trace_state: None,
            issues_only: None,
            limit: None,
        })
        .expect_err("short query rejected");
        assert!(error.to_string().contains("at least 2"));
    }

    #[test]
    fn accepts_short_query_with_structured_filter() {
        let filter = parse_endpoint_filter(&CablingEndpointsQuery {
            query: Some("a".to_string()),
            endpoint_kind: Some("interface,front-port".to_string()),
            site_id: None,
            rack_id: None,
            device_id: None,
            link_state: None,
            trace_state: None,
            issues_only: None,
            limit: None,
        })
        .expect("structured filter permits short query");
        assert_eq!(filter.endpoint_kind.len(), 2);
    }

    #[test]
    fn rejects_invalid_enum_and_uuid() {
        assert!(
            parse_endpoint_filter(&CablingEndpointsQuery {
                query: None,
                endpoint_kind: Some("cable_end".to_string()),
                site_id: None,
                rack_id: None,
                device_id: None,
                link_state: None,
                trace_state: None,
                issues_only: None,
                limit: None,
            })
            .is_err()
        );
        assert!(parse_csv_uuids(Some("not-a-uuid"), "site_id").is_err());
    }
}
