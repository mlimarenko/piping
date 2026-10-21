use axum::{
    Extension, Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde_json::json;
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::Path;
use crate::api::rest::request_context::build_request_context;
use crate::api::sync_helpers::build_source_service;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::sync::domain::ProviderType;
use crate::errors::AppError;

/// Validate provider connection settings.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/validate",
    tag = "providers",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Provider connection is valid", body = serde_json::Value),
        (status = "4XX", description = "Provider validation request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Provider validation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn validate_provider(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Execute,
            ResourceType::Inventory,
            None,
        )
        .await
    {
        return handle_app_error(error).into_response();
    }
    let provider_id = match Uuid::parse_str(&provider_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid provider id: {error}"
            )))
            .into_response();
        }
    };

    let source_service = build_source_service(&state);
    let source = match source_service.validate_source_connection(provider_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error).into_response(),
    };
    let provider_type = match source.provider_type {
        ProviderType::Netbox => "netbox",
        ProviderType::Custom => "custom",
    };
    Json(json!({
        "id": source.id,
        "provider_type": provider_type,
        "valid": true
    }))
    .into_response()
}
