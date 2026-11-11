use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::{error::handle_app_error, request_id::RequestId};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::sync_helpers::build_pull_schedule_service;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::request_context::RequestContext;
use crate::contexts::sync::domain::MappingRef;
use crate::contexts::sync::infrastructure::persistence::{PullOccurrence, PullSchedule};
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePullScheduleRequest {
    pub provider_id: Uuid,
    pub mapping_version: String,
    pub cron_expression: String,
    pub timezone: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ScheduleListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PullScheduleResponse {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub mapping_version: String,
    pub principal_method: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub cancelled_at: Option<String>,
}

impl From<PullSchedule> for PullScheduleResponse {
    fn from(value: PullSchedule) -> Self {
        Self {
            id: value.id,
            provider_id: value.provider_id,
            mapping_version: MappingRef::new(value.mapping_logical_id, value.mapping_version_id)
                .to_wire(),
            principal_method: value.principal_method,
            cron_expression: value.cron_expression,
            timezone: value.timezone,
            enabled: value.enabled,
            next_run_at: value.next_run_at.map(|t| t.to_rfc3339()),
            last_run_at: value.last_run_at.map(|t| t.to_rfc3339()),
            cancelled_at: value.cancelled_at.map(|t| t.to_rfc3339()),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PullOccurrenceResponse {
    pub scheduled_for: String,
    pub status: String,
    pub job_id: Option<Uuid>,
    pub result_code: Option<String>,
}

impl From<PullOccurrence> for PullOccurrenceResponse {
    fn from(value: PullOccurrence) -> Self {
        Self {
            scheduled_for: value.scheduled_for.to_rfc3339(),
            status: value.status,
            job_id: value.job_id,
            result_code: value.result_code,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PullScheduleDetail {
    pub schedule: PullScheduleResponse,
    pub occurrences: Vec<PullOccurrenceResponse>,
}

async fn authorize(
    state: &AppState,
    request: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    build_policy_evaluation_service(state)
        .ensure_access(request, action, ResourceType::Inventory, None)
        .await
}

#[utoipa::path(post, path = "/api/v1/sync/pull-schedules", tag = "sync-jobs", request_body = CreatePullScheduleRequest,
    responses((status = 201, body = PullScheduleResponse), (status = 400, body = crate::api::rest::error_mapping::ApiErrorResponse), (status = 403, body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreatePullScheduleRequest>,
) -> Response {
    let request = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = authorize(&state, &request, Action::Execute).await {
        return handle_app_error(error);
    }
    let mapping = match MappingRef::parse(&payload.mapping_version) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match build_pull_schedule_service(&state)
        .create(
            payload.provider_id,
            mapping,
            &payload.cron_expression,
            &payload.timezone,
            &request,
        )
        .await
    {
        Ok(schedule) => (
            StatusCode::CREATED,
            Json(PullScheduleResponse::from(schedule)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(get, path = "/api/v1/sync/pull-schedules", tag = "sync-jobs", params(ScheduleListQuery), responses((status = 200, body = [PullScheduleResponse])))]
pub async fn list(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Query(query): Query<ScheduleListQuery>,
) -> Response {
    let request = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = authorize(&state, &request, Action::Read).await {
        return handle_app_error(error);
    }
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    if !(1..=500).contains(&limit) || !(0..=100_000).contains(&offset) {
        return handle_app_error(AppError::Validation("Invalid schedule pagination".into()));
    }
    match build_pull_schedule_service(&state)
        .schedules
        .list(limit, offset)
        .await
    {
        Ok(schedules) => Json(
            schedules
                .into_iter()
                .map(PullScheduleResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(get, path = "/api/v1/sync/pull-schedules/{id}", tag = "sync-jobs", params(("id" = Uuid, Path)), responses((status = 200, body = PullScheduleDetail), (status = 404, body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = authorize(&state, &request, Action::Read).await {
        return handle_app_error(error);
    }
    let service = build_pull_schedule_service(&state);
    let schedule = match service.schedules.get(id).await {
        Ok(Some(value)) => value,
        Ok(None) => return handle_app_error(AppError::NotFound("Schedule not found".into())),
        Err(error) => return handle_app_error(error),
    };
    match service.schedules.occurrences(id, 100).await {
        Ok(occurrences) => Json(PullScheduleDetail {
            schedule: schedule.into(),
            occurrences: occurrences.into_iter().map(Into::into).collect(),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path = "/api/v1/sync/pull-schedules/{id}", tag = "sync-jobs", params(("id" = Uuid, Path)), responses((status = 204, description = "Schedule cancelled"), (status = 404, body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn cancel(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = authorize(&state, &request, Action::Execute).await {
        return handle_app_error(error);
    }
    let service = build_pull_schedule_service(&state);
    match service.schedules.cancel(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => match service.schedules.get(id).await {
            Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
            Ok(None) => handle_app_error(AppError::NotFound("Schedule not found".into())),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}
