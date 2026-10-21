//! Monitoring query sources routes, split out of `monitoring.rs`.
//!
//! Moved verbatim; behaviour and the OpenAPI annotations are unchanged.

use super::*;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::app::AppState;
use crate::errors::AppError;

use super::helpers::normalize_string;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::contexts::access_control::domain::Action;

use crate::contexts::monitoring::application::{
    redacted_url, validate_monitoring_query_source_url,
};

// --- query sources ---------------------------------------------------------

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QuerySourceItem {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub source_type: String,
    /// Endpoint-only URL. Query sources reject URL userinfo, query parameters,
    /// and fragments; the value is returned by this API.
    pub query_url: String,
    /// Nullable canonical UUID of the named secret bound as this source's
    /// bearer token.
    #[schema(pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")]
    pub credential_ref: Option<Uuid>,
    pub timeout_ms: i32,
    pub options: Value,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QuerySourceListResponse {
    pub items: Vec<QuerySourceItem>,
    pub total: usize,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct QuerySourceListQuery {
    pub sort: Option<String>,
    pub order: Option<String>,
    pub source_type: Option<String>,
    pub enabled: Option<String>,
}

fn query_source_list_filter(
    query: &QuerySourceListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::QUERY_SOURCE_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("source_type", query.source_type.as_deref()),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QuerySourceRequest {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub source_type: Option<String>,
    /// Endpoint-only URL. URL userinfo, query parameters, and fragments are
    /// rejected; bind credentials through `credential_ref` instead.
    pub query_url: String,
    #[serde(default, deserialize_with = "deserialize_optional_canonical_uuid")]
    #[schema(pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")]
    /// Canonical UUID of an existing named secret used as the source bearer
    /// token.
    pub credential_ref: Option<Uuid>,
    #[serde(default)]
    pub timeout_ms: Option<i32>,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn query_source_item(record: repository::QuerySourceRecord) -> QuerySourceItem {
    QuerySourceItem {
        id: record.id,
        name: record.name,
        slug: record.slug,
        source_type: record.source_type,
        query_url: redacted_url(&record.query_url),
        credential_ref: record.credential_secret_id,
        timeout_ms: record.timeout_ms,
        options: record.options,
        enabled: record.enabled,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

pub(crate) fn validate_query_source(
    payload: QuerySourceRequest,
) -> Result<repository::QuerySourceInput, AppError> {
    let name = normalize_string(Some(&payload.name))
        .ok_or_else(|| AppError::Validation("name must not be empty".to_string()))?;
    let slug = normalize_string(Some(&payload.slug))
        .ok_or_else(|| AppError::Validation("slug must not be empty".to_string()))?;
    let query_url = normalize_string(Some(&payload.query_url))
        .ok_or_else(|| AppError::Validation("query_url must not be empty".to_string()))?;
    let source_type = validate_enum_value(
        "source_type",
        payload.source_type.as_deref().unwrap_or("prometheus"),
        &MONITORING_SOURCE_TYPES,
    )?;

    let timeout_ms = payload.timeout_ms.unwrap_or(3000);
    if timeout_ms < 100 {
        return Err(AppError::Validation(
            "timeout_ms must be at least 100".to_string(),
        ));
    }
    let options = validate_json_object("options", payload.options)?;
    // Create/update source shape check: keep the strict guard (literal
    // private-IP hosts stay rejected; hostnames that resolve privately are
    // permitted and trusted at fetch time via source provenance).
    validate_monitoring_query_source_url(&query_url)?;

    Ok(repository::QuerySourceInput {
        name,
        slug,
        source_type,
        query_url,
        credential_secret_id: payload.credential_ref,
        timeout_ms,
        options,
        enabled: payload.enabled.unwrap_or(true),
    })
}

/// List monitoring query sources.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/query-sources",
    tag = "monitoring",
    params(QuerySourceListQuery),
    responses(
        (status = 200, description = "Monitoring query sources", body = QuerySourceListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_query_sources(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<QuerySourceListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match query_source_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::list_query_sources(&state.db_pool, &filter).await {
        Ok(records) => {
            let items: Vec<QuerySourceItem> = records.into_iter().map(query_source_item).collect();
            (
                StatusCode::OK,
                Json(QuerySourceListResponse {
                    total: items.len(),
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a monitoring query source.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/query-sources",
    tag = "monitoring",
    request_body = QuerySourceRequest,
    responses(
        (status = 201, description = "Query source created", body = QuerySourceItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Monitoring configuration write or query-source credential disclosure is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_query_source(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<QuerySourceRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_query_source(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_query_source_credential_use(&state, &request_context, input.credential_secret_id)
            .await
    {
        return handle_app_error(error);
    }

    match repository::create_query_source(&state.db_pool, &input).await {
        Ok(record) => (StatusCode::CREATED, Json(query_source_item(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a monitoring query source.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/query-sources/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Query source id")),
    request_body = QuerySourceRequest,
    responses(
        (status = 200, description = "Query source updated", body = QuerySourceItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Monitoring configuration write or query-source credential disclosure is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Query source not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_query_source(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<QuerySourceRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_query_source(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        ensure_query_source_credential_use(&state, &request_context, input.credential_secret_id)
            .await
    {
        return handle_app_error(error);
    }

    match repository::update_query_source(&state.db_pool, id, &input).await {
        Ok(Some(record)) => (StatusCode::OK, Json(query_source_item(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Query source not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring query source.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/query-sources/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Query source id")),
    responses(
        (status = 204, description = "Query source deleted"),
        (status = 404, description = "Query source not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_query_source(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_query_source(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Query source not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

// --- query source assignments ----------------------------------------------

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QuerySourceAssignmentItem {
    pub id: Uuid,
    pub query_source_id: Uuid,
    pub query_source_slug: String,
    pub query_source_name: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub options: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QuerySourceAssignmentListResponse {
    pub items: Vec<QuerySourceAssignmentItem>,
    pub total: usize,
}

fn query_source_assignment_list_filter(
    query: &MonitoringScopeListQuery,
) -> Result<repository::MonitoringListFilter, AppError> {
    repository::MonitoringListFilter::from_params(
        repository::QUERY_SOURCE_ASSIGNMENT_COLUMNS,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            ("query_source_id", query.query_source_id.as_deref()),
            ("scope_type", query.scope_type.as_deref()),
            ("scope_id", query.scope_id.as_deref()),
            ("enabled", query.enabled.as_deref()),
        ],
    )
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QuerySourceAssignmentRequest {
    pub query_source_id: Uuid,
    pub scope_type: String,
    #[serde(default)]
    pub scope_id: Option<Uuid>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub options: Option<Value>,
}

fn query_source_assignment_item(
    record: repository::QuerySourceAssignmentRecord,
) -> QuerySourceAssignmentItem {
    QuerySourceAssignmentItem {
        id: record.id,
        query_source_id: record.query_source_id,
        query_source_slug: record.query_source_slug,
        query_source_name: record.query_source_name,
        scope_type: record.scope_type,
        scope_id: record.scope_id,
        weight: record.weight,
        enabled: record.enabled,
        options: record.options,
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn validate_query_source_assignment(
    payload: QuerySourceAssignmentRequest,
) -> Result<repository::QuerySourceAssignmentInput, AppError> {
    let scope_type = validate_monitoring_scope(&payload.scope_type, payload.scope_id)?;
    let options = validate_json_object("options", payload.options)?;
    Ok(repository::QuerySourceAssignmentInput {
        query_source_id: payload.query_source_id,
        scope_type,
        scope_id: payload.scope_id,
        weight: payload.weight.unwrap_or(0),
        enabled: payload.enabled.unwrap_or(true),
        options,
    })
}

/// List monitoring query source assignments.
#[utoipa::path(
    get,
    path = "/api/v1/monitoring/query-source-assignments",
    tag = "monitoring",
    params(MonitoringScopeListQuery),
    responses(
        (status = 200, description = "Query source assignments matching the scope filters", body = QuerySourceAssignmentListResponse),
        (status = "4XX", description = "Request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn list_query_source_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<MonitoringScopeListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_monitoring_config_read(&state, &request_context).await {
        return handle_app_error(error);
    }

    let filter = match query_source_assignment_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::list_query_source_assignments(&state.db_pool, &filter).await {
        Ok(records) => {
            let items: Vec<QuerySourceAssignmentItem> = records
                .into_iter()
                .map(query_source_assignment_item)
                .collect();
            (
                StatusCode::OK,
                Json(QuerySourceAssignmentListResponse {
                    total: items.len(),
                    items,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a monitoring query source assignment.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/query-source-assignments",
    tag = "monitoring",
    request_body = QuerySourceAssignmentRequest,
    responses(
        (status = 201, description = "Query source assignment created", body = QuerySourceAssignmentItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn create_query_source_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<QuerySourceAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let input = match validate_query_source_assignment(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::create_query_source_assignment(&state.db_pool, &input).await {
        Ok(record) => (
            StatusCode::CREATED,
            Json(query_source_assignment_item(record)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a monitoring query source assignment.
#[utoipa::path(
    put,
    path = "/api/v1/monitoring/query-source-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Query source assignment id")),
    request_body = QuerySourceAssignmentRequest,
    responses(
        (status = 200, description = "Query source assignment updated", body = QuerySourceAssignmentItem),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Query source assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn update_query_source_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<QuerySourceAssignmentRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Update).await
    {
        return handle_app_error(error);
    }

    let input = match validate_query_source_assignment(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match repository::update_query_source_assignment(&state.db_pool, id, &input).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(query_source_assignment_item(record))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "Query source assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a monitoring query source assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/monitoring/query-source-assignments/{id}",
    tag = "monitoring",
    params(("id" = Uuid, Path, description = "Query source assignment id")),
    responses(
        (status = 204, description = "Query source assignment deleted"),
        (status = 404, description = "Query source assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    )
)]
pub async fn delete_query_source_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_monitoring_config_write(&state, &request_context, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match repository::delete_query_source_assignment(&state.db_pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(
            "Query source assignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}
