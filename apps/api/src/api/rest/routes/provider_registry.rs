use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access;
use crate::api::rest::envelope_extractors::Path;
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::validation::parse_uuid;
use crate::api::sync_helpers::{
    build_registry_for_provider, invalidate_netbox_registry_cache_for_provider,
    refresh_netbox_registry_for_provider,
};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::sync::domain::Provider;
use crate::contexts::sync::domain::ProviderType;
use crate::errors::AppError;
use crate::integrations::netbox::NetboxResourceMeta;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryResourceInfo {
    pub resource_key: String,
    pub list_path: String,
    pub detail_path_template: String,
    pub operations: Vec<String>,
    pub ordering_supported: bool,
    pub changelog_message_supported: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryResourcesResponse {
    pub resources: Vec<RegistryResourceInfo>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryResourceMetaResponse {
    #[schema(value_type = Object)]
    pub resource: NetboxResourceMeta,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryRefreshResponse {
    pub refreshed: bool,
    pub resource_count: usize,
}

/// List the resource types in the provider registry.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/registry/resources",
    tag = "provider-registry",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Registry resources", body = RegistryResourcesResponse),
        (status = "4XX", description = "Registry resources request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Registry resources request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_registry_resources(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
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
    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider = match load_provider(&state, provider_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let registry = match build_registry_for_provider(&state, &provider).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let mut resources = Vec::new();
    for resource_key in registry.list_resources() {
        if let Some(descriptor) = registry.get_descriptor(&resource_key) {
            let mut operations: Vec<String> =
                descriptor.operations_supported.iter().cloned().collect();
            operations.sort();
            resources.push(RegistryResourceInfo {
                resource_key,
                list_path: descriptor.list_path.clone(),
                detail_path_template: descriptor.detail_path_template.clone(),
                operations,
                ordering_supported: descriptor.ordering_supported,
                changelog_message_supported: descriptor.changelog_message_supported,
            });
        }
    }

    (
        StatusCode::OK,
        Json(RegistryResourcesResponse { resources }),
    )
        .into_response()
}

/// Get metadata for a provider-registry resource type.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/registry/resources/{resource_key}",
    tag = "provider-registry",
    params(
        ("id" = String, Path, description = "Provider id"),
        ("resource_key" = String, Path, description = "Registry resource key"),
    ),
    responses(
        (status = 200, description = "Registry resource metadata", body = RegistryResourceMetaResponse),
        (status = 404, description = "Registry resource not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Registry resource request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Registry resource request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_registry_resource_meta(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path((provider_id, resource_key)): Path<(String, String)>,
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
    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider = match load_provider(&state, provider_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let registry = match build_registry_for_provider(&state, &provider).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match registry.get_resource_meta(&resource_key) {
        Some(resource) => (
            StatusCode::OK,
            Json(RegistryResourceMetaResponse { resource }),
        )
            .into_response(),
        None => handle_app_error(AppError::NotFound("Resource not found".to_string())),
    }
}

/// Refresh the provider registry.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/registry/refresh",
    tag = "provider-registry",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 200, description = "Registry refreshed", body = RegistryRefreshResponse),
        (status = "4XX", description = "Registry refresh request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Registry refresh failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn refresh_registry(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Execute,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider = match load_provider(&state, provider_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    // Custom providers have no remote schema to refresh.
    if provider.provider_type == ProviderType::Custom {
        return (
            StatusCode::OK,
            Json(RegistryRefreshResponse {
                refreshed: false,
                resource_count: 0,
            }),
        )
            .into_response();
    }
    let registry = match refresh_netbox_registry_for_provider(&state, &provider).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    // Drop cached preview samples for this
    // provider so the FE editor's Preview tab re-fetches against the
    // freshly-loaded NetBox schema. Otherwise stale records survive a
    // schema change and the preview shows fields that no longer exist.
    state
        .services
        .sample_record_cache
        .invalidate_provider(provider_id)
        .await;
    let count = registry.list_resources().len();
    (
        StatusCode::OK,
        Json(RegistryRefreshResponse {
            refreshed: true,
            resource_count: count,
        }),
    )
        .into_response()
}

/// Invalidate the provider-registry cache.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/registry/invalidate",
    tag = "provider-registry",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    responses(
        (status = 204, description = "Registry cache invalidated"),
        (status = "4XX", description = "Registry invalidation request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Registry invalidation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn invalidate_registry(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Execute,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider = match load_provider(&state, provider_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    // Custom providers have no cached schema to invalidate.
    if provider.provider_type == ProviderType::Custom {
        return StatusCode::NO_CONTENT.into_response();
    }
    if let Err(error) = invalidate_netbox_registry_cache_for_provider(&state, &provider).await {
        return handle_app_error(error);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn load_provider(state: &AppState, provider_id: Uuid) -> Result<Provider, AppError> {
    let service = state.services.sync_queries.clone();
    service
        .get_provider(provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))
}
