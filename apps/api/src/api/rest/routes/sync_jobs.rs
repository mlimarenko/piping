use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::error_mapping::build_error_response;
use crate::api::rest::request_context::build_request_context;
use crate::api::sync_helpers::build_inventory_pull_runner;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::sync::domain::{MappingConfigurationStatus, MappingRef, ProviderType};
use crate::contexts::sync::infrastructure::persistence::{
    MappingConfigurationRepository, SyncJobRepository,
};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateSyncJobRequest {
    pub provider_id: String,
    pub mapping_version: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncJobResponse {
    pub id: String,
    pub provider_id: String,
    pub mapping_version: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub stats: serde_json::Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncJobReportResponse {
    pub id: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub stats: serde_json::Value,
}

/// Start a sync job.
#[utoipa::path(
    post,
    path = "/api/v1/sync/jobs",
    tag = "sync-jobs",
    request_body = CreateSyncJobRequest,
    responses(
        (status = 200, description = "Sync job created", body = SyncJobResponse),
        (status = 400, description = "Sync job create request was invalid", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Provider not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync job create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 422, description = "Unprocessable entity", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_sync_job(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateSyncJobRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
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
        return handle_app_error(error);
    }
    let provider_id = match Uuid::parse_str(&payload.provider_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationUuid(error)),
    };

    let service = state.services.sync_queries.clone();
    let provider = match service.get_provider(provider_id).await {
        Ok(Some(provider)) => provider,
        Ok(None) => return handle_app_error(AppError::NotFound("Provider not found".to_string())),
        Err(error) => return map_error(error),
    };
    if !provider.is_enabled {
        return handle_app_error(AppError::Validation(format!(
            "Provider '{}' is disabled",
            provider.name
        )));
    }

    // Custom providers do not support automated sync; use the manual import API.
    if provider.provider_type == ProviderType::Custom {
        return build_error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            "custom providers do not support automated sync yet; use manual import API",
            "VALIDATION_ERROR",
        );
    }

    // `mapping_version` is the immutable `<logical_uuid>:<version>` reference
    // of a mapping configuration; there is no free-form legacy version.
    let reference = match MappingRef::parse(&payload.mapping_version) {
        Ok(reference) => reference,
        Err(error) => return handle_app_error(error),
    };
    let mapping_version = reference.to_wire();
    let document = match state
        .repositories
        .sync_mapping_configurations
        .get_document(reference.logical_id, reference.version_id)
        .await
    {
        Ok(Some(document)) => document,
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Mapping configuration not found".into()));
        }
        Err(error) => return handle_app_error(error),
    };
    if document.source_provider_id != provider_id
        || document.status != MappingConfigurationStatus::Active
    {
        return handle_app_error(AppError::Validation(
            "An active mapping configuration owned by this source is required".into(),
        ));
    }
    if let Err(error) = document.target_sync_request().ensure_supported() {
        return handle_app_error(error);
    }

    let runner = match build_inventory_pull_runner(&state, &provider).await {
        Ok(runner) => runner,
        Err(error) => return map_error(error),
    };

    // Run the pull on a detached task and await its handle.
    //
    // The work used to run inline in the request future. Axum drops that future
    // when the client goes away (a closed browser tab is enough), which cancels
    // the sync mid-flight: neither `complete()` nor `fail()` executes, so the
    // `sync_job` row stays `running` forever — the UI filters on exactly that
    // status — and the Redis lock is never released either. Dropping a
    // `JoinHandle` does NOT cancel its task, so awaiting it here keeps the
    // response contract identical while making the job always reach a terminal
    // state.
    let sync_task =
        tokio::spawn(async move { runner.run_pull(provider_id, &mapping_version).await });
    let outcome = match sync_task.await {
        Ok(outcome) => outcome,
        Err(join_error) => Err(AppError::Internal(format!(
            "sync task failed to run to completion: {join_error}"
        ))),
    };
    match outcome {
        Ok(job) => (
            StatusCode::OK,
            Json(SyncJobResponse {
                id: job.id.to_string(),
                provider_id: job.provider_id.to_string(),
                mapping_version: job.mapping_version,
                status: format!("{:?}", job.status).to_lowercase(),
                started_at: job
                    .started_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                finished_at: job
                    .finished_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                stats: job.stats,
            }),
        )
            .into_response(),
        Err(error) => map_error(error),
    }
}

/// Get a sync job.
#[utoipa::path(
    get,
    path = "/api/v1/sync/jobs/{id}",
    tag = "sync-jobs",
    params(
        ("id" = String, Path, description = "Sync job id"),
    ),
    responses(
        (status = 200, description = "Sync job", body = SyncJobResponse),
        (status = 400, description = "Sync job id was invalid", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Sync job not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync job request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_sync_job(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Read,
            ResourceType::Inventory,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationUuid(error)),
    };

    let service = state.services.sync_queries.clone();
    match service.get_job(job_id).await {
        Ok(Some(job)) => (
            StatusCode::OK,
            Json(SyncJobResponse {
                id: job.id.to_string(),
                provider_id: job.provider_id.to_string(),
                mapping_version: job.mapping_version,
                status: format!("{:?}", job.status).to_lowercase(),
                started_at: job
                    .started_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                finished_at: job
                    .finished_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                stats: job.stats,
            }),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Sync job not found".to_string())),
        Err(error) => map_error(error),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct LatestSyncJobQuery {
    pub provider_id: String,
}

/// Get the latest sync job for a provider, bounded to one row.
#[utoipa::path(
    get,
    path = "/api/v1/sync/jobs/latest",
    tag = "sync-jobs",
    params(LatestSyncJobQuery),
    responses(
        (status = 200, description = "Latest sync job", body = SyncJobReportResponse),
        (status = 400, description = "Provider id was invalid", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "No sync job for provider", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync job request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_latest_sync_job(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Query(query): Query<LatestSyncJobQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Read,
            ResourceType::Inventory,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }
    let provider_id = match Uuid::parse_str(&query.provider_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationUuid(error)),
    };

    match state
        .repositories
        .sync_jobs
        .latest_for_provider(provider_id)
        .await
    {
        Ok(Some(job)) => (
            StatusCode::OK,
            Json(SyncJobReportResponse {
                id: job.id.to_string(),
                status: format!("{:?}", job.status).to_lowercase(),
                started_at: job
                    .started_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                finished_at: job
                    .finished_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                stats: job.stats,
            }),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("No sync job for provider".to_string())),
        Err(error) => map_error(error),
    }
}

/// Get a sync job's report.
#[utoipa::path(
    get,
    path = "/api/v1/sync/jobs/{id}/report",
    tag = "sync-jobs",
    params(
        ("id" = String, Path, description = "Sync job id"),
    ),
    responses(
        (status = 200, description = "Sync job report", body = SyncJobReportResponse),
        (status = 400, description = "Sync job id was invalid", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Sync job not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync job report request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_sync_job_report(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Read,
            ResourceType::Inventory,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationUuid(error)),
    };

    let service = state.services.sync_queries.clone();
    match service.get_job(job_id).await {
        Ok(Some(job)) => (
            StatusCode::OK,
            Json(SyncJobReportResponse {
                id: job.id.to_string(),
                status: format!("{:?}", job.status).to_lowercase(),
                started_at: job
                    .started_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                finished_at: job
                    .finished_at
                    .map(|value: chrono::DateTime<chrono::Utc>| value.to_rfc3339()),
                stats: job.stats,
            }),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Sync job not found".to_string())),
        Err(error) => map_error(error),
    }
}

fn map_error(error: AppError) -> axum::response::Response {
    handle_app_error(error)
}
