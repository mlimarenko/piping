//! REST handlers for `/api/v1/webhooks` — outbound webhook subscription management.
//!
//! All validation and SQL live behind `WebhookSubscriptionService`; this module
//! only parses requests, delegates, and shapes JSON responses.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::contexts::sync::infrastructure::persistence::repositories::webhook_subscription_repository::{
    CreateWebhookSubscription, UpdateWebhookSubscription, WEBHOOK_SUBSCRIPTION_COLUMNS,
    WebhookSubscriptionListFilter,
};
use crate::errors::AppError;
use crate::api::rest::routes::validation::parse_uuid;

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookSubscriptionResponse {
    pub id: String,
    pub name: String,
    pub target_url: String,
    /// Whether a signing secret is configured for this webhook.
    pub secret_set: bool,
    pub event_types: Vec<String>,
    pub enabled: bool,
    pub conditions: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
    pub last_fired_at: Option<String>,
    pub last_status: Option<i32>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookSubscriptionListResponse {
    pub items: Vec<WebhookSubscriptionResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct WebhookSubscriptionListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Comma-separated enabled flags (`true`/`false`); matches any (`IN`).
    pub enabled: Option<String>,
    /// Comma-separated secret flags (`true`/`false`); matches any (`IN`).
    pub secret_set: Option<String>,
    /// Comma-separated event type values; matches subscriptions containing any selected type.
    pub event_types: Option<String>,
    /// Comma-separated HTTP status codes; matches any non-null last status.
    pub last_status: Option<String>,
    /// Case-insensitive search by name, URL, event type, or last error.
    pub search: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateWebhookSubscriptionRequest {
    pub name: String,
    pub target_url: String,
    pub secret: Option<String>,
    pub event_types: Vec<String>,
    pub enabled: Option<bool>,
    pub conditions: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateWebhookSubscriptionRequest {
    pub name: Option<String>,
    pub target_url: Option<String>,
    /// Provide a non-empty string to set a new secret.
    /// Omit the field (or send `null` / empty string) to leave the current secret unchanged.
    pub secret: Option<String>,
    pub event_types: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub conditions: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TestFireResponse {
    pub status: u16,
    pub ok: bool,
    pub error: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookEventCatalogItemResponse {
    /// Actual emitted `event_type` string to subscribe to.
    pub value: String,
    /// Domain object the event concerns.
    pub object: String,
    /// Action performed on the object.
    pub action: String,
    /// High-level grouping for UI organization.
    pub group: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookEventCatalogResponse {
    pub items: Vec<WebhookEventCatalogItemResponse>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

use crate::contexts::sync::infrastructure::persistence::repositories::webhook_subscription_repository::WebhookSubscription;

fn to_response(s: WebhookSubscription) -> WebhookSubscriptionResponse {
    WebhookSubscriptionResponse {
        id: s.id.to_string(),
        name: s.name,
        target_url: s.target_url,
        secret_set: s.secret.is_some(),
        event_types: s.event_types,
        enabled: s.enabled,
        conditions: s.conditions,
        created_at: s.created_at.to_rfc3339(),
        updated_at: s.updated_at.to_rfc3339(),
        last_fired_at: s.last_fired_at.map(|t| t.to_rfc3339()),
        last_status: s.last_status,
        last_error: s.last_error,
    }
}

async fn ensure_webhook_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    let policy_service = build_policy_evaluation_service(state);
    policy_service
        .ensure_access(request_context, action, ResourceType::Webhook, None)
        .await
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// List webhook subscriptions.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks",
    tag = "webhook-subscriptions",
    params(WebhookSubscriptionListQuery),
    responses(
        (status = 200, description = "List of webhook subscriptions", body = WebhookSubscriptionListResponse),
        (status = 400, description = "Invalid filter", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_webhook_subscriptions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<WebhookSubscriptionListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Read).await {
        return handle_app_error(e);
    }

    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let enabled = parse_csv_strings(query.enabled.as_deref());
    let secret_set = parse_csv_strings(query.secret_set.as_deref());
    let event_types = parse_csv_strings(query.event_types.as_deref());
    let last_status = parse_csv_strings(query.last_status.as_deref());
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        WEBHOOK_SUBSCRIPTION_COLUMNS,
    ) {
        Ok(value) => value,
        Err(e) => return handle_app_error(e),
    };
    let filter = WebhookSubscriptionListFilter {
        enabled,
        secret_set,
        event_types,
        last_status,
        search,
        sort,
    };

    match state
        .services
        .webhook_subscriptions
        .list(&filter, limit, offset)
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(WebhookSubscriptionListResponse {
                items: items.into_iter().map(to_response).collect(),
                total,
                limit,
                offset,
            }),
        )
            .into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Create a webhook subscription.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks",
    tag = "webhook-subscriptions",
    request_body = CreateWebhookSubscriptionRequest,
    responses(
        (status = 201, description = "Webhook subscription created", body = WebhookSubscriptionResponse),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 500, description = "Internal server error", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_webhook_subscription(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateWebhookSubscriptionRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Create).await {
        return handle_app_error(e);
    }

    if let Err(e) = crate::contexts::sync::application::WebhookSubscriptionService::validate_create(
        &payload.name,
        &payload.target_url,
        payload.secret.as_deref(),
        &payload.event_types,
        payload.conditions.as_ref(),
    ) {
        return handle_app_error(e);
    }

    let input = CreateWebhookSubscription {
        name: payload.name,
        target_url: payload.target_url,
        secret: payload.secret,
        event_types: payload.event_types,
        enabled: payload.enabled.unwrap_or(true),
        conditions: payload.conditions,
    };

    match state.services.webhook_subscriptions.create(input).await {
        Ok(sub) => (StatusCode::CREATED, Json(to_response(sub))).into_response(),
        Err(AppError::DatabaseSource(ref e))
            if e.as_database_error()
                .map(|db| db.is_unique_violation())
                .unwrap_or(false) =>
        {
            handle_app_error(AppError::Validation(
                "webhook name already exists".to_string(),
            ))
        }
        Err(e) => handle_app_error(e),
    }
}

/// Get a webhook subscription.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks/{id}",
    tag = "webhook-subscriptions",
    params(
        ("id" = String, Path, description = "Webhook subscription id"),
    ),
    responses(
        (status = 200, description = "Webhook subscription", body = WebhookSubscriptionResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_webhook_subscription(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Read).await {
        return handle_app_error(e);
    }

    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.services.webhook_subscriptions.get(id).await {
        Ok(Some(sub)) => (StatusCode::OK, Json(to_response(sub))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "webhook subscription '{id}' not found"
        ))),
        Err(e) => handle_app_error(e),
    }
}

/// Update a webhook subscription.
#[utoipa::path(
    put,
    path = "/api/v1/webhooks/{id}",
    tag = "webhook-subscriptions",
    params(
        ("id" = String, Path, description = "Webhook subscription id"),
    ),
    request_body = UpdateWebhookSubscriptionRequest,
    responses(
        (status = 200, description = "Updated webhook subscription", body = WebhookSubscriptionResponse),
        (status = 400, description = "Validation error", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_webhook_subscription(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UpdateWebhookSubscriptionRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Update).await {
        return handle_app_error(e);
    }

    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    // Treat empty string as "no change" — only non-empty string sets a new secret.
    let secret_for_validation = payload.secret.as_deref().filter(|s| !s.is_empty());

    if let Err(e) = crate::contexts::sync::application::WebhookSubscriptionService::validate_update(
        payload.name.as_deref(),
        payload.target_url.as_deref(),
        secret_for_validation.map(Some),
        payload.event_types.as_deref(),
        payload.conditions.as_ref().map(Some),
    ) {
        return handle_app_error(e);
    }

    // Convert: empty/absent string → None (keep existing); non-empty → Some(value).
    let secret_update: Option<Option<String>> = match &payload.secret {
        Some(s) if !s.is_empty() => Some(Some(s.clone())),
        _ => None,
    };

    let input = UpdateWebhookSubscription {
        name: payload.name,
        target_url: payload.target_url,
        secret: secret_update,
        event_types: payload.event_types,
        enabled: payload.enabled,
        conditions: payload.conditions.map(Some),
    };

    match state.services.webhook_subscriptions.update(id, input).await {
        Ok(Some(sub)) => (StatusCode::OK, Json(to_response(sub))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "webhook subscription '{id}' not found"
        ))),
        Err(e) => handle_app_error(e),
    }
}

/// Delete a webhook subscription.
#[utoipa::path(
    delete,
    path = "/api/v1/webhooks/{id}",
    tag = "webhook-subscriptions",
    params(
        ("id" = String, Path, description = "Webhook subscription id"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_webhook_subscription(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Delete).await {
        return handle_app_error(e);
    }

    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.services.webhook_subscriptions.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "webhook subscription '{id}' not found"
        ))),
        Err(e) => handle_app_error(e),
    }
}

/// Send a test event to a webhook subscription.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/{id}/test-fire",
    tag = "webhook-subscriptions",
    params(
        ("id" = String, Path, description = "Webhook subscription id"),
    ),
    responses(
        (status = 200, description = "Test fire result", body = TestFireResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn test_fire_webhook_subscription(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Update).await {
        return handle_app_error(e);
    }

    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.services.webhook_subscriptions.test_fire(id).await {
        Ok(Some(result)) => (
            StatusCode::OK,
            Json(TestFireResponse {
                status: result.status,
                ok: result.ok,
                error: result.error,
                latency_ms: result.latency_ms,
            }),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "webhook subscription '{id}' not found"
        ))),
        Err(e) => handle_app_error(e),
    }
}

/// Clear a webhook subscription's signing secret.
#[utoipa::path(
    delete,
    path = "/api/v1/webhooks/{id}/secret",
    tag = "webhook-subscriptions",
    params(
        ("id" = String, Path, description = "Webhook subscription id"),
    ),
    responses(
        (status = 204, description = "Secret cleared"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn clear_webhook_secret(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Update).await {
        return handle_app_error(e);
    }

    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.services.webhook_subscriptions.clear_secret(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "webhook subscription '{id}' not found"
        ))),
        Err(e) => handle_app_error(e),
    }
}

/// List the curated catalog of webhook-worthy domain events.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks/event-catalog",
    tag = "webhook-subscriptions",
    responses(
        (status = 200, description = "Webhook event catalog", body = WebhookEventCatalogResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_webhook_event_catalog(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_webhook_access(&state, &request_context, Action::Read).await {
        return handle_app_error(e);
    }

    let items = state
        .services
        .webhook_subscriptions
        .event_catalog()
        .iter()
        .map(|item| WebhookEventCatalogItemResponse {
            value: item.value.to_string(),
            object: item.object.to_string(),
            action: item.action.to_string(),
            group: item.group.to_string(),
        })
        .collect();

    (StatusCode::OK, Json(WebhookEventCatalogResponse { items })).into_response()
}
