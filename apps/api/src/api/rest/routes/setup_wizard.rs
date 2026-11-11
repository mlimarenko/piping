use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::setup::application::wizard_service::CONNECTION_SETUP_WIZARD_TYPE;
use crate::contexts::setup::domain::{WizardProgress, WizardReadiness};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

const CANONICAL_INSTALL_SOURCE_LABEL: &str = "Piping";
const CANONICAL_INSTALL_SOURCE_URL: &str = "https://github.com/mlimarenko/piping";
const CANONICAL_INSTALL_SHELL: &str = "bash";
const CANONICAL_INSTALL_SUPPORTED_PLATFORMS: [&str; 3] = ["Linux", "Docker Compose", "Kubernetes"];

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateWizardProgressRequest {
    pub current_step: String,
    pub step_data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InstallPublicationStatus {
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupWizardInstallMetadataResponse {
    #[schema(required = true, nullable = true)]
    pub command: Option<String>,
    pub publication_status: InstallPublicationStatus,
    pub shell: String,
    pub source_repository_label: String,
    pub source_repository_url: String,
    pub supported_platforms: Vec<String>,
}

impl SetupWizardInstallMetadataResponse {
    pub const FIELD_NAMES: &'static [&'static str] = &[
        "command",
        "publicationStatus",
        "shell",
        "sourceRepositoryLabel",
        "sourceRepositoryUrl",
        "supportedPlatforms",
    ];
    pub const REQUIRED_FIELDS: &'static [&'static str] = Self::FIELD_NAMES;
    pub const SCHEMA_NAME: &'static str = "SetupWizardInstallMetadataResponse";
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WizardReadinessResponse {
    pub has_hosts: bool,
    pub has_connection_configs: bool,
    pub user_has_credential_sets: bool,
    pub user_has_connected: bool,
    pub wizard_status: Option<String>,
    pub wizard_current_step: Option<String>,
    pub user_is_admin: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WizardProgressResponse {
    pub id: String,
    pub user_id: String,
    pub wizard_type: String,
    pub current_step: Option<String>,
    pub step_data: Value,
    pub status: String,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CompleteWizardResponse {
    pub status: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DismissWizardResponse {
    pub status: String,
}

pub fn canonical_setup_wizard_install_metadata()
-> Result<SetupWizardInstallMetadataResponse, AppError> {
    let source_repository_url = CANONICAL_INSTALL_SOURCE_URL.trim();
    if source_repository_url.is_empty() {
        return Err(AppError::Config(
            "Canonical install source repository URL is not configured".to_string(),
        ));
    }

    Ok(SetupWizardInstallMetadataResponse {
        command: None,
        publication_status: InstallPublicationStatus::Pending,
        shell: CANONICAL_INSTALL_SHELL.to_string(),
        source_repository_label: CANONICAL_INSTALL_SOURCE_LABEL.to_string(),
        source_repository_url: source_repository_url.to_string(),
        supported_platforms: CANONICAL_INSTALL_SUPPORTED_PLATFORMS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    })
}

/// Get the installation descriptor.
#[utoipa::path(
    get,
    path = "/api/v1/setup/wizard/install",
    tag = "setup-wizard",
    responses(
        (status = 200, description = "Setup wizard install metadata", body = SetupWizardInstallMetadataResponse),
        (status = "4XX", description = "Setup wizard install request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard install request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_install(
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.user_id.is_none() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }

    match canonical_setup_wizard_install_metadata() {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Check setup readiness.
#[utoipa::path(
    get,
    path = "/api/v1/setup/wizard/readiness",
    tag = "setup-wizard",
    responses(
        (status = 200, description = "Setup wizard readiness", body = WizardReadinessResponse),
        (status = "4XX", description = "Setup wizard readiness request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard readiness request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_readiness(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let user_is_admin = match resolve_admin_flag(&state, &request_context).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.wizard.clone();
    match service.check_readiness(user_id, user_is_admin).await {
        Ok(readiness) => (
            StatusCode::OK,
            Json(wizard_readiness_to_response(readiness)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get setup-wizard progress.
#[utoipa::path(
    get,
    path = "/api/v1/setup/wizard/progress",
    tag = "setup-wizard",
    responses(
        (status = 200, description = "Setup wizard progress", body = WizardProgressResponse),
        (status = 404, description = "Wizard progress not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Setup wizard progress request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard progress request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_progress(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.wizard.clone();
    match service
        .get_progress(user_id, CONNECTION_SETUP_WIZARD_TYPE)
        .await
    {
        Ok(Some(progress)) => {
            (StatusCode::OK, Json(wizard_progress_to_response(progress))).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Wizard progress not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Update setup-wizard progress.
#[utoipa::path(
    put,
    path = "/api/v1/setup/wizard/progress",
    tag = "setup-wizard",
    request_body = UpdateWizardProgressRequest,
    responses(
        (status = 200, description = "Setup wizard progress updated", body = WizardProgressResponse),
        (status = "4XX", description = "Setup wizard progress update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard progress update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_progress(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<UpdateWizardProgressRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.wizard.clone();
    match service
        .update_progress(
            user_id,
            CONNECTION_SETUP_WIZARD_TYPE,
            payload.current_step.as_str(),
            payload.step_data,
        )
        .await
    {
        Ok(progress) => {
            (StatusCode::OK, Json(wizard_progress_to_response(progress))).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Mark the setup wizard complete.
#[utoipa::path(
    post,
    path = "/api/v1/setup/wizard/complete",
    tag = "setup-wizard",
    responses(
        (status = 200, description = "Setup wizard completed", body = CompleteWizardResponse),
        (status = "4XX", description = "Setup wizard complete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard complete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn complete_wizard(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.wizard.clone();
    match service
        .complete(user_id, CONNECTION_SETUP_WIZARD_TYPE)
        .await
    {
        Ok(progress) => (
            StatusCode::OK,
            Json(CompleteWizardResponse {
                status: progress.status,
                completed_at: progress.completed_at.map(|value| value.to_rfc3339()),
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Dismiss the setup wizard.
#[utoipa::path(
    post,
    path = "/api/v1/setup/wizard/dismiss",
    tag = "setup-wizard",
    responses(
        (status = 200, description = "Setup wizard dismissed", body = DismissWizardResponse),
        (status = "4XX", description = "Setup wizard dismiss request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard dismiss request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn dismiss_wizard(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.wizard.clone();
    match service.dismiss(user_id, CONNECTION_SETUP_WIZARD_TYPE).await {
        Ok(progress) => (
            StatusCode::OK,
            Json(DismissWizardResponse {
                status: progress.status,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Reset the setup wizard.
#[utoipa::path(
    post,
    path = "/api/v1/setup/wizard/reset",
    tag = "setup-wizard",
    responses(
        (status = 200, description = "Setup wizard reset", body = WizardProgressResponse),
        (status = "4XX", description = "Setup wizard reset request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Setup wizard reset request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn reset_wizard(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.wizard.clone();
    match service.reset(user_id, CONNECTION_SETUP_WIZARD_TYPE).await {
        Ok(progress) => {
            (StatusCode::OK, Json(wizard_progress_to_response(progress))).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

fn wizard_readiness_to_response(readiness: WizardReadiness) -> WizardReadinessResponse {
    WizardReadinessResponse {
        has_hosts: readiness.has_hosts,
        has_connection_configs: readiness.has_connection_configs,
        user_has_credential_sets: readiness.user_has_credential_sets,
        user_has_connected: readiness.user_has_connected,
        wizard_status: readiness.wizard_status,
        wizard_current_step: readiness.wizard_current_step,
        user_is_admin: readiness.user_is_admin,
    }
}

fn wizard_progress_to_response(progress: WizardProgress) -> WizardProgressResponse {
    WizardProgressResponse {
        id: progress.id.to_string(),
        user_id: progress.user_id.to_string(),
        wizard_type: progress.wizard_type,
        current_step: progress.current_step,
        step_data: progress.step_data,
        status: progress.status,
        completed_at: progress.completed_at.map(|value| value.to_rfc3339()),
        created_at: progress.created_at.to_rfc3339(),
        updated_at: progress.updated_at.to_rfc3339(),
    }
}

async fn resolve_admin_flag(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<bool, AppError> {
    // Report admin exactly the way an admin action is authorized: through the
    // guarded `ensure_access` path. The bare `evaluate` path this used to call
    // skips BOTH the API-token scope ceiling and the request-derived constraint
    // context (ip/timestamp), so a PAT scoped away from `ManagePolicies`, or an
    // admin boxed in by an ip/time-constrained deny, would still read as admin.
    // `ensure_access` applies both; a `Forbidden` (scope- or policy-denied) is
    // simply "not admin", while any other error propagates rather than being
    // swallowed to `false`.
    let policy_service = build_policy_evaluation_service(state);
    match policy_service
        .ensure_access(
            request_context,
            Action::ManagePolicies,
            ResourceType::Policy,
            None,
        )
        .await
    {
        Ok(()) => Ok(true),
        Err(AppError::Forbidden(_)) => Ok(false),
        Err(error) => Err(error),
    }
}
