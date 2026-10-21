use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::{
    api::{
        middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId},
        rest::request_context::build_request_context,
    },
    app::AppState,
    contexts::{
        access_control::domain::{Action, ResourceType},
        inventory::infrastructure::persistence::{
            CollectedFactsIngestRequest, CollectedFactsIngestResponse,
        },
    },
    errors::AppError,
};

use super::super::ensure_access;

/// Ingest facts collected by a playbook run.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/collected-facts",
    tag = "inventory",
    request_body = CollectedFactsIngestRequest,
    responses(
        (status = 200, description = "Collected facts ingested", body = CollectedFactsIngestResponse),
        (status = "4XX", description = "Collected facts ingest request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Collected facts ingest failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn ingest_collected_facts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CollectedFactsIngestRequest>,
) -> impl IntoResponse {
    let job_token_id = playbook_job_token_id(Some(&auth_context));
    if job_token_id.is_none() {
        return handle_app_error(AppError::Forbidden(
            "Collected facts ingestion requires a playbook job token".to_string(),
        ));
    }
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);

    for action in [Action::Create, Action::Update] {
        if let Err(error) =
            ensure_access(&state, &request_context, action, ResourceType::Inventory).await
        {
            return handle_app_error(error);
        }
    }

    match state
        .repositories
        .collected_facts
        .ingest(job_token_id, payload)
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

fn playbook_job_token_id(auth_context: Option<&Extension<AuthContext>>) -> Option<Uuid> {
    let context = auth_context?;
    if context.auth_method != "playbook_job_token" {
        return None;
    }
    context.api_token_id.as_deref()?.parse().ok()
}
