//! REST handlers for `/api/v1/instance/branding` (org-wide white-label store).
//!
//! `GET` is public so login and first-paint shells can render the same
//! org-wide app name, logo, favicon, and palette without producing auth noise.
//! `PUT` is admin-only and gated exactly like the OIDC settings
//! mutation (`ResourceType::Keycloak` + `Action::Update`), the established
//! instance-wide settings gate. All SQL lives behind
//! `SqlxInstanceBrandingRepository`; this module only parses, gates, delegates,
//! and shapes JSON.

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::config::domain::{InstanceBranding, InstanceBrandingUpdate};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use axum::{
    Extension, Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BrandingResponse {
    pub app_name: String,
    pub logo_data_url: Option<String>,
    pub favicon_data_url: Option<String>,
    #[schema(value_type = Object)]
    pub theme: Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BrandingPayload {
    pub app_name: String,
    pub logo_data_url: Option<String>,
    pub favicon_data_url: Option<String>,
    #[schema(value_type = Object)]
    pub theme: Value,
}

impl From<InstanceBranding> for BrandingResponse {
    fn from(branding: InstanceBranding) -> Self {
        Self {
            app_name: branding.app_name,
            logo_data_url: branding.logo_data_url,
            favicon_data_url: branding.favicon_data_url,
            theme: branding.theme,
            updated_at: branding.updated_at,
        }
    }
}

/// Get the instance branding.
#[utoipa::path(
    get,
    path = "/api/v1/instance/branding",
    tag = "instance-branding",
    responses(
        (status = 200, description = "Instance branding settings", body = BrandingResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_instance_branding(
    State(state): State<AppState>,
    Extension(_request_id): Extension<RequestId>,
) -> Response {
    match state.repositories.instance_branding.get().await {
        Ok(Some(branding)) => Json(BrandingResponse::from(branding)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Instance branding has not been initialized".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Update the instance branding.
#[utoipa::path(
    put,
    path = "/api/v1/instance/branding",
    tag = "instance-branding",
    request_body = BrandingPayload,
    responses(
        (status = 200, description = "Instance branding updated", body = BrandingResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Admin access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_instance_branding(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BrandingPayload>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }

    let updated_by = request_context.user_id;

    let update = InstanceBrandingUpdate {
        app_name: payload.app_name.trim().to_string(),
        logo_data_url: payload.logo_data_url,
        favicon_data_url: payload.favicon_data_url,
        theme: payload.theme,
        updated_by,
    };

    match state.repositories.instance_branding.upsert(update).await {
        Ok(branding) => Json(BrandingResponse::from(branding)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

async fn ensure_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    crate::api::permission_helpers::ensure_access_with_resource(
        state,
        request_context,
        action,
        ResourceType::Branding,
        Some("branding"),
    )
    .await
}
