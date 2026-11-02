use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access;
use crate::api::rest::envelope_extractors::{Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::sync_helpers::local_inventory_catalog;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::sync::domain::{LocalInventoryResource, SourceEntity, SyncJob};
use crate::contexts::sync::infrastructure::persistence::{ProviderRepository, SyncJobRepository};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct LocalInventoryTargetQuery {
    /// Optional provider whose latest job and enabled state are reported.
    pub provider_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LatestJobResponse {
    pub id: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub stats: serde_json::Value,
}

impl From<SyncJob> for LatestJobResponse {
    fn from(job: SyncJob) -> Self {
        Self {
            id: job.id.to_string(),
            status: format!("{:?}", job.status).to_lowercase(),
            started_at: job.started_at.map(|value| value.to_rfc3339()),
            finished_at: job.finished_at.map(|value| value.to_rfc3339()),
            stats: job.stats,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalInventoryResourcesResponse {
    pub resources: Vec<LocalInventoryResource>,
    pub latest_job: Option<LatestJobResponse>,
    pub provider_enabled: Option<bool>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalInventoryResourceResponse {
    pub resource: LocalInventoryResource,
}

/// List the local inventory target resources.
#[utoipa::path(
    get,
    path = "/api/v1/sync/targets/local-inventory/resources",
    tag = "sync-targets",
    params(LocalInventoryTargetQuery),
    responses(
        (status = 200, description = "Local inventory resources", body = LocalInventoryResourcesResponse),
        (status = 400, description = "Invalid provider query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Provider not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Target resources unavailable", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_local_inventory_resources(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Query(query): Query<LocalInventoryTargetQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
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

    let catalog = match local_inventory_catalog(&state) {
        Ok(catalog) => catalog,
        Err(error) => return handle_app_error(error),
    };

    let provider_id = match query
        .provider_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationUuid(error)),
    };

    let (latest_job, provider_enabled) = match provider_id {
        Some(provider_id) => {
            let enabled = match state
                .repositories
                .sync_providers
                .get_by_id(provider_id)
                .await
            {
                Ok(Some(provider)) => Some(provider.is_enabled),
                Ok(None) => {
                    return handle_app_error(AppError::NotFound("Provider not found".to_string()));
                }
                Err(error) => return handle_app_error(error),
            };
            let job = match state
                .repositories
                .sync_jobs
                .latest_for_provider(provider_id)
                .await
            {
                Ok(job) => job,
                Err(error) => return handle_app_error(error),
            };
            (job.map(LatestJobResponse::from), enabled)
        }
        None => (None, None),
    };

    (
        StatusCode::OK,
        Json(LocalInventoryResourcesResponse {
            resources: catalog.resources(),
            latest_job,
            provider_enabled,
        }),
    )
        .into_response()
}

/// Get one local inventory target resource by its canonical source entity.
#[utoipa::path(
    get,
    path = "/api/v1/sync/targets/local-inventory/resources/{entity}",
    tag = "sync-targets",
    params(("entity" = String, Path, description = "Canonical source entity, e.g. dcim.sites")),
    responses(
        (status = 200, description = "Local inventory resource", body = LocalInventoryResourceResponse),
        (status = 400, description = "Invalid source entity", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Unknown source entity", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Target resources unavailable", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_local_inventory_resource(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(entity): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
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

    let entity = match SourceEntity::new(&entity) {
        Ok(entity) => entity,
        Err(error) => return handle_app_error(error),
    };

    let catalog = match local_inventory_catalog(&state) {
        Ok(catalog) => catalog,
        Err(error) => return handle_app_error(error),
    };

    match catalog.resource(&entity) {
        Some(resource) => (
            StatusCode::OK,
            Json(LocalInventoryResourceResponse { resource }),
        )
            .into_response(),
        None => handle_app_error(AppError::NotFound(format!(
            "No local inventory resource for source entity '{}'",
            entity.as_str()
        ))),
    }
}
