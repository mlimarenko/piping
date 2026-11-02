//! REST handlers for the built-in assistant shell.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access_with_resource as ensure_access;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::assistant::application::assistant_service::CreateAssistantSessionInput;
use crate::contexts::assistant::application::{
    AssistantBootstrapAccess, AssistantBootstrapConfig, AssistantCreateSessionResult,
    AssistantSessionState, DEFAULT_ASSISTANT_ARCHIVE_RETENTION_DAYS,
    DEFAULT_ASSISTANT_MAX_SESSIONS_PER_USER, clamp_archive_retention_days, clamp_max_sessions,
};
use crate::contexts::assistant::domain::{
    AssistantMessage, AssistantSession, AssistantSessionLifecycle, AssistantSessionStatus,
};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::secrets::application::consumer_type::assistant_instance_id;
use crate::contexts::secrets::application::{ASSISTANT_INSTANCE_CONSUMER_TYPE, SecretStore};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

const SETTING_ENABLED: &str = "assistant.instance.enabled";
const SETTING_PROVIDER: &str = "assistant.instance.provider";
const SETTING_BASE_URL: &str = "assistant.instance.base_url";
const SETTING_MODEL: &str = "assistant.instance.model";
const SETTING_PORTAL_CONTEXT_ENABLED: &str = "assistant.instance.portal_context_enabled";
const SETTING_MAX_SESSIONS_PER_USER: &str = "assistant.instance.max_sessions_per_user";
const SETTING_ARCHIVE_RETENTION_DAYS: &str = "assistant.instance.archive_retention_days";
const ASSISTANT_API_KEY_FIELD_KEY: &str = "api_key";

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AssistantCapabilitiesResponse {
    pub session_shell: bool,
    pub chat_messages: bool,
    pub chat_streaming: bool,
    pub mcp_tools: bool,
    pub portal_context_packets: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AssistantBootstrapResponse {
    pub config: AssistantBootstrapConfig,
    pub access: AssistantBootstrapAccess,
    pub feature_status: String,
    pub capabilities: AssistantCapabilitiesResponse,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AssistantSessionListResponse {
    pub sessions: Vec<AssistantSession>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateAssistantSessionRequest {
    pub title: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreateAssistantSessionResponse {
    pub session: AssistantSession,
    pub archived_session_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AssistantSessionLimitResponse {
    pub message: String,
    pub active_session_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateAssistantSessionRequest {
    pub title: Option<String>,
    pub pinned: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SendAssistantMessageRequest {
    pub text: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SendAssistantMessageResponse {
    pub session: AssistantSession,
    pub user_message: AssistantMessage,
    pub assistant_message: AssistantMessage,
}

async fn access_allowed(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_type: ResourceType,
    resource_id: Option<&str>,
) -> Result<bool, AppError> {
    match state
        .services
        .policy_evaluation
        .ensure_access(request_context, action, resource_type, resource_id)
        .await
    {
        Ok(()) => Ok(true),
        Err(AppError::Forbidden(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn string_value(value: Option<Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let value = value.trim().to_string();
            if value.is_empty() { None } else { Some(value) }
        }
        Value::Object(map) => map
            .get("value")
            .or_else(|| map.get("url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn bool_value(value: Option<Value>, fallback: bool) -> bool {
    match value {
        Some(Value::Bool(value)) => value,
        Some(Value::String(value)) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        ),
        Some(Value::Object(map)) => bool_value(map.get("enabled").cloned(), fallback),
        _ => fallback,
    }
}

fn int_value(value: Option<Value>, fallback: i32) -> i32 {
    match value {
        Some(Value::Number(value)) => value.as_i64().and_then(|value| i32::try_from(value).ok()),
        Some(Value::String(value)) => value.trim().parse::<i32>().ok(),
        Some(Value::Object(map)) => Some(int_value(map.get("value").cloned(), fallback)),
        _ => None,
    }
    .unwrap_or(fallback)
}

async fn instance_setting_value(state: &AppState, key: &str) -> Result<Option<Value>, AppError> {
    state.services.settings.get_instance_default(key).await
}

async fn resolve_secret_value(
    state: &AppState,
    secret_id: Uuid,
) -> Result<Option<String>, AppError> {
    match state.services.secret_store.get_secret(secret_id).await {
        Ok(value) => {
            let value = value.trim().to_string();
            Ok((!value.is_empty()).then_some(value))
        }
        Err(AppError::MissingSecret(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn resolve_assistant_api_key(state: &AppState) -> Result<Option<String>, AppError> {
    let consumer_id = assistant_instance_id()?;
    let Some(binding) = state
        .repositories
        .secrets
        .get_usage_binding(
            ASSISTANT_INSTANCE_CONSUMER_TYPE,
            consumer_id,
            ASSISTANT_API_KEY_FIELD_KEY,
        )
        .await?
    else {
        return Ok(None);
    };
    resolve_secret_value(state, binding.secret_id).await
}

async fn assistant_api_key_configured(state: &AppState) -> Result<bool, AppError> {
    Ok(resolve_assistant_api_key(state).await?.is_some())
}

async fn assistant_config(state: &AppState) -> Result<AssistantBootstrapConfig, AppError> {
    let enabled = bool_value(instance_setting_value(state, SETTING_ENABLED).await?, false);
    let provider = string_value(instance_setting_value(state, SETTING_PROVIDER).await?);
    let base_url = string_value(instance_setting_value(state, SETTING_BASE_URL).await?);
    let model = string_value(instance_setting_value(state, SETTING_MODEL).await?);
    let portal_context_enabled = bool_value(
        instance_setting_value(state, SETTING_PORTAL_CONTEXT_ENABLED).await?,
        false,
    );
    let max_sessions_per_user = clamp_max_sessions(int_value(
        instance_setting_value(state, SETTING_MAX_SESSIONS_PER_USER).await?,
        DEFAULT_ASSISTANT_MAX_SESSIONS_PER_USER,
    ));
    let archive_retention_days = clamp_archive_retention_days(int_value(
        instance_setting_value(state, SETTING_ARCHIVE_RETENTION_DAYS).await?,
        DEFAULT_ASSISTANT_ARCHIVE_RETENTION_DAYS,
    ));

    Ok(AssistantBootstrapConfig {
        instance_enabled: enabled,
        provider,
        base_url_configured: base_url.is_some(),
        api_key_configured: assistant_api_key_configured(state).await?,
        model,
        portal_context_enabled,
        max_sessions_per_user,
        archive_retention_days,
    })
}

async fn assistant_access(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<AssistantBootstrapAccess, AppError> {
    Ok(AssistantBootstrapAccess {
        can_open: access_allowed(
            state,
            request_context,
            Action::Read,
            ResourceType::Assistant,
            Some("assistant"),
        )
        .await?,
        can_execute: access_allowed(
            state,
            request_context,
            Action::Execute,
            ResourceType::Assistant,
            Some("assistant"),
        )
        .await?,
        can_update_preferences: access_allowed(
            state,
            request_context,
            Action::Update,
            ResourceType::Assistant,
            Some("assistant"),
        )
        .await?,
        can_view_conversations: access_allowed(
            state,
            request_context,
            Action::ViewAudit,
            ResourceType::AssistantConversation,
            Some("assistant_conversation:*"),
        )
        .await?,
    })
}

fn feature_status(config: &AssistantBootstrapConfig, access: &AssistantBootstrapAccess) -> String {
    if !config.instance_enabled {
        return "disabled_by_instance".to_string();
    }
    if !access.can_open {
        return "disabled_by_policy".to_string();
    }
    "session_shell_ready".to_string()
}

fn manual_archive_conflict(existing: &AssistantSession, user_id: Uuid) -> Option<AppError> {
    if existing.user_id != user_id {
        return Some(AppError::Forbidden(
            "Assistant session access denied".to_string(),
        ));
    }
    if existing.status == AssistantSessionStatus::Running {
        return Some(AppError::Conflict(
            "Running assistant sessions cannot be archived implicitly".to_string(),
        ));
    }
    if existing.lifecycle != AssistantSessionLifecycle::Active {
        return Some(AppError::NotFound(
            "Assistant session not found".to_string(),
        ));
    }
    None
}

async fn ensure_assistant_open(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<AssistantBootstrapConfig, AppError> {
    let config = assistant_config(state).await?;
    if !config.instance_enabled {
        return Err(AppError::Forbidden("Assistant is disabled".to_string()));
    }
    ensure_access(
        state,
        request_context,
        Action::Read,
        ResourceType::Assistant,
        Some("assistant"),
    )
    .await?;
    Ok(config)
}

async fn record_assistant_event(
    state: &AppState,
    actor_id: Uuid,
    event_type: &str,
    session: &AssistantSession,
    metadata: Value,
) -> Result<(), AppError> {
    state
        .services
        .audit
        .record_event(AuditEvent {
            id: Uuid::now_v7(),
            event_type: event_type.to_string(),
            actor_id: Some(actor_id),
            actor_kind: "user".to_string(),
            target_type: Some("assistant_session".to_string()),
            target_id: Some(session.id),
            metadata,
            created_at: Utc::now(),
        })
        .await
}

/// Bootstrap the assistant shell for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/assistant/bootstrap",
    tag = "assistant",
    responses(
        (status = 200, description = "Assistant bootstrap state", body = AssistantBootstrapResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_assistant_bootstrap(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    let config = match assistant_config(&state).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let access = match assistant_access(&state, &request_context).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    Json(AssistantBootstrapResponse {
        feature_status: feature_status(&config, &access),
        capabilities: AssistantCapabilitiesResponse {
            session_shell: config.instance_enabled && access.can_open,
            chat_messages: false,
            chat_streaming: false,
            mcp_tools: false,
            portal_context_packets: false,
        },
        config,
        access,
    })
    .into_response()
}

/// List current user's assistant sessions.
#[utoipa::path(
    get,
    path = "/api/v1/assistant/sessions",
    tag = "assistant",
    responses(
        (status = 200, description = "Assistant sessions", body = AssistantSessionListResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Assistant access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_assistant_sessions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_assistant_open(&state, &request_context).await {
        return handle_app_error(error);
    }
    match state.services.assistant.list_sessions(user_id).await {
        Ok(sessions) => Json(AssistantSessionListResponse { sessions }).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a new assistant session, rotating old idle sessions when needed.
#[utoipa::path(
    post,
    path = "/api/v1/assistant/sessions",
    tag = "assistant",
    request_body = CreateAssistantSessionRequest,
    responses(
        (status = 201, description = "Assistant session created", body = CreateAssistantSessionResponse),
        (status = 409, description = "No eligible session can be rotated", body = AssistantSessionLimitResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Assistant access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_assistant_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateAssistantSessionRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let config = match ensure_assistant_open(&state, &request_context).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .services
        .assistant
        .create_session(
            user_id,
            CreateAssistantSessionInput {
                title: payload.title,
                provider: config.provider.clone(),
                model: config.model.clone(),
                max_active_sessions: config.max_sessions_per_user,
                archive_retention_days: config.archive_retention_days,
            },
        )
        .await
    {
        Ok(AssistantCreateSessionResult::Created {
            session,
            archived_session_id,
        }) => {
            let session = *session;
            if let Err(error) = record_assistant_event(
                &state,
                user_id,
                "assistant.session.created",
                &session,
                json!({ "archived_session_id": archived_session_id }),
            )
            .await
            {
                return handle_app_error(error);
            }
            (
                StatusCode::CREATED,
                Json(CreateAssistantSessionResponse {
                    session,
                    archived_session_id,
                }),
            )
                .into_response()
        }
        Ok(AssistantCreateSessionResult::LimitReached(conflict)) => (
            StatusCode::CONFLICT,
            Json(AssistantSessionLimitResponse {
                message: "Assistant session limit reached; archive or unpin a session before creating another one.".to_string(),
                active_session_ids: conflict.active_session_ids,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Reject assistant chat messages while provider messaging is disabled.
#[utoipa::path(
    post,
    path = "/api/v1/assistant/sessions/{id}/messages",
    tag = "assistant",
    params(("id" = Uuid, Path, description = "Assistant session id")),
    request_body = SendAssistantMessageRequest,
    responses(
        (status = 400, description = "Assistant provider messaging is not enabled.", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Assistant execute access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn send_assistant_message(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(_id): Path<Uuid>,
    EnvelopeJson(_payload): EnvelopeJson<SendAssistantMessageRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_assistant_open(&state, &request_context).await {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Execute,
        ResourceType::Assistant,
        Some("assistant"),
    )
    .await
    {
        return handle_app_error(error);
    }

    handle_app_error(AppError::Validation(
        "Assistant provider messaging is not enabled in this release".to_string(),
    ))
}

/// Get assistant session state and transcript if permitted.
#[utoipa::path(
    get,
    path = "/api/v1/assistant/sessions/{id}/state",
    tag = "assistant",
    params(("id" = Uuid, Path, description = "Assistant session id")),
    responses(
        (status = 200, description = "Assistant session state", body = AssistantSessionState),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Assistant conversation access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Assistant session not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_assistant_session_state(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let Some(session) = (match state.services.assistant.get_session(id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    }) else {
        return handle_app_error(AppError::NotFound(
            "Assistant session not found".to_string(),
        ));
    };

    let access_result = if session.user_id == user_id {
        ensure_assistant_open(&state, &request_context)
            .await
            .map(|_| ())
    } else {
        ensure_access(
            &state,
            &request_context,
            Action::ViewAudit,
            ResourceType::AssistantConversation,
            Some("assistant_conversation:*"),
        )
        .await
    };
    if let Err(error) = access_result {
        return handle_app_error(error);
    }

    match state.services.assistant.get_session_state(id).await {
        Ok(Some(state_value)) => Json(state_value).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Assistant session not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Update a current user's active assistant session.
#[utoipa::path(
    patch,
    path = "/api/v1/assistant/sessions/{id}",
    tag = "assistant",
    params(("id" = Uuid, Path, description = "Assistant session id")),
    request_body = UpdateAssistantSessionRequest,
    responses(
        (status = 200, description = "Assistant session updated", body = AssistantSession),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Assistant access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Assistant session not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_assistant_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdateAssistantSessionRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_assistant_open(&state, &request_context).await {
        return handle_app_error(error);
    }
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Assistant,
        Some("assistant"),
    )
    .await
    {
        return handle_app_error(error);
    }
    let Some(existing) = (match state.services.assistant.get_session(id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    }) else {
        return handle_app_error(AppError::NotFound(
            "Assistant session not found".to_string(),
        ));
    };
    if existing.user_id != user_id {
        return handle_app_error(AppError::Forbidden(
            "Assistant session access denied".to_string(),
        ));
    }

    match state
        .services
        .assistant
        .update_session(id, payload.title, payload.pinned)
        .await
    {
        Ok(Some(session)) => Json(session).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Assistant session not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Archive a current user's assistant session.
#[utoipa::path(
    delete,
    path = "/api/v1/assistant/sessions/{id}",
    tag = "assistant",
    params(("id" = Uuid, Path, description = "Assistant session id")),
    responses(
        (status = 204, description = "Assistant session archived"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Assistant access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Assistant session not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Conflict", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn archive_assistant_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let config = match ensure_assistant_open(&state, &request_context).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let Some(existing) = (match state.services.assistant.get_session(id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    }) else {
        return handle_app_error(AppError::NotFound(
            "Assistant session not found".to_string(),
        ));
    };
    if existing.user_id != user_id {
        return handle_app_error(AppError::Forbidden(
            "Assistant session access denied".to_string(),
        ));
    }
    if existing.status.to_string() == "running" {
        return handle_app_error(AppError::Conflict(
            "Running assistant sessions cannot be archived implicitly".to_string(),
        ));
    }

    match state
        .services
        .assistant
        .archive_session(id, user_id, config.archive_retention_days)
        .await
    {
        Ok(Some(session)) => {
            if let Err(error) = record_assistant_event(
                &state,
                user_id,
                "assistant.session.archived",
                &session,
                json!({ "manual": true }),
            )
            .await
            {
                return handle_app_error(error);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            match state.services.assistant.get_session(id).await {
                Ok(Some(current)) => {
                    if let Some(error) = manual_archive_conflict(&current, user_id) {
                        return handle_app_error(error);
                    }
                    "Assistant session not found".to_string()
                }
                Ok(None) => "Assistant session not found".to_string(),
                Err(error) => return handle_app_error(error),
            },
        )),
        Err(error) => handle_app_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant_session_for_archive(
        user_id: Uuid,
        lifecycle: AssistantSessionLifecycle,
        status: AssistantSessionStatus,
    ) -> AssistantSession {
        AssistantSession {
            id: Uuid::now_v7(),
            user_id,
            title: "Ops triage".to_string(),
            status,
            lifecycle,
            pinned: false,
            provider: Some("openrouter".to_string()),
            model: Some("openai/gpt-4.1-mini".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived_at: None,
            purge_after: None,
            purged_at: None,
            last_error: None,
            message_count: 0,
            metadata: json!({}),
        }
    }

    #[test]
    fn manual_archive_conflict_maps_running_session_to_conflict() {
        let user_id = Uuid::now_v7();
        let session = assistant_session_for_archive(
            user_id,
            AssistantSessionLifecycle::Active,
            AssistantSessionStatus::Running,
        );
        let error = manual_archive_conflict(&session, user_id).unwrap();
        assert!(matches!(error, AppError::Conflict(_)));
    }

    #[test]
    fn manual_archive_conflict_allows_owned_idle_active_session() {
        let user_id = Uuid::now_v7();
        let session = assistant_session_for_archive(
            user_id,
            AssistantSessionLifecycle::Active,
            AssistantSessionStatus::Idle,
        );
        assert!(manual_archive_conflict(&session, user_id).is_none());
    }
}
