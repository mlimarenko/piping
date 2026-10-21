use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::parse_uuid;
use crate::app::AppState;
use crate::config::{
    DEFAULT_SOURCES_LIST_LIMIT, DEFAULT_SOURCES_LIST_OFFSET, MAX_SOURCES_LIST_LIMIT,
    MIN_SOURCES_LIST_LIMIT,
};
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::request_context::RequestContext;
use crate::contexts::sync::domain::{
    Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE, SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
    SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET, SourceBootstrapState, SourceBootstrapStateStatus,
};
use crate::contexts::sync::infrastructure::{
    PROVIDER_COLUMNS, ProviderListFilter, SourceBootstrapStateRepository,
};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct SourceListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
    #[serde(default)]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub is_enabled: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SourceResponse {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub config: Value,
    pub is_enabled: bool,
    pub secret_fields: Vec<SourceSecretFieldState>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SourceSecretFieldState {
    pub field_key: String,
    pub enabled: bool,
    pub has_value: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SourceListResponse {
    pub items: Vec<SourceResponse>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateSourceRequest {
    pub name: String,
    pub provider_type: String,
    pub config: Value,
    pub is_enabled: Option<bool>,
    pub bootstrap: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSourceRequest {
    pub name: Option<String>,
    pub config: Option<Value>,
    pub is_enabled: Option<bool>,
    pub bootstrap: Option<bool>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SourceBootstrapResponse {
    pub source_id: String,
    pub status: String,
    pub message: String,
    pub external_webhook_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SourceStatusResponse {
    pub source_id: String,
    pub bootstrap_status: String,
    pub operational_status: String,
    pub webhook_last_received_at: Option<String>,
    pub outbox_pending_count: i64,
    pub outbox_failed_count: i64,
    pub last_successful_outbound_at: Option<String>,
    pub last_error: Option<String>,
    pub external_webhook_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReplayOutboxRequest {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReplayOutboxResponse {
    pub source_id: String,
    pub accepted: bool,
    pub replayed_events: usize,
}

/// Bootstrap workflow status for the dedicated `bootstrap/*` endpoints,
/// distinct from the general runtime status in `SourceStatusResponse`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BootstrapStatusResponse {
    pub provider_id: String,
    pub status: String,
    pub attempts: i32,
    pub attempted_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BootstrapLastErrorResponse {
    pub provider_id: String,
    pub last_error: Option<String>,
    pub status: String,
}

/// List sync providers.
#[utoipa::path(
    get,
    path = "/api/v1/providers",
    tag = "providers",
    params(SourceListQuery),
    responses(
        (status = 200, description = "Providers", body = SourceListResponse),
        (status = "4XX", description = "Provider list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_providers(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<SourceListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    let limit = query
        .limit
        .unwrap_or(DEFAULT_SOURCES_LIST_LIMIT)
        .clamp(MIN_SOURCES_LIST_LIMIT, MAX_SOURCES_LIST_LIMIT);
    let offset = query.offset.unwrap_or(DEFAULT_SOURCES_LIST_OFFSET);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PROVIDER_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = match source_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let total = match state
        .services
        .source_service
        .count_sources(filter.clone())
        .await
    {
        Ok(total) => total,
        Err(error) => return handle_app_error(error),
    };
    let paged = match state
        .services
        .source_service
        .list_sources_paginated(limit, offset, sort, filter)
        .await
    {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };
    let mut response_items = Vec::with_capacity(paged.len());
    for provider in paged {
        match source_to_response(&state, provider).await {
            Ok(item) => response_items.push(item),
            Err(error) => return handle_app_error(error),
        }
    }
    let response = SourceListResponse {
        items: response_items,
        total,
        limit,
        offset,
    };
    (StatusCode::OK, Json(response)).into_response()
}

fn source_list_filter(query: &SourceListQuery) -> Result<ProviderListFilter, AppError> {
    let provider_type =
        crate::contexts::shared::list_query::parse_csv_strings(query.provider_type.as_deref());
    let enabled_values =
        crate::contexts::shared::list_query::parse_csv_strings(query.is_enabled.as_deref());
    let mut is_enabled = Vec::with_capacity(enabled_values.len());
    for value in enabled_values {
        match value.as_str() {
            "true" => is_enabled.push(true),
            "false" => is_enabled.push(false),
            _ => {
                return Err(AppError::Validation(
                    "is_enabled filter accepts only true or false".to_string(),
                ));
            }
        }
    }
    Ok(ProviderListFilter {
        provider_type,
        is_enabled,
    })
}

/// Create a sync provider.
#[utoipa::path(
    post,
    path = "/api/v1/providers",
    tag = "providers",
    request_body = CreateSourceRequest,
    responses(
        (status = 201, description = "Provider created", body = SourceResponse),
        (status = "4XX", description = "Provider create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_provider(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateSourceRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }

    let bootstrap = payload.bootstrap.unwrap_or(false);
    let provider_type = match parse_provider_type(&payload.provider_type) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let source = Provider {
        id: Uuid::now_v7(),
        name: payload.name,
        provider_type,
        config: payload.config,
        is_enabled: payload.is_enabled.unwrap_or(true),
    };
    let created = match state.services.source_service.create_source(source).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if bootstrap
        && let Err(error) = state
            .services
            .source_service
            .bootstrap_source(created.id)
            .await
    {
        return handle_app_error(error);
    }

    let response = match source_to_response(&state, created).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    (StatusCode::CREATED, Json(response)).into_response()
}

/// Get a sync provider.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider", body = SourceResponse),
        (status = 404, description = "Provider not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Provider request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_provider(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.services.source_service.get_source(provider_id).await {
        Ok(Some(provider)) => match source_to_response(&state, provider).await {
            Ok(response) => (StatusCode::OK, Json(response)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Provider not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Update a sync provider.
#[utoipa::path(
    patch,
    path = "/api/v1/providers/{id}",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    request_body = UpdateSourceRequest,
    responses(
        (status = 200, description = "Provider updated", body = SourceResponse),
        (status = 404, description = "Provider not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Provider update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_provider(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateSourceRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }

    let UpdateSourceRequest {
        name,
        config,
        is_enabled,
        bootstrap,
    } = payload;

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let current = match state.services.source_service.get_source(provider_id).await {
        Ok(Some(value)) => value,
        Ok(None) => return handle_app_error(AppError::NotFound("Provider not found".to_string())),
        Err(error) => return handle_app_error(error),
    };

    let updated = Provider {
        id: current.id,
        name: name.unwrap_or(current.name),
        provider_type: current.provider_type,
        config: match merge_provider_config(current.config, config) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
        is_enabled: is_enabled.unwrap_or(current.is_enabled),
    };

    let updated = match state.services.source_service.update_source(updated).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if bootstrap.unwrap_or(false)
        && let Err(error) = state
            .services
            .source_service
            .bootstrap_source(updated.id)
            .await
    {
        return handle_app_error(error);
    }

    let response = match source_to_response(&state, updated).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    (StatusCode::OK, Json(response)).into_response()
}

/// Delete a sync provider.
#[utoipa::path(
    delete,
    path = "/api/v1/providers/{id}",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 204, description = "Provider deleted"),
        (status = "4XX", description = "Provider delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_provider(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Delete).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .services
        .source_service
        .delete_source(provider_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bootstrap a sync provider.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/bootstrap",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider bootstrap completed", body = SourceBootstrapResponse),
        (status = "4XX", description = "Provider bootstrap request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider bootstrap request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bootstrap_provider(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Execute).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .services
        .source_service
        .bootstrap_source(provider_id)
        .await
    {
        Ok(runtime) => (
            StatusCode::OK,
            Json(SourceBootstrapResponse {
                source_id: provider_id.to_string(),
                status: format!("{:?}", runtime.bootstrap_status).to_lowercase(),
                message: "bootstrap completed".to_string(),
                external_webhook_id: runtime.external_webhook_id,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a sync provider's status.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/status",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider runtime status", body = SourceStatusResponse),
        (status = 404, description = "Provider runtime status not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Provider status request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider status request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_provider_status(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .services
        .source_service
        .get_runtime_status(provider_id)
        .await
    {
        Ok(Some(runtime)) => {
            let counters = match state
                .services
                .outbound_service
                .get_counters(provider_id)
                .await
            {
                Ok(counters) => counters,
                Err(error) => return handle_app_error(error),
            };
            (
                StatusCode::OK,
                Json(SourceStatusResponse {
                    source_id: provider_id.to_string(),
                    bootstrap_status: format!("{:?}", runtime.bootstrap_status).to_lowercase(),
                    operational_status: format!("{:?}", runtime.operational_status).to_lowercase(),
                    webhook_last_received_at: runtime
                        .webhook_last_received_at
                        .map(|v| v.to_rfc3339()),
                    outbox_pending_count: counters.pending_count,
                    outbox_failed_count: counters.failed_count,
                    last_successful_outbound_at: counters
                        .last_successful_outbound_at
                        .map(|v| v.to_rfc3339()),
                    last_error: runtime.last_error_message,
                    external_webhook_id: runtime.external_webhook_id,
                }),
            )
                .into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Source runtime not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Replay a provider's outbox events.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/outbox/replay",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    request_body = ReplayOutboxRequest,
    responses(
        (status = 202, description = "Provider outbox replay accepted", body = ReplayOutboxResponse),
        (status = "4XX", description = "Provider outbox replay request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider outbox replay request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn replay_provider_outbox(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ReplayOutboxRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Execute).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .services
        .source_service
        .replay_outbox(provider_id, payload.limit)
        .await
    {
        Ok(replayed_events) => (
            StatusCode::ACCEPTED,
            Json(ReplayOutboxResponse {
                source_id: provider_id.to_string(),
                accepted: true,
                replayed_events,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get the bootstrap status for a provider.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/bootstrap/status",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider bootstrap status", body = BootstrapStatusResponse),
        (status = "4XX", description = "Provider bootstrap status request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider bootstrap status request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_provider_bootstrap_status(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let bootstrap = match state
        .repositories
        .sync_source_bootstrap_states
        .get(provider_id)
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let bootstrap = bootstrap.unwrap_or_else(|| SourceBootstrapState {
        provider_id,
        status: SourceBootstrapStateStatus::NotStarted,
        attempts: 0,
        attempted_at: None,
        completed_at: None,
        last_error: None,
        updated_at: chrono::Utc::now(),
    });

    (
        StatusCode::OK,
        Json(BootstrapStatusResponse {
            provider_id: provider_id.to_string(),
            status: bootstrap.status.as_str().to_string(),
            attempts: bootstrap.attempts,
            attempted_at: bootstrap.attempted_at.map(|v| v.to_rfc3339()),
            completed_at: bootstrap.completed_at.map(|v| v.to_rfc3339()),
            last_error: bootstrap.last_error,
        }),
    )
        .into_response()
}

/// Replay the bootstrap for a provider.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/bootstrap/replay",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider bootstrap replay completed", body = SourceBootstrapResponse),
        (status = "4XX", description = "Provider bootstrap replay request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider bootstrap replay request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn replay_provider_bootstrap(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Execute).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    // Mark in_progress before invoking; outcome (Complete/Failed) recorded
    // by source_service. Existing call increments the attempt counter.
    let now = chrono::Utc::now();
    let attempts = match state
        .repositories
        .sync_source_bootstrap_states
        .get(provider_id)
        .await
    {
        Ok(Some(prev)) => prev.attempts.saturating_add(1),
        Ok(None) => 1,
        Err(error) => return handle_app_error(error),
    };
    let in_progress = SourceBootstrapState {
        provider_id,
        status: SourceBootstrapStateStatus::InProgress,
        attempts,
        attempted_at: Some(now),
        completed_at: None,
        last_error: None,
        updated_at: now,
    };
    if let Err(error) = state
        .repositories
        .sync_source_bootstrap_states
        .upsert(&in_progress)
        .await
    {
        return handle_app_error(error);
    }

    match state
        .services
        .source_service
        .bootstrap_source(provider_id)
        .await
    {
        Ok(runtime) => {
            let completed_at = chrono::Utc::now();
            let final_state = SourceBootstrapState {
                provider_id,
                status: SourceBootstrapStateStatus::Complete,
                attempts,
                attempted_at: Some(now),
                completed_at: Some(completed_at),
                last_error: None,
                updated_at: completed_at,
            };
            let _ = state
                .repositories
                .sync_source_bootstrap_states
                .upsert(&final_state)
                .await;
            (
                StatusCode::OK,
                Json(SourceBootstrapResponse {
                    source_id: provider_id.to_string(),
                    status: format!("{:?}", runtime.bootstrap_status).to_lowercase(),
                    message: "bootstrap replayed".to_string(),
                    external_webhook_id: runtime.external_webhook_id,
                }),
            )
                .into_response()
        }
        Err(error) => {
            let completed_at = chrono::Utc::now();
            let failed_state = SourceBootstrapState {
                provider_id,
                status: SourceBootstrapStateStatus::Failed,
                attempts,
                attempted_at: Some(now),
                completed_at: Some(completed_at),
                last_error: Some(error.to_string()),
                updated_at: completed_at,
            };
            let _ = state
                .repositories
                .sync_source_bootstrap_states
                .upsert(&failed_state)
                .await;
            handle_app_error(error)
        }
    }
}

/// Get the last bootstrap error for a provider.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/bootstrap/last-error",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider bootstrap last error", body = BootstrapLastErrorResponse),
        (status = "4XX", description = "Provider bootstrap last-error request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider bootstrap last-error request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_provider_bootstrap_last_error(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let bootstrap = match state
        .repositories
        .sync_source_bootstrap_states
        .get(provider_id)
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let bootstrap = bootstrap.unwrap_or_else(|| SourceBootstrapState {
        provider_id,
        status: SourceBootstrapStateStatus::NotStarted,
        attempts: 0,
        attempted_at: None,
        completed_at: None,
        last_error: None,
        updated_at: chrono::Utc::now(),
    });
    (
        StatusCode::OK,
        Json(BootstrapLastErrorResponse {
            provider_id: provider_id.to_string(),
            last_error: bootstrap.last_error,
            status: bootstrap.status.as_str().to_string(),
        }),
    )
        .into_response()
}

async fn ensure_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    crate::api::permission_helpers::ensure_access(
        state,
        request_context,
        action,
        ResourceType::Inventory,
    )
    .await
}

async fn source_to_response(
    state: &AppState,
    provider: Provider,
) -> Result<SourceResponse, AppError> {
    let secret_state_entries = state
        .services
        .secret_field_states
        .list_for_consumer(SYNC_PROVIDER_CONSUMER_TYPE, provider.id)
        .await?;

    let mut secret_fields = Vec::with_capacity(2);
    for field_key in [
        SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
        SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET,
    ] {
        let state_entry = secret_state_entries
            .iter()
            .find(|entry| entry.field_key == field_key);
        secret_fields.push(SourceSecretFieldState {
            field_key: field_key.to_string(),
            enabled: state_entry.is_some_and(|entry| entry.enabled),
            has_value: state_entry.is_some_and(|entry| entry.has_value),
        });
    }

    Ok(SourceResponse {
        id: provider.id.to_string(),
        name: provider.name,
        provider_type: match provider.provider_type {
            ProviderType::Netbox => "netbox".to_string(),
            ProviderType::Custom => "custom".to_string(),
        },
        config: sanitize_provider_config(provider.config),
        is_enabled: provider.is_enabled,
        secret_fields,
    })
}

fn sanitize_provider_config(config: Value) -> Value {
    let Value::Object(mut map) = config else {
        return config;
    };
    map.remove(SYNC_PROVIDER_FIELD_KEY_API_TOKEN);
    map.remove(SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET);
    Value::Object(map)
}

fn merge_provider_config(current: Value, patch: Option<Value>) -> Result<Value, AppError> {
    let Some(patch) = patch else {
        return Ok(current);
    };

    let Value::Object(mut current_map) = current else {
        return Err(AppError::Validation(
            "Provider config must be a JSON object".to_string(),
        ));
    };
    let Value::Object(patch_map) = patch else {
        return Err(AppError::Validation(
            "Provider config patch must be a JSON object".to_string(),
        ));
    };

    for (key, value) in patch_map {
        current_map.insert(key, value);
    }

    Ok(Value::Object(current_map))
}

fn parse_provider_type(value: &str) -> Result<ProviderType, AppError> {
    match value {
        "netbox" => Ok(ProviderType::Netbox),
        "custom" => Ok(ProviderType::Custom),
        other => Err(AppError::Validation(format!(
            "Unknown provider_type '{}'",
            other
        ))),
    }
}
