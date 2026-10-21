//! REST handlers for `/api/v1/inventory/layouts/*`.
//!
//! All SQL and business logic live behind `LayoutService`; this module only
//! parses requests, delegates, and shapes JSON responses.

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
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::application::{EffectiveUserLayout, LayoutProfile};
use crate::errors::AppError;

use super::super::{ensure_access, parse_required_string};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LayoutPayload {
    pub entity_type: Option<String>,
    pub layout: Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LayoutProfileResponse {
    pub id: Uuid,
    pub entity_type: String,
    pub layout: Value,
    pub version: i32,
    pub is_active: bool,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EffectiveUserLayoutResponse {
    pub entity_type: String,
    pub source: String,
    pub effective_layout: Value,
    pub system_layout: Option<Value>,
    pub user_layout: Option<Value>,
}

fn map_profile(p: LayoutProfile) -> LayoutProfileResponse {
    LayoutProfileResponse {
        id: p.id,
        entity_type: p.entity_type,
        layout: p.layout,
        version: p.version,
        is_active: p.is_active,
        updated_by_user_id: p.updated_by_user_id,
        created_at: p.created_at,
        updated_at: p.updated_at,
    }
}

fn map_effective(e: EffectiveUserLayout) -> EffectiveUserLayoutResponse {
    EffectiveUserLayoutResponse {
        entity_type: e.entity_type,
        source: e.source,
        effective_layout: e.effective_layout,
        system_layout: e.system_layout,
        user_layout: e.user_layout,
    }
}

/// List system layouts.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/layouts/system",
    tag = "inventory",
    responses(
        (status = 200, description = "System layouts", body = [LayoutProfileResponse]),
        (status = "4XX", description = "System layout list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "System layout list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_system_layouts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
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

    match state.services.inventory_layouts.list_system_layouts().await {
        Ok(layouts) => {
            let body: Vec<LayoutProfileResponse> = layouts.into_iter().map(map_profile).collect();
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create or update a system layout.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/layouts/system",
    tag = "inventory",
    request_body = LayoutPayload,
    responses(
        (status = 200, description = "System layout upserted", body = LayoutProfileResponse),
        (status = "4XX", description = "System layout upsert request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "System layout upsert failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn upsert_system_layout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<LayoutPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let entity_type_raw = payload.entity_type.unwrap_or_default();
    let entity_type = match parse_required_string(&entity_type_raw, "entity_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let actor_id = request_context.require_authenticated_user_id().ok();

    match state
        .services
        .inventory_layouts
        .upsert_system_layout(&entity_type, &payload.layout, actor_id)
        .await
    {
        Ok(profile) => (StatusCode::OK, Json(map_profile(profile))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a system layout.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/layouts/system/{entity_type}",
    tag = "inventory",
    params(
        ("entity_type" = String, Path, description = "Inventory entity type"),
    ),
    responses(
        (status = 200, description = "System layout", body = LayoutProfileResponse),
        (status = 404, description = "System layout not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "System layout get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "System layout get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_system_layout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
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

    let entity_type = match parse_required_string(&entity_type, "entity_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .inventory_layouts
        .get_system_layout(&entity_type)
        .await
    {
        Ok(Some(profile)) => (StatusCode::OK, Json(map_profile(profile))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("System layout not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Get the current user's saved layout.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/layouts/user/{entity_type}",
    tag = "inventory",
    params(
        ("entity_type" = String, Path, description = "Inventory entity type"),
    ),
    responses(
        (status = 200, description = "Effective user layout", body = EffectiveUserLayoutResponse),
        (status = "4XX", description = "User layout get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "User layout get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_user_layout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
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

    let entity_type = match parse_required_string(&entity_type, "entity_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .inventory_layouts
        .get_effective_user_layout(user_id, &entity_type)
        .await
    {
        Ok(effective) => (StatusCode::OK, Json(map_effective(effective))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create or update the current user's layout.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/layouts/user/{entity_type}",
    tag = "inventory",
    params(
        ("entity_type" = String, Path, description = "Inventory entity type"),
    ),
    request_body = LayoutPayload,
    responses(
        (status = 200, description = "Effective user layout upserted", body = EffectiveUserLayoutResponse),
        (status = "4XX", description = "User layout upsert request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "User layout upsert failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn upsert_user_layout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<LayoutPayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let entity_type = match parse_required_string(&entity_type, "entity_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .inventory_layouts
        .upsert_user_layout(user_id, &entity_type, &payload.layout)
        .await
    {
        Ok(effective) => (StatusCode::OK, Json(map_effective(effective))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete the current user's saved layout.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/layouts/user/{entity_type}",
    tag = "inventory",
    params(
        ("entity_type" = String, Path, description = "Inventory entity type"),
    ),
    responses(
        (status = 204, description = "User layout deleted"),
        (status = "4XX", description = "User layout delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "User layout delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_user_layout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let entity_type = match parse_required_string(&entity_type, "entity_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .inventory_layouts
        .delete_user_layout(user_id, &entity_type)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}
