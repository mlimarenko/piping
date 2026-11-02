use crate::contexts::connections::application::connection_service::normalize_launch_params;
use axum::{
    Extension, Json,
    body::Body,
    extract::{Multipart, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::api::access_control_helpers::{
    build_access_control_service, build_policy_evaluation_service,
};
use crate::api::connection_authz_attributes::{
    connection_target_authz_attributes, connection_target_authz_attributes_for,
    resolve_connection_target_facets,
};
use crate::api::connections_helpers::build_connection_service;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{RequestContextExtractor, build_request_context};
use crate::api::rest::routes::validation::parse_uuid;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::connections::application::{RelayHealthState, RelayLegHealthSnapshot};
use crate::contexts::connections::domain::{
    ConnectionProtocol, ConnectionSession, ConnectionStatus, ProxyNode, parse_connection_protocol,
    parse_connection_status, parse_optional_connection_protocol,
};
use crate::contexts::connections::infrastructure::persistence::{
    CONNECTION_SESSION_COLUMNS, CONNECTION_TEMPLATE_COLUMNS, ConnectionSessionFilter,
    ConnectionTemplateFilter,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::integrations::proxy::{
    ProxyLinkMetricsResponse, ProxyMetricsResponse, build_client_ws_url, client_ws_path,
    create_proxy_ws_token,
};
use crate::observability::{EventFields, LogLevel, log_error_event, log_event};

const LAUNCH_SOURCE_TEMPLATE: &str = "template";
const LAUNCH_SOURCE_PARAMS: &str = "params";
const LAUNCH_SOURCE_HOST_DEFAULT: &str = "host-default";
const CONTEXT_NONE: &str = "none";
const CONTEXT_UNKNOWN: &str = "unknown";
const USER_ANONYMOUS: &str = "anonymous";
const CREDENTIAL_POLICY_CONTEXT_FIRST: &str = "context_first";
const CREDENTIAL_POLICY_PERSONAL_FIRST: &str = "personal_first";
const SESSION_METADATA_ENTRY_ID_KEY: &str = "entryId";
const SESSION_METADATA_POLICY_KEY: &str = "credentialMergePolicy";
const SESSION_METADATA_TEMPLATE_ID_KEY: &str = "template_id";
const SESSION_HEALTH_STALE_AFTER_MS: i64 = 7_500;
const LEG_CLIENT_BACKEND: &str = "client_backend";
const LEG_BACKEND_PROXY: &str = "backend_proxy";
const LEG_PROXY_TARGET: &str = "proxy_target";
const LEG_BACKEND_TARGET: &str = "backend_target";
const PROXY_LINK_CLIENT_PROXY: &str = "client_proxy";
const PROXY_LINK_PROXY_PROTOCOL: &str = "proxy_protocol";
const HEALTH_STATE_CONNECTING: &str = "connecting";
const HEALTH_STATE_OK: &str = "ok";
const HEALTH_STATE_DEGRADED: &str = "degraded";
const HEALTH_STATE_DISCONNECTED: &str = "disconnected";
const HEALTH_STATE_FAILED: &str = "failed";
const HEALTH_STATE_UNKNOWN: &str = "unknown";
const ROUTE_MODE_DIRECT: &str = "direct";
const ROUTE_MODE_PROXIED: &str = "proxied";
const REASON_PROXY_METRICS_UNAVAILABLE: &str = "proxy_metrics_unavailable";
const REASON_PROXY_SESSION_MISSING: &str = "proxy_session_missing";

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateConnectionSessionRequest {
    pub target_id: String,
    pub protocol: String,
    pub template_id: Option<String>,
    pub proxy_node_id: Option<String>,
    pub params: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InternalConnectionSessionCreatePolicy {
    pub require_embedded_proxy: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionMetadataInput {
    #[serde(rename = "entryId", skip_serializing_if = "Option::is_none")]
    entry_id: Option<String>,
    #[serde(
        rename = "credentialMergePolicy",
        skip_serializing_if = "Option::is_none"
    )]
    credential_merge_policy: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnectionTemplateListQuery {
    pub ids: Option<Vec<String>>,
    pub protocol: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnectionSessionListQuery {
    pub target_id: Option<String>,
    pub user_id: Option<String>,
    /// Comma-separated connection protocols; matches any (`IN`).
    pub protocol: Option<String>,
    /// Comma-separated connection statuses; matches any (`IN`).
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProxyConnectionDetailsResponse {
    pub connection_id: String,
    pub websocket_url: String,
    pub token: String,
    pub token_ttl_seconds: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionResponse {
    pub id: String,
    pub target_id: String,
    pub user_id: String,
    pub protocol: String,
    pub status: String,
    pub proxy_id: Option<String>,
    pub proxy_node_id: Option<String>,
    pub proxy_node_name: Option<String>,
    pub proxy_node_slug: Option<String>,
    pub routing_source: String,
    pub client_ip: Option<String>,
    pub last_activity: String,
    pub session_metadata: Value,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionTemplateResponse {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub defaults: Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionTemplateListResponse {
    pub items: Vec<ConnectionTemplateResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionListResponse {
    pub items: Vec<ConnectionSessionResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreateConnectionSessionResponse {
    pub session: ConnectionSessionResponse,
    pub proxy: ProxyConnectionDetailsResponse,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResumeConnectionSessionResponse {
    pub session: ConnectionSessionResponse,
    pub proxy: ProxyConnectionDetailsResponse,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LiveConnectionHealthResponse {
    pub session_id: String,
    pub connection_type: String,
    pub route_mode: String,
    pub state: String,
    pub reason_code: Option<String>,
    pub worst_leg_id: Option<String>,
    pub proxy_node_name: Option<String>,
    pub updated_at: String,
    pub stale_after_ms: i64,
    pub legs: Vec<LiveConnectionLegHealthResponse>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LiveConnectionLegHealthResponse {
    pub id: String,
    pub state: String,
    pub latency_ms: Option<i64>,
    pub rx_bps: Option<i64>,
    pub tx_bps: Option<i64>,
    pub rx_bytes: Option<i64>,
    pub tx_bytes: Option<i64>,
    pub quality_label: Option<String>,
    pub quality_score: Option<i32>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionClientCredentialsResponse {
    pub session_id: String,
    pub protocol: String,
    pub credentials: Value,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnectionSessionFileListQuery {
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnectionSessionFilePathQuery {
    pub path: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ConnectionSessionFileDownloadQuery {
    pub path: String,
    pub archive: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileDeleteRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileMoveRequest {
    pub paths: Vec<String>,
    pub destination: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileEntryResponse {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: Option<u64>,
    pub modified_at: Option<String>,
    pub permissions: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileListResponse {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<ConnectionSessionFileEntryResponse>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileTransferResponse {
    pub path: String,
    pub bytes: u64,
    pub duration_ms: u128,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileDownloadPreflightResponse {
    pub path: String,
    pub file_name: String,
    pub bytes: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub entry_count: Option<u64>,
    pub kind: String,
    pub archive: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFilePreviewResponse {
    pub path: String,
    pub bytes: u64,
    pub preview_bytes: u64,
    pub truncated: bool,
    pub content: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileDeleteResponse {
    pub paths: Vec<String>,
    pub count: usize,
    pub duration_ms: u128,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionSessionFileMoveResponse {
    pub paths: Vec<String>,
    pub destination: String,
    pub count: usize,
    pub duration_ms: u128,
}

/// Open a connection session to a target.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions",
    tag = "connections",
    request_body = CreateConnectionSessionRequest,
    responses(
        (status = 200, description = "Connection session opened", body = CreateConnectionSessionResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Connection to the target is forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Connection session state conflicts with the request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Connection session creation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_connection_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    internal_policy: Option<Extension<InternalConnectionSessionCreatePolicy>>,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateConnectionSessionRequest>,
) -> impl IntoResponse {
    let auth_audit_context = Some(auth_context.0.clone());
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let correlation_id = request_context.correlation_id.clone();
    let request_ip = request_context.request_ip.clone();
    let request_user_id = request_context.user_id.map(|user_id| user_id.to_string());
    let params_keys = json_top_level_keys(payload.params.as_ref());
    let metadata_keys = json_top_level_keys(payload.metadata.as_ref());
    let template_id_raw = payload.template_id.clone();
    let launch_source = resolve_launch_source(template_id_raw.is_some(), payload.params.is_some());
    log_event(
        LogLevel::Debug,
        "connection.session.request.accepted",
        EventFields::default()
            .with_correlation_id(correlation_id.clone())
            .with_operation("create_connection_session")
            .with_resource("target", payload.target_id.clone())
            .with_context("protocol", payload.protocol.clone())
            .with_context("launch_source", launch_source)
            .with_context(
                "template_id",
                label_optional_string(template_id_raw.clone(), CONTEXT_NONE),
            )
            .with_context("params_keys", format!("{params_keys:?}"))
            .with_context("metadata_keys", format!("{metadata_keys:?}"))
            .with_context(
                "request_ip",
                label_optional_string(request_ip.clone(), CONTEXT_UNKNOWN),
            )
            .with_user_id(label_optional_string(
                request_user_id.clone(),
                USER_ANONYMOUS,
            )),
    );

    // Authentication first (cheap, no DB) so an anonymous caller is rejected
    // before any target lookup.
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let target_id = match Uuid::parse_str(&payload.target_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid target_id: {}",
                error
            )));
        }
    };

    let protocol = match parse_connection_protocol(&payload.protocol) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::Validation(error.to_string())),
    };
    if protocol == ConnectionProtocol::App {
        return handle_app_error(AppError::Validation(
            "APP protocol does not create connection sessions; use direct app launch flow"
                .to_string(),
        ));
    }
    let normalized_params = match normalize_launch_params(&protocol, payload.params.clone()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    // Resolve the target's inventory facets BEFORE authorizing so the connect
    // decision can scope on the device/VM's role/site/tenant/platform/status and
    // the requested protocol. The facets come from the resolved
    // target object, never from caller input. This is also where the target's
    // existence is confirmed, so a non-existent target 404s here.
    //
    // Per-target, attribute-aware connect authorization. The resource id scopes
    // resource-pattern policies to THIS target (closing the former type-level
    // `None` gap), and the resolved facets + protocol let attribute constraints
    // (e.g. "role_id = RU3 AND protocol = vnc") apply.
    //
    // Use the canonical `protocol.as_str()` from the parsed enum — the SAME
    // value that governs the launched session below — not the raw
    // `payload.protocol`, so the authorized protocol equals the launched
    // protocol by construction (not by a coincidental normalization match that a
    // future protocol alias could break).
    //
    // NOTE (accepted low-severity trade-off): facet resolution now runs before
    // this check, so an authenticated caller with no Connect grant can learn a
    // target's existence (404) before the 403. Targets are generally READ-able,
    // the 404 is existence-only, and restoring a coarse pre-check here would
    // false-deny users whose only Connect grant is attribute-constrained (no
    // attributes resolved yet → constraint always fails).
    if let Err(error) =
        authorize_connect_target(&state, &request_context, target_id, &protocol).await
    {
        return handle_app_error(error);
    }

    let template_id = match parse_optional_uuid(payload.template_id.as_deref(), "template_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    log_event(
        LogLevel::Debug,
        "connection.session.inputs.resolved",
        EventFields::default()
            .with_correlation_id(correlation_id.clone())
            .with_operation("create_connection_session")
            .with_resource("target", target_id.to_string())
            .with_context("protocol", protocol.as_str().to_string())
            .with_context("template_defaults", template_id.is_some().to_string())
            .with_context("params_supplied", payload.params.is_some().to_string()),
    );

    let service = match build_connection_service(&state) {
        Ok(service) => service,
        Err(error) => return handle_app_error(error),
    };

    let session_metadata =
        match merge_session_metadata(payload.metadata.clone(), payload.template_id.clone()) {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let requested_proxy_node_id =
        match parse_optional_uuid(payload.proxy_node_id.as_deref(), "proxy_node_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let access_control = build_access_control_service(&state);
    let (audit_auth_method, audit_api_token_id) =
        audit_token_identifiers(auth_audit_context.as_ref());
    let session = match service
        .create_session(
            crate::contexts::connections::application::CreateSessionInput {
                target_id,
                user_id,
                protocol: protocol.clone(),
                client_ip: request_context.request_ip.clone(),
                audit_auth_method,
                audit_api_token_id,
                session_metadata: Some(session_metadata),
                template_id,
                protocol_params: normalized_params,
                requested_proxy_node_id,
                require_embedded_proxy: internal_policy
                    .as_ref()
                    .is_some_and(|policy| policy.require_embedded_proxy),
            },
            Some(access_control.as_ref()),
            Some(&request_context),
        )
        .await
    {
        Ok(session) => session,
        Err(error) => {
            log_error_event(
                "connection.session.create.failed",
                "connection_service_error",
                error.to_string(),
                EventFields::default()
                    .with_correlation_id(correlation_id.clone())
                    .with_operation("create_connection_session")
                    .with_resource("target", target_id.to_string())
                    .with_user_id(user_id.to_string())
                    .with_context("protocol", protocol.as_str().to_string()),
            );
            return handle_app_error(error);
        }
    };

    let response = match build_create_session_response(&state, &headers, &session).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    log_event(
        LogLevel::Debug,
        "connection.session.create.completed",
        EventFields::default()
            .with_correlation_id(correlation_id)
            .with_operation("create_connection_session")
            .with_resource("connection", response.session.id.clone())
            .with_status("ok")
            .with_context("target_id", response.session.target_id.clone())
            .with_context("protocol", response.session.protocol.clone())
            .with_context(
                "proxy_id",
                label_optional_string(response.session.proxy_id.clone(), CONTEXT_NONE),
            )
            .with_context(
                "client_ip",
                label_optional_string(response.session.client_ip.clone(), CONTEXT_UNKNOWN),
            ),
    );

    (StatusCode::OK, Json(response)).into_response()
}

/// Launch source label derived from the request shape.
fn resolve_launch_source(has_template: bool, has_params: bool) -> &'static str {
    if has_template {
        LAUNCH_SOURCE_TEMPLATE
    } else if has_params {
        LAUNCH_SOURCE_PARAMS
    } else {
        LAUNCH_SOURCE_HOST_DEFAULT
    }
}

/// Audit identifiers derived from the caller's authentication context.
fn audit_token_identifiers(context: Option<&AuthContext>) -> (Option<String>, Option<Uuid>) {
    let Some(context) = context else {
        return (None, None);
    };
    let auth_method =
        (!context.auth_method.trim().is_empty()).then(|| context.auth_method.trim().to_string());
    let api_token_id = context
        .api_token_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok());
    (auth_method, api_token_id)
}

/// Resolve the target's inventory facets and authorize a connect to it.
async fn authorize_connect_target(
    state: &AppState,
    request_context: &RequestContext,
    target_id: Uuid,
    protocol: &ConnectionProtocol,
) -> Result<(), AppError> {
    let target_facets = match resolve_connection_target_facets(state, target_id).await {
        Ok(Some(facets)) => facets,
        Ok(None) => {
            return Err(AppError::NotFound(
                "Target device or virtual machine not found".to_string(),
            ));
        }
        Err(error) => return Err(error),
    };
    let connect_resource_id = format!("connection_target:{target_id}");
    let connect_attributes =
        connection_target_authz_attributes(&target_facets, Some(protocol.as_str()));
    build_policy_evaluation_service(state)
        .ensure_access_with_attributes(
            request_context,
            Action::Connect,
            ResourceType::ConnectionTarget,
            Some(connect_resource_id.as_str()),
            Some(connect_attributes),
        )
        .await
}

/// Proxy node display labels for a session that runs through a proxy node.
async fn resolve_proxy_node_labels(
    state: &AppState,
    proxy_node_id: Uuid,
) -> Result<(Option<String>, Option<String>), AppError> {
    let proxy_node = state
        .services
        .connection
        .get_proxy_node(proxy_node_id)
        .await?;
    Ok((
        proxy_node.as_ref().map(|node| node.name.clone()),
        proxy_node.as_ref().map(|node| node.slug.clone()),
    ))
}

/// Build the create-session response, attaching proxy node metadata last.
async fn build_create_session_response(
    state: &AppState,
    headers: &HeaderMap,
    session: &ConnectionSession,
) -> Result<CreateConnectionSessionResponse, AppError> {
    let proxy = build_session_proxy_details(state, headers, session).await?;
    let mut response = CreateConnectionSessionResponse {
        session: map_session(session),
        proxy,
    };
    if let Some(proxy_node_id) = session.proxy_node_id {
        let (name, slug) = resolve_proxy_node_labels(state, proxy_node_id).await?;
        response.session.proxy_node_name = name;
        response.session.proxy_node_slug = slug;
    }
    Ok(response)
}

/// Resume an active connection session without creating a new remote session.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/resume",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection session ID"),
    ),
    responses(
        (status = 200, description = "Connection session resume details", body = ResumeConnectionSessionResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn resume_connection_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let actor_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid session id: {}",
                error
            )));
        }
    };

    let service = state.services.connection.clone();
    let session = match service.require_session(session_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if session.user_id != actor_id {
        return handle_app_error(AppError::Forbidden(
            "Connection session belongs to another user".to_string(),
        ));
    }
    if !matches!(session.status, ConnectionStatus::Active) {
        return handle_app_error(AppError::NotFound(
            "Connection session is no longer active".to_string(),
        ));
    }

    // Re-check Connect with the SAME attribute context the create site used
    // (facets + the session's protocol), so a user whose only Connect grant is
    // attribute-constrained can resume the session they were allowed to open.
    let target_resource_id = format!("connection_target:{}", session.target_id);
    let session_attributes = match connection_target_authz_attributes_for(
        &state,
        session.target_id,
        Some(session.protocol.as_str()),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access_with_attributes(
            &request_context,
            Action::Connect,
            ResourceType::ConnectionTarget,
            Some(target_resource_id.as_str()),
            Some(session_attributes),
        )
        .await
    {
        return handle_app_error(error);
    }

    let mut response = ResumeConnectionSessionResponse {
        session: map_session(&session),
        proxy: match build_session_proxy_details(&state, &headers, &session).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
    };
    if let Some(proxy_node_id) = session.proxy_node_id {
        let proxy_node = match service.get_proxy_node(proxy_node_id).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
        response.session.proxy_node_name = proxy_node.as_ref().map(|node| node.name.clone());
        response.session.proxy_node_slug = proxy_node.as_ref().map(|node| node.slug.clone());
    }

    (StatusCode::OK, Json(response)).into_response()
}

/// Get live connection health for a connection session.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions/{id}/health",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection session ID"),
    ),
    responses(
        (status = 200, description = "Live connection health for the session", body = LiveConnectionHealthResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Forbidden", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_connection_session_health(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let actor_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid session id: {}",
                error
            )));
        }
    };

    let service = state.services.connection.clone();
    let session = match service.require_session(session_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if session.user_id != actor_id {
        return handle_app_error(AppError::Forbidden(
            "Connection session belongs to another user".to_string(),
        ));
    }

    // Same attribute context as the create site — see the resume handler note.
    let target_resource_id = format!("connection_target:{}", session.target_id);
    let session_attributes = match connection_target_authz_attributes_for(
        &state,
        session.target_id,
        Some(session.protocol.as_str()),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access_with_attributes(
            &request_context,
            Action::Connect,
            ResourceType::ConnectionTarget,
            Some(target_resource_id.as_str()),
            Some(session_attributes),
        )
        .await
    {
        return handle_app_error(error);
    }

    let proxy_metrics_result = if session.proxy_id.is_some() {
        service.get_connection_metrics(session.id).await
    } else {
        Ok(None)
    };
    let proxy_metrics_reason = match &proxy_metrics_result {
        Ok(Some(_)) => None,
        Ok(None) if session.proxy_id.is_none() => Some(REASON_PROXY_SESSION_MISSING.to_string()),
        Ok(None) => Some(REASON_PROXY_METRICS_UNAVAILABLE.to_string()),
        Err(_) => Some(REASON_PROXY_METRICS_UNAVAILABLE.to_string()),
    };
    let proxy_metrics = proxy_metrics_result.ok().flatten();
    let backend_proxy_latency_ms = proxy_metrics
        .as_ref()
        .and_then(|sample| sample.request_latency_ms);

    let proxy_node_name = if let Some(proxy_node_id) = session.proxy_node_id {
        optional_proxy_node_name_from_lookup(
            session.id,
            proxy_node_id,
            service.get_proxy_node(proxy_node_id).await,
        )
    } else {
        None
    };

    let relay_snapshot = state.session_relay_health.snapshot(session.id).await;
    let response = build_live_connection_health_response(
        &session,
        relay_snapshot,
        proxy_metrics.as_ref().map(|sample| &sample.metrics),
        backend_proxy_latency_ms,
        proxy_metrics_reason,
        proxy_node_name,
    );

    (StatusCode::OK, Json(response)).into_response()
}

/// Get short-lived client credentials for a connection session.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions/{id}/client-credentials",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection session ID"),
    ),
    responses(
        (status = 200, description = "Client credentials for the connection session", body = ConnectionSessionClientCredentialsResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_connection_session_client_credentials(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);

    let actor_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let session_id = match Uuid::parse_str(&session_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid session id: {}",
                error
            )));
        }
    };

    let service = state.services.connection.clone();
    let session = match service.require_session(session_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if session.user_id != actor_id || session.status.is_terminal() {
        return handle_app_error(AppError::NotFound(
            "Connection session not found".to_string(),
        ));
    }

    // Same attribute context as the create site — see the resume handler note.
    // Both the Connect and the Secret leg get the attributes, so an operator
    // can scope credential exposure by site/role/protocol too.
    let target_resource_id = format!("connection_target:{}", session.target_id);
    let session_attributes = match connection_target_authz_attributes_for(
        &state,
        session.target_id,
        Some(session.protocol.as_str()),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access_with_attributes(
            &request_context,
            Action::Connect,
            ResourceType::ConnectionTarget,
            Some(target_resource_id.as_str()),
            Some(session_attributes.clone()),
        )
        .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = policy_service
        .ensure_access_with_attributes(
            &request_context,
            Action::Secret,
            ResourceType::ConnectionTarget,
            Some(target_resource_id.as_str()),
            Some(session_attributes),
        )
        .await
    {
        return handle_app_error(error);
    }

    let access_control = build_access_control_service(&state);
    let credentials = match service
        .resolve_session_client_credentials(
            session_id,
            actor_id,
            Some(access_control.as_ref()),
            Some(&request_context),
        )
        .await
    {
        Ok(map) => Value::Object(map),
        Err(error) => return handle_app_error(error),
    };

    let response = ConnectionSessionClientCredentialsResponse {
        session_id: session_id.to_string(),
        protocol: session.protocol.as_str().to_string(),
        credentials,
    };
    (
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(response),
    )
        .into_response()
}

/// List files visible from an active SSH connection session.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions/{id}/files",
    tag = "connections",
    params(
        ConnectionSessionFileListQuery,
        ("id" = String, Path, description = "Connection session id"),
    ),
    operation_id = "list_connection_session_files",
    responses(
        (status = 200, description = "Remote directory listing", body = ConnectionSessionFileListResponse),
        (status = "4XX", description = "File list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "File list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connection_session_files(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<ConnectionSessionFileListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = match parse_uuid(&session_id, "session id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let proxy_state = match state.proxy_state.clone() {
        Some(value) => value,
        None => {
            return handle_app_error(AppError::External(
                "SSH file tools require embedded proxy mode".to_string(),
            ));
        }
    };
    let proxy_connection_id =
        match require_authorized_active_ssh_session(&state, &request_context, session_id).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };

    let path = query.path.unwrap_or_else(|| ".".to_string());
    match crate::proxy::agent_ssh_list_dir(proxy_state, proxy_connection_id, path).await {
        Ok(list) => (StatusCode::OK, Json(map_ssh_file_list(list))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Upload one file to an active SSH connection session.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/files/upload",
    tag = "connections",
    params(
        ConnectionSessionFilePathQuery,
        ("id" = String, Path, description = "Connection session id"),
    ),
    operation_id = "upload_connection_session_file",
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "File uploaded", body = ConnectionSessionFileTransferResponse),
        (status = "4XX", description = "Upload request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Upload request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn upload_connection_session_file(
    State(state): State<AppState>,
    context: RequestContextExtractor,
    Path(session_id): Path<String>,
    Query(query): Query<ConnectionSessionFilePathQuery>,
    multipart: Multipart,
) -> impl IntoResponse {
    match upload_connection_session_file_inner(state, context.0, session_id, query, multipart).await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Ensure a remote directory exists in an active SSH connection session.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/files/mkdir",
    tag = "connections",
    params(
        ConnectionSessionFilePathQuery,
        ("id" = String, Path, description = "Connection session id"),
    ),
    operation_id = "ensure_connection_session_directory",
    responses(
        (status = 200, description = "Remote directory ensured", body = ConnectionSessionFileTransferResponse),
        (status = "4XX", description = "Mkdir request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mkdir request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn ensure_connection_session_directory(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<ConnectionSessionFilePathQuery>,
) -> impl IntoResponse {
    match ensure_connection_session_directory_inner(
        state,
        request_id,
        auth_context,
        headers,
        session_id,
        query,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Preview a text file from an active SSH connection session.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions/{id}/files/preview",
    tag = "connections",
    params(
        ConnectionSessionFilePathQuery,
        ("id" = String, Path, description = "Connection session id"),
    ),
    operation_id = "preview_connection_session_file",
    responses(
        (status = 200, description = "Remote text file preview", body = ConnectionSessionFilePreviewResponse),
        (status = "4XX", description = "Preview request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Preview request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_connection_session_file(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<ConnectionSessionFilePathQuery>,
) -> impl IntoResponse {
    match preview_connection_session_file_inner(
        state,
        request_id,
        auth_context,
        headers,
        session_id,
        query,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Delete files or directories from an active SSH connection session.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/files/delete",
    tag = "connections",
    params(("id" = String, Path, description = "Connection session id")),
    operation_id = "delete_connection_session_files",
    request_body = ConnectionSessionFileDeleteRequest,
    responses(
        (status = 200, description = "Remote paths deleted", body = ConnectionSessionFileDeleteResponse),
        (status = "4XX", description = "Delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_connection_session_files(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ConnectionSessionFileDeleteRequest>,
) -> impl IntoResponse {
    match delete_connection_session_files_inner(
        state,
        request_id,
        Some(auth_context),
        headers,
        session_id,
        payload,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Move files or directories in an active SSH connection session.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/files/move",
    tag = "connections",
    params(("id" = String, Path, description = "Connection session id")),
    operation_id = "move_connection_session_files",
    request_body = ConnectionSessionFileMoveRequest,
    responses(
        (status = 200, description = "Remote paths moved", body = ConnectionSessionFileMoveResponse),
        (status = "4XX", description = "Move request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Move request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn move_connection_session_files(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ConnectionSessionFileMoveRequest>,
) -> impl IntoResponse {
    match move_connection_session_files_inner(
        state,
        request_id,
        Some(auth_context),
        headers,
        session_id,
        payload,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Inspect a remote SSH file before browser-managed download starts.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions/{id}/files/download/preflight",
    tag = "connections",
    params(
        ConnectionSessionFileDownloadQuery,
        ("id" = String, Path, description = "Connection session id"),
    ),
    operation_id = "preflight_connection_session_file_download",
    responses(
        (status = 200, description = "Download preflight metadata", body = ConnectionSessionFileDownloadPreflightResponse),
        (status = "4XX", description = "Download preflight request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Download preflight request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preflight_connection_session_file_download(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<ConnectionSessionFileDownloadQuery>,
) -> impl IntoResponse {
    match preflight_connection_session_file_download_inner(
        state,
        request_id,
        auth_context,
        headers,
        session_id,
        query,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// Download one file from an active SSH connection session.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions/{id}/files/download",
    tag = "connections",
    params(
        ConnectionSessionFileDownloadQuery,
        ("id" = String, Path, description = "Connection session id"),
    ),
    operation_id = "download_connection_session_file",
    responses(
        (status = 200, description = "File or directory archive download stream", content_type = "application/octet-stream", body = Vec<u8>),
        (status = "4XX", description = "Download request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Download request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn download_connection_session_file(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<ConnectionSessionFileDownloadQuery>,
) -> impl IntoResponse {
    match download_connection_session_file_inner(
        state,
        request_id,
        auth_context,
        headers,
        session_id,
        query,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => handle_app_error(error),
    }
}

/// List connection templates.
#[utoipa::path(
    get,
    path = "/api/v1/connections/templates",
    tag = "connections",
    params(ConnectionTemplateListQuery),
    responses(
        (status = 200, description = "Connection templates", body = ConnectionTemplateListResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connection_templates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConnectionTemplateListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Read,
            ResourceType::ConnectionSession,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }

    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let filter = match map_template_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    let templates = match service
        .list_templates(filter.clone(), Some(limit), Some(offset))
        .await
    {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };
    let total = match service.count_templates(filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let response = ConnectionTemplateListResponse {
        items: templates.into_iter().map(map_template).collect(),
        total,
        limit,
        offset,
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// List connection sessions.
#[utoipa::path(
    get,
    path = "/api/v1/connections/sessions",
    tag = "connections",
    params(ConnectionSessionListQuery),
    responses(
        (status = 200, description = "Connection sessions", body = ConnectionSessionListResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connection_sessions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ConnectionSessionListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Read,
            ResourceType::ConnectionSession,
            None,
        )
        .await
    {
        return handle_app_error(error);
    }

    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let filter = match map_session_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.connection.clone();
    let sessions = match service
        .list_sessions(filter.clone(), Some(limit), Some(offset))
        .await
    {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };
    let total = match service.count_sessions(filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let response = ConnectionSessionListResponse {
        items: sessions.iter().map(map_session).collect(),
        total,
        limit,
        offset,
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Disconnect a connection session, leaving it resumable.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/disconnect",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection session ID"),
    ),
    responses(
        (status = 200, description = "Connection session disconnected", body = ConnectionSessionResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn disconnect_connection_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);

    let session_id = match Uuid::parse_str(&session_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid session id: {}",
                error
            )));
        }
    };

    let resource_id = connection_session_resource_id(session_id);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Update,
            ResourceType::ConnectionSession,
            Some(resource_id.as_str()),
        )
        .await
    {
        return handle_app_error(error);
    }

    let actor_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = match build_connection_service(&state) {
        Ok(service) => service,
        Err(error) => return handle_app_error(error),
    };
    match service.disconnect_session(session_id, actor_id).await {
        Ok(session) => (StatusCode::OK, Json(map_session(&session))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Terminate a connection session.
#[utoipa::path(
    post,
    path = "/api/v1/connections/sessions/{id}/terminate",
    tag = "connections",
    params(
        ("id" = String, Path, description = "Connection session ID"),
    ),
    responses(
        (status = 200, description = "Connection session terminated", body = ConnectionSessionResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn terminate_connection_session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let policy_service = build_policy_evaluation_service(&state);

    let session_id = match Uuid::parse_str(&session_id) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Validation(format!(
                "Invalid session id: {}",
                error
            )));
        }
    };

    let resource_id = connection_session_resource_id(session_id);
    if let Err(error) = policy_service
        .ensure_access(
            &request_context,
            Action::Update,
            ResourceType::ConnectionSession,
            Some(resource_id.as_str()),
        )
        .await
    {
        return handle_app_error(error);
    }

    let actor_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = match build_connection_service(&state) {
        Ok(service) => service,
        Err(error) => return handle_app_error(error),
    };
    match service.terminate_session(session_id, actor_id).await {
        Ok(session) => {
            state.session_relay_health.remove(session_id).await;
            (StatusCode::OK, Json(map_session(&session))).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

async fn upload_connection_session_file_inner(
    state: AppState,
    request_context: RequestContext,
    session_id: String,
    query: ConnectionSessionFilePathQuery,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;
    let max_upload_bytes = state.settings.proxy.ssh_file_transfer.max_upload_bytes;

    let temp_dir = tempfile::Builder::new()
        .prefix("piping-upload-")
        .tempdir()
        .map_err(|error| {
            AppError::External(format!("Failed to create upload temp dir: {error}"))
        })?;
    let local_path = temp_dir.path().join("payload");
    let mut output = tokio::fs::File::create(&local_path)
        .await
        .map_err(|error| {
            AppError::External(format!("Failed to create upload temp file: {error}"))
        })?;
    let mut saw_file = false;
    let mut bytes = 0_u64;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Validation(format!("Invalid multipart upload: {error}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        if saw_file {
            return Err(AppError::Validation(
                "Only one file field is accepted per upload request".to_string(),
            ));
        }
        saw_file = true;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| AppError::Validation(format!("Invalid multipart upload: {error}")))?
        {
            let next_bytes = bytes.saturating_add(chunk.len() as u64);
            if next_bytes > max_upload_bytes {
                return Err(AppError::PayloadTooLarge(format!(
                    "Upload exceeds configured limit of {max_upload_bytes} bytes"
                )));
            }
            output.write_all(&chunk).await.map_err(|error| {
                AppError::External(format!("Failed to write upload temp file: {error}"))
            })?;
            bytes = next_bytes;
        }
    }
    output.flush().await.map_err(|error| {
        AppError::External(format!("Failed to flush upload temp file: {error}"))
    })?;
    drop(output);

    if !saw_file {
        return Err(AppError::Validation(
            "Multipart upload must include a file field".to_string(),
        ));
    }

    let transfer = crate::proxy::agent_ssh_upload_file(
        proxy_state,
        proxy_connection_id,
        local_path,
        query.path.clone(),
    )
    .await?;
    drop(temp_dir);

    let response = ConnectionSessionFileTransferResponse {
        path: query.path,
        bytes: transfer.bytes.max(bytes),
        duration_ms: transfer.duration_ms,
    };
    Ok((StatusCode::OK, Json(response)).into_response())
}

async fn ensure_connection_session_directory_inner(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    session_id: String,
    query: ConnectionSessionFilePathQuery,
) -> Result<Response, AppError> {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;

    let transfer =
        crate::proxy::agent_ssh_ensure_dir(proxy_state, proxy_connection_id, query.path.clone())
            .await?;

    Ok((
        StatusCode::OK,
        Json(ConnectionSessionFileTransferResponse {
            path: query.path,
            bytes: transfer.bytes,
            duration_ms: transfer.duration_ms,
        }),
    )
        .into_response())
}

async fn preview_connection_session_file_inner(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    session_id: String,
    query: ConnectionSessionFilePathQuery,
) -> Result<Response, AppError> {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;

    let preview =
        crate::proxy::agent_ssh_preview_file(proxy_state, proxy_connection_id, query.path).await?;

    Ok((
        StatusCode::OK,
        Json(ConnectionSessionFilePreviewResponse {
            path: preview.path,
            bytes: preview.bytes,
            preview_bytes: preview.preview_bytes,
            truncated: preview.truncated,
            content: preview.content,
        }),
    )
        .into_response())
}

async fn delete_connection_session_files_inner(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    session_id: String,
    payload: ConnectionSessionFileDeleteRequest,
) -> Result<Response, AppError> {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;

    let result =
        crate::proxy::agent_ssh_delete_files(proxy_state, proxy_connection_id, payload.paths)
            .await?;
    let count = result.paths.len();
    Ok((
        StatusCode::OK,
        Json(ConnectionSessionFileDeleteResponse {
            paths: result.paths,
            count,
            duration_ms: result.duration_ms,
        }),
    )
        .into_response())
}

async fn move_connection_session_files_inner(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    session_id: String,
    payload: ConnectionSessionFileMoveRequest,
) -> Result<Response, AppError> {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;

    let result = crate::proxy::agent_ssh_move_files(
        proxy_state,
        proxy_connection_id,
        payload.paths,
        payload.destination,
    )
    .await?;
    let count = result.paths.len();
    Ok((
        StatusCode::OK,
        Json(ConnectionSessionFileMoveResponse {
            paths: result.paths,
            destination: result.destination,
            count,
            duration_ms: result.duration_ms,
        }),
    )
        .into_response())
}

async fn preflight_connection_session_file_download_inner(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    session_id: String,
    query: ConnectionSessionFileDownloadQuery,
) -> Result<Response, AppError> {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;
    let max_download_bytes = state.settings.proxy.ssh_file_transfer.max_download_bytes;
    let archive = query.archive.unwrap_or(false);

    let preflight = crate::proxy::agent_ssh_preflight_download_file(
        proxy_state,
        proxy_connection_id,
        query.path,
        max_download_bytes,
        archive,
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(ConnectionSessionFileDownloadPreflightResponse {
            path: preflight.path,
            file_name: preflight.file_name,
            bytes: preflight.bytes,
            logical_bytes: preflight.logical_bytes,
            entry_count: preflight.entry_count,
            kind: preflight.kind,
            archive: preflight.archive,
        }),
    )
        .into_response())
}

async fn download_connection_session_file_inner(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    session_id: String,
    query: ConnectionSessionFileDownloadQuery,
) -> Result<Response, AppError> {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let session_id = parse_uuid(&session_id, "session id")?;
    let proxy_state = state.proxy_state.clone().ok_or_else(|| {
        AppError::External("SSH file tools require embedded proxy mode".to_string())
    })?;
    let proxy_connection_id =
        require_authorized_active_ssh_session(&state, &request_context, session_id).await?;
    let max_download_bytes = state.settings.proxy.ssh_file_transfer.max_download_bytes;
    let archive = query.archive.unwrap_or(false);

    let temp_dir = tempfile::Builder::new()
        .prefix("piping-download-")
        .tempdir()
        .map_err(|error| {
            AppError::External(format!("Failed to create download temp dir: {error}"))
        })?;
    let local_path = temp_dir.path().join("payload");
    let transfer = crate::proxy::agent_ssh_download_file(
        proxy_state,
        proxy_connection_id,
        query.path.clone(),
        local_path.clone(),
        max_download_bytes,
        archive,
    )
    .await?;

    let file = tokio::fs::File::open(&local_path).await.map_err(|error| {
        AppError::External(format!("Failed to open downloaded temp file: {error}"))
    })?;
    let _ = tokio::fs::remove_file(&local_path).await;
    drop(temp_dir);

    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let file_name = crate::proxy::agent_ssh_download_file_name(&query.path, archive);
    let content_disposition = format!(
        "attachment; filename=\"{}\"",
        sanitize_content_disposition_filename(&file_name)
    );
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition)
            .map_err(|error| AppError::External(format!("Invalid download header: {error}")))?,
    );
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&transfer.bytes.to_string()).map_err(|error| {
            AppError::External(format!("Invalid download size header: {error}"))
        })?,
    );
    Ok(response)
}

async fn require_authorized_active_ssh_session(
    state: &AppState,
    request_context: &RequestContext,
    session_id: Uuid,
) -> Result<Uuid, AppError> {
    let actor_id = request_context.require_authenticated_user_id()?;
    let session = state
        .services
        .connection
        .require_session(session_id)
        .await?;
    if session.user_id != actor_id {
        return Err(AppError::Forbidden(
            "Connection session belongs to another user".to_string(),
        ));
    }
    if !matches!(session.status, ConnectionStatus::Active) {
        return Err(AppError::NotFound(
            "Connection session is no longer active".to_string(),
        ));
    }
    if session.protocol != ConnectionProtocol::Ssh {
        return Err(AppError::Validation(
            "File tools are only available for SSH sessions".to_string(),
        ));
    }

    // Same attribute context as the other session-lifecycle sites (facets +
    // the session's protocol), so attribute-scoped Connect policies — allow
    // AND deny — govern the file tools exactly as they govern resume/health.
    let target_resource_id = format!("connection_target:{}", session.target_id);
    let session_attributes = connection_target_authz_attributes_for(
        state,
        session.target_id,
        Some(session.protocol.as_str()),
    )
    .await?;
    build_policy_evaluation_service(state)
        .ensure_access_with_attributes(
            request_context,
            Action::Connect,
            ResourceType::ConnectionTarget,
            Some(target_resource_id.as_str()),
            Some(session_attributes),
        )
        .await?;

    session.proxy_id.ok_or_else(|| {
        AppError::NotFound("Connection session has no active proxy connection".to_string())
    })
}

fn map_ssh_file_list(list: crate::proxy::AgentSshFileList) -> ConnectionSessionFileListResponse {
    ConnectionSessionFileListResponse {
        path: list.path,
        parent: list.parent,
        entries: list
            .entries
            .into_iter()
            .map(|entry| ConnectionSessionFileEntryResponse {
                name: entry.name,
                path: entry.path,
                kind: entry.kind,
                size: entry.size,
                modified_at: entry.modified_at,
                permissions: entry.permissions,
            })
            .collect(),
    }
}

fn sanitize_content_disposition_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}

fn build_live_connection_health_response(
    session: &ConnectionSession,
    relay_snapshot: Option<RelayLegHealthSnapshot>,
    proxy_metrics: Option<&ProxyMetricsResponse>,
    backend_proxy_latency_ms: Option<i64>,
    proxy_metrics_reason: Option<String>,
    proxy_node_name: Option<String>,
) -> LiveConnectionHealthResponse {
    let route_mode = if session.proxy_node_id.is_some() {
        ROUTE_MODE_PROXIED
    } else {
        ROUTE_MODE_DIRECT
    };
    let terminal_state = terminal_health_state(&session.status);
    let client_backend = build_client_backend_leg(session, relay_snapshot.as_ref(), terminal_state);
    let proxy_overall_state =
        if should_apply_proxy_overall_connecting_state(session, &client_backend) {
            proxy_metrics.and_then(proxy_overall_connecting_state)
        } else {
            None
        };
    let promote_connecting_links_with_activity = matches!(session.status, ConnectionStatus::Active)
        && client_backend.state != HEALTH_STATE_CONNECTING;
    let proxy_link = |link_type: &str| {
        proxy_metrics.and_then(|metrics| {
            metrics
                .links
                .iter()
                .find(|link| link.link_type == link_type)
        })
    };

    let mut legs = vec![client_backend];
    if session.proxy_node_id.is_some() {
        legs.push(build_proxy_link_leg(ProxyLinkLeg {
            leg_id: LEG_BACKEND_PROXY,
            link: proxy_link(PROXY_LINK_CLIENT_PROXY),
            measured_latency_ms: backend_proxy_latency_ms,
            terminal_state,
            proxy_overall_state,
            fallback_state: fallback_remote_leg_state(
                &session.status,
                proxy_metrics_reason.as_deref(),
            ),
            promote_connecting_with_activity: promote_connecting_links_with_activity,
        }));
        legs.push(build_proxy_link_leg(ProxyLinkLeg {
            leg_id: LEG_PROXY_TARGET,
            link: proxy_link(PROXY_LINK_PROXY_PROTOCOL),
            measured_latency_ms: None,
            terminal_state,
            proxy_overall_state,
            fallback_state: fallback_remote_leg_state(
                &session.status,
                proxy_metrics_reason.as_deref(),
            ),
            promote_connecting_with_activity: promote_connecting_links_with_activity,
        }));
    } else {
        legs.push(build_proxy_link_leg(ProxyLinkLeg {
            leg_id: LEG_BACKEND_TARGET,
            link: proxy_link(PROXY_LINK_PROXY_PROTOCOL),
            measured_latency_ms: None,
            terminal_state,
            proxy_overall_state,
            fallback_state: fallback_remote_leg_state(
                &session.status,
                proxy_metrics_reason.as_deref(),
            ),
            promote_connecting_with_activity: promote_connecting_links_with_activity,
        }));
    }

    let (state, worst_leg_id) = summarize_live_health(&legs);
    let reason_code = session_reason_code(&session.status)
        .or_else(|| relay_snapshot.and_then(|snapshot| snapshot.reason_code))
        .or(proxy_metrics_reason);

    LiveConnectionHealthResponse {
        session_id: session.id.to_string(),
        connection_type: session.protocol.as_str().to_string(),
        route_mode: route_mode.to_string(),
        state,
        reason_code,
        worst_leg_id,
        proxy_node_name,
        updated_at: Utc::now().to_rfc3339(),
        stale_after_ms: SESSION_HEALTH_STALE_AFTER_MS,
        legs,
    }
}

fn build_client_backend_leg(
    session: &ConnectionSession,
    relay_snapshot: Option<&RelayLegHealthSnapshot>,
    terminal_state: Option<&'static str>,
) -> LiveConnectionLegHealthResponse {
    let state = terminal_state
        .or_else(|| relay_snapshot.map(|snapshot| relay_state_as_health_state(snapshot.state)))
        .unwrap_or(match session.status {
            ConnectionStatus::Pending => HEALTH_STATE_CONNECTING,
            ConnectionStatus::Active => HEALTH_STATE_UNKNOWN,
            ConnectionStatus::Disconnected | ConnectionStatus::Terminated => {
                HEALTH_STATE_DISCONNECTED
            }
            ConnectionStatus::Failed => HEALTH_STATE_FAILED,
        });

    LiveConnectionLegHealthResponse {
        id: LEG_CLIENT_BACKEND.to_string(),
        state: state.to_string(),
        latency_ms: None,
        rx_bps: relay_snapshot.and_then(|snapshot| snapshot.rx_bps),
        tx_bps: relay_snapshot.and_then(|snapshot| snapshot.tx_bps),
        rx_bytes: relay_snapshot.map(|snapshot| snapshot.rx_bytes),
        tx_bytes: relay_snapshot.map(|snapshot| snapshot.tx_bytes),
        quality_label: None,
        quality_score: None,
        message: None,
    }
}

/// Inputs for one proxy-link health leg.
struct ProxyLinkLeg<'a> {
    leg_id: &'a str,
    link: Option<&'a ProxyLinkMetricsResponse>,
    measured_latency_ms: Option<i64>,
    terminal_state: Option<&'static str>,
    proxy_overall_state: Option<&'static str>,
    fallback_state: &'static str,
    promote_connecting_with_activity: bool,
}

fn build_proxy_link_leg(leg: ProxyLinkLeg<'_>) -> LiveConnectionLegHealthResponse {
    let ProxyLinkLeg {
        leg_id,
        link,
        measured_latency_ms,
        terminal_state,
        proxy_overall_state,
        fallback_state,
        promote_connecting_with_activity,
    } = leg;
    let link_state =
        map_proxy_link_health_state(link, measured_latency_ms, promote_connecting_with_activity);
    let state = terminal_state
        .or_else(|| match link_state {
            Some(state) if state == HEALTH_STATE_FAILED || state == HEALTH_STATE_DISCONNECTED => {
                Some(state)
            }
            _ => proxy_overall_state,
        })
        .or(link_state)
        .unwrap_or(fallback_state)
        .to_string();
    LiveConnectionLegHealthResponse {
        id: leg_id.to_string(),
        state,
        latency_ms: normalize_latency_ms(measured_latency_ms)
            .or_else(|| normalize_latency_ms(link.and_then(|value| value.latency_ms))),
        rx_bps: link.and_then(|value| value.rx_bps),
        tx_bps: link.and_then(|value| value.tx_bps),
        rx_bytes: link.and_then(|value| value.rx_bytes),
        tx_bytes: link.and_then(|value| value.tx_bytes),
        quality_label: link
            .and_then(|value| normalize_quality_label(value.quality_label.as_deref())),
        quality_score: link.and_then(|value| value.quality_score),
        message: None,
    }
}

fn map_proxy_link_health_state(
    link: Option<&ProxyLinkMetricsResponse>,
    measured_latency_ms: Option<i64>,
    promote_connecting_with_activity: bool,
) -> Option<&'static str> {
    let link = link?;
    let state = map_proxy_link_state(link);
    if state == HEALTH_STATE_CONNECTING
        && promote_connecting_with_activity
        && proxy_link_has_open_activity(link, measured_latency_ms)
    {
        return Some(HEALTH_STATE_OK);
    }
    Some(state)
}

fn proxy_link_has_open_activity(
    link: &ProxyLinkMetricsResponse,
    measured_latency_ms: Option<i64>,
) -> bool {
    normalize_latency_ms(measured_latency_ms).is_some()
        || normalize_latency_ms(link.latency_ms).is_some()
        || positive_metric(link.rx_bytes)
        || positive_metric(link.tx_bytes)
        || positive_metric(link.rx_bps)
        || positive_metric(link.tx_bps)
}

fn positive_metric(value: Option<i64>) -> bool {
    matches!(value, Some(metric) if metric > 0)
}

fn normalize_latency_ms(value: Option<i64>) -> Option<i64> {
    value.filter(|&latency_ms| latency_ms > 0)
}

fn should_apply_proxy_overall_connecting_state(
    session: &ConnectionSession,
    client_backend: &LiveConnectionLegHealthResponse,
) -> bool {
    matches!(session.status, ConnectionStatus::Pending)
        || client_backend.state == HEALTH_STATE_CONNECTING
}

fn proxy_overall_connecting_state(metrics: &ProxyMetricsResponse) -> Option<&'static str> {
    if metrics.overall_status == "connecting" {
        Some(HEALTH_STATE_CONNECTING)
    } else {
        None
    }
}

fn optional_proxy_node_name_from_lookup(
    session_id: Uuid,
    proxy_node_id: Uuid,
    result: Result<Option<ProxyNode>, AppError>,
) -> Option<String> {
    match result {
        Ok(node) => node.map(|value| value.name),
        Err(error) => {
            log_error_event(
                "connection.session.health.proxy_node_lookup_failed",
                "proxy_node_lookup_failed",
                error.to_string(),
                EventFields::default()
                    .with_operation("get_connection_session_health")
                    .with_resource("connection_session", session_id.to_string())
                    .with_context("proxy_node_id", proxy_node_id.to_string()),
            );
            None
        }
    }
}

fn terminal_health_state(status: &ConnectionStatus) -> Option<&'static str> {
    match status {
        ConnectionStatus::Disconnected | ConnectionStatus::Terminated => {
            Some(HEALTH_STATE_DISCONNECTED)
        }
        ConnectionStatus::Failed => Some(HEALTH_STATE_FAILED),
        ConnectionStatus::Pending | ConnectionStatus::Active => None,
    }
}

fn fallback_remote_leg_state(
    status: &ConnectionStatus,
    metrics_reason: Option<&str>,
) -> &'static str {
    if let Some(state) = terminal_health_state(status) {
        return state;
    }
    if matches!(status, ConnectionStatus::Pending) {
        return HEALTH_STATE_CONNECTING;
    }
    if matches!(metrics_reason, Some(REASON_PROXY_METRICS_UNAVAILABLE)) {
        return HEALTH_STATE_UNKNOWN;
    }
    HEALTH_STATE_UNKNOWN
}

fn relay_state_as_health_state(state: RelayHealthState) -> &'static str {
    match state {
        RelayHealthState::Connecting => HEALTH_STATE_CONNECTING,
        RelayHealthState::Ok => HEALTH_STATE_OK,
        RelayHealthState::Disconnected => HEALTH_STATE_DISCONNECTED,
        RelayHealthState::Failed => HEALTH_STATE_FAILED,
    }
}

fn map_proxy_link_state(link: &ProxyLinkMetricsResponse) -> &'static str {
    match link.status.as_str() {
        "healthy" | "connected" => HEALTH_STATE_OK,
        "degraded" => HEALTH_STATE_DEGRADED,
        "unhealthy" | "error" | "failed" => HEALTH_STATE_FAILED,
        "disconnected" => HEALTH_STATE_DISCONNECTED,
        "connecting" => HEALTH_STATE_CONNECTING,
        _ => HEALTH_STATE_UNKNOWN,
    }
}

fn normalize_quality_label(value: Option<&str>) -> Option<String> {
    match value {
        Some("good") => Some("good".to_string()),
        Some("warning") => Some("warning".to_string()),
        Some("poor") => Some("poor".to_string()),
        _ => None,
    }
}

fn session_reason_code(status: &ConnectionStatus) -> Option<String> {
    match status {
        ConnectionStatus::Failed => Some("session_failed".to_string()),
        ConnectionStatus::Disconnected => Some("session_disconnected".to_string()),
        ConnectionStatus::Terminated => Some("session_terminated".to_string()),
        ConnectionStatus::Pending | ConnectionStatus::Active => None,
    }
}

fn summarize_live_health(legs: &[LiveConnectionLegHealthResponse]) -> (String, Option<String>) {
    let worst =
        legs.iter().fold(
            None::<&LiveConnectionLegHealthResponse>,
            |selected, leg| match selected {
                Some(current)
                    if health_state_rank(current.state.as_str())
                        >= health_state_rank(leg.state.as_str()) =>
                {
                    Some(current)
                }
                _ => Some(leg),
            },
        );
    let Some(worst) = worst else {
        return (HEALTH_STATE_UNKNOWN.to_string(), None);
    };
    let rank = health_state_rank(worst.state.as_str());
    let state = match worst.state.as_str() {
        HEALTH_STATE_FAILED => HEALTH_STATE_FAILED,
        HEALTH_STATE_DISCONNECTED => HEALTH_STATE_DISCONNECTED,
        HEALTH_STATE_DEGRADED => HEALTH_STATE_DEGRADED,
        HEALTH_STATE_CONNECTING => HEALTH_STATE_CONNECTING,
        HEALTH_STATE_UNKNOWN => HEALTH_STATE_UNKNOWN,
        HEALTH_STATE_OK => HEALTH_STATE_OK,
        _ => HEALTH_STATE_UNKNOWN,
    };
    let worst_leg_id = if rank == health_state_rank(HEALTH_STATE_OK) {
        None
    } else {
        Some(worst.id.clone())
    };
    (state.to_string(), worst_leg_id)
}

fn health_state_rank(state: &str) -> i32 {
    match state {
        HEALTH_STATE_FAILED => 5,
        HEALTH_STATE_DISCONNECTED => 4,
        HEALTH_STATE_DEGRADED => 3,
        HEALTH_STATE_CONNECTING => 2,
        HEALTH_STATE_UNKNOWN => 1,
        HEALTH_STATE_OK => 0,
        _ => 1,
    }
}

fn map_session(session: &ConnectionSession) -> ConnectionSessionResponse {
    ConnectionSessionResponse {
        id: session.id.to_string(),
        target_id: session.target_id.to_string(),
        user_id: session.user_id.to_string(),
        protocol: session.protocol.as_str().to_string(),
        status: session.status.as_str().to_string(),
        proxy_id: session.proxy_id.map(|value| value.to_string()),
        proxy_node_id: session.proxy_node_id.map(|value| value.to_string()),
        proxy_node_name: None,
        proxy_node_slug: None,
        routing_source: session.routing_source.as_str().to_string(),
        client_ip: session.client_ip.clone(),
        last_activity: session.last_activity.to_rfc3339(),
        session_metadata: session.session_metadata.clone(),
        started_at: session.started_at.to_rfc3339(),
        ended_at: session.ended_at.map(|value| value.to_rfc3339()),
    }
}

async fn build_session_proxy_details(
    state: &AppState,
    headers: &HeaderMap,
    session: &ConnectionSession,
) -> Result<ProxyConnectionDetailsResponse, AppError> {
    let settings = state.settings.as_ref();
    let route = state
        .services
        .connection
        .resolve_session_proxy_route(session)
        .await?;
    let credential = match route.credential {
        Some(value) => value,
        None => state.services.proxy_shared_secret_provider.get()?,
    };
    let token = create_proxy_ws_token(&credential.secret, session.id, session.user_id)?;
    // Prefer the public-facing base URL derived from incoming request headers
    // (X-Forwarded-Host / X-Forwarded-Proto) so browser-visible WebSocket URLs hit
    // the edge proxy, not the backend's internal bind. Fall back to the
    // internal bind when headers are missing (e.g. direct tests).
    let ws_base_url = crate::config::public_base_url_from_headers(headers)
        .unwrap_or_else(|| crate::config::effective_proxy_base_url(settings));
    let client_path = client_ws_path(&session.protocol, session.id)?;
    let websocket_url = build_client_ws_url(&ws_base_url, &client_path)?;
    Ok(ProxyConnectionDetailsResponse {
        connection_id: session.id.to_string(),
        websocket_url,
        token,
        token_ttl_seconds: settings.proxy.ws_token_ttl_seconds,
    })
}

fn map_template(
    template: crate::contexts::connections::domain::ConnectionTemplate,
) -> ConnectionTemplateResponse {
    ConnectionTemplateResponse {
        id: template.id.to_string(),
        name: template.name,
        protocol: template.protocol.as_str().to_string(),
        defaults: template.defaults,
    }
}

fn connection_session_resource_id(session_id: Uuid) -> String {
    format!("connection_session:{}", session_id)
}

fn merge_session_metadata(
    metadata: Option<Value>,
    template_id: Option<String>,
) -> Result<Value, AppError> {
    let metadata_input = match metadata.unwrap_or_else(|| json!({})) {
        Value::Null => SessionMetadataInput::default(),
        value => serde_json::from_value::<SessionMetadataInput>(value).map_err(|error| {
            AppError::Validation(format!("Session metadata is invalid: {}", error))
        })?,
    };

    let mut data = Map::new();
    if let Some(entry_id) = normalize_optional_string(metadata_input.entry_id) {
        data.insert(
            SESSION_METADATA_ENTRY_ID_KEY.to_string(),
            Value::String(entry_id),
        );
    }
    if let Some(policy) = normalize_optional_string(metadata_input.credential_merge_policy) {
        let normalized = policy.to_ascii_lowercase();
        if !is_supported_credential_merge_policy(&normalized) {
            return Err(AppError::Validation(format!(
                "session_metadata.credentialMergePolicy must be one of: {}, {}",
                CREDENTIAL_POLICY_CONTEXT_FIRST, CREDENTIAL_POLICY_PERSONAL_FIRST
            )));
        }
        data.insert(
            SESSION_METADATA_POLICY_KEY.to_string(),
            Value::String(normalized),
        );
    }
    if let Some(template_id) = template_id {
        data.insert(
            SESSION_METADATA_TEMPLATE_ID_KEY.to_string(),
            Value::String(template_id),
        );
    }
    Ok(Value::Object(data))
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn map_template_filter(
    query: &ConnectionTemplateListQuery,
) -> Result<ConnectionTemplateFilter, AppError> {
    let ids = match query.ids.as_ref() {
        Some(ids) => {
            let mut parsed = Vec::with_capacity(ids.len());
            for value in ids {
                parsed.push(parse_uuid(value, "template_id")?);
            }
            Some(parsed)
        }
        None => None,
    };
    Ok(ConnectionTemplateFilter {
        ids,
        protocol: parse_optional_protocol(query.protocol.clone())?,
        search: normalize_optional_string(query.search.clone()),
        sort: SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            CONNECTION_TEMPLATE_COLUMNS,
        )?,
    })
}

fn map_session_filter(
    query: &ConnectionSessionListQuery,
) -> Result<ConnectionSessionFilter, AppError> {
    Ok(ConnectionSessionFilter {
        target_id: parse_csv_uuids(query.target_id.as_deref(), "target_id")?,
        user_id: parse_csv_uuids(query.user_id.as_deref(), "user_id")?,
        protocol: parse_csv_protocols(query.protocol.as_deref())?,
        status: parse_csv_statuses(query.status.as_deref())?,
        sort: SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            CONNECTION_SESSION_COLUMNS,
        )?,
    })
}

/// Parse a comma-separated list of connection protocols into a deduplicated
/// vector. Empty/whitespace entries are skipped; an invalid protocol is a
/// client error.
fn parse_csv_protocols(raw: Option<&str>) -> Result<Vec<ConnectionProtocol>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<ConnectionProtocol> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let protocol = parse_connection_protocol(trimmed)
            .map_err(|error| AppError::Validation(error.to_string()))?;
        if !out
            .iter()
            .any(|existing| existing.as_str() == protocol.as_str())
        {
            out.push(protocol);
        }
    }
    Ok(out)
}

/// Parse a comma-separated list of connection statuses into a deduplicated
/// vector. Empty/whitespace entries are skipped; an invalid status is a client
/// error.
fn parse_csv_statuses(raw: Option<&str>) -> Result<Vec<ConnectionStatus>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<ConnectionStatus> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let status = parse_connection_status(trimmed)
            .map_err(|error| AppError::Validation(error.to_string()))?;
        if !out
            .iter()
            .any(|existing| existing.as_str() == status.as_str())
        {
            out.push(status);
        }
    }
    Ok(out)
}

fn json_top_level_keys(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Object(map)) => {
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            keys
        }
        _ => Vec::new(),
    }
}

fn label_optional_string(value: Option<String>, missing_label: &str) -> String {
    value.unwrap_or_else(|| missing_label.to_string())
}

fn parse_optional_uuid(value: Option<&str>, label: &str) -> Result<Option<Uuid>, AppError> {
    value.map(|raw| parse_uuid(raw, label)).transpose()
}

fn parse_optional_protocol(value: Option<String>) -> Result<Option<ConnectionProtocol>, AppError> {
    parse_optional_connection_protocol(normalize_optional_string(value))
        .map_err(|error| AppError::Validation(error.to_string()))
}

fn is_supported_credential_merge_policy(value: &str) -> bool {
    matches!(
        value,
        CREDENTIAL_POLICY_CONTEXT_FIRST | CREDENTIAL_POLICY_PERSONAL_FIRST
    )
}

#[cfg(test)]
mod tests {
    use super::{
        HEALTH_STATE_CONNECTING, HEALTH_STATE_DEGRADED, HEALTH_STATE_FAILED, HEALTH_STATE_OK,
        HEALTH_STATE_UNKNOWN, LEG_BACKEND_PROXY, LEG_BACKEND_TARGET, LEG_CLIENT_BACKEND,
        LEG_PROXY_TARGET, REASON_PROXY_METRICS_UNAVAILABLE, ROUTE_MODE_DIRECT, ROUTE_MODE_PROXIED,
        build_live_connection_health_response, merge_session_metadata, normalize_launch_params,
        optional_proxy_node_name_from_lookup,
    };
    use crate::contexts::connections::application::{RelayHealthState, RelayLegHealthSnapshot};
    use crate::contexts::connections::domain::{
        ConnectionProtocol, ConnectionSession, ConnectionStatus, RoutingSource,
    };
    use crate::errors::AppError;
    use crate::integrations::proxy::{ProxyLinkMetricsResponse, ProxyMetricsResponse};
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn assert_validation_error_contains<T: std::fmt::Debug>(
        result: Result<T, AppError>,
        needle: &str,
    ) {
        assert!(
            matches!(
                &result,
                Err(AppError::Validation(message)) if message.contains(needle)
            ),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn normalize_launch_params_rejects_unknown_key_for_ssh() {
        let result = normalize_launch_params(
            &ConnectionProtocol::Ssh,
            Some(json!({
                "host": "10.10.1.230",
                "port": 22,
                "retired_mode": true
            })),
        );

        assert_validation_error_contains(result, "unknown field");
    }

    #[test]
    fn normalize_launch_params_rejects_string_timeout() {
        let result = normalize_launch_params(
            &ConnectionProtocol::Ssh,
            Some(json!({
                "timeout": "5"
            })),
        );

        assert_validation_error_contains(result, "invalid type");
    }

    #[test]
    fn normalize_launch_params_accepts_rdp_canonical_shape() {
        let result = normalize_launch_params(
            &ConnectionProtocol::Rdp,
            Some(json!({
                "host": "10.10.1.230",
                "port": 3389,
                "credentials": {
                    "username": "operator",
                    "domain": "GRAD"
                },
                "options": {
                    "width": 1280,
                    "height": 720,
                    "ignore_cert": false,
                    "security": "nla",
                    "color_depth": 24,
                    "enable_drive": false
                }
            })),
        );

        assert!(
            matches!(&result, Ok(Some(_))),
            "canonical rdp params should pass: {result:?}"
        );
    }

    #[test]
    fn normalize_launch_params_rejects_rdp_options_inside_credentials() {
        let result = normalize_launch_params(
            &ConnectionProtocol::Rdp,
            Some(json!({
                "host": "10.10.1.230",
                "port": 3389,
                "credentials": {
                    "username": "operator",
                    "screen_resolution": "1280x720"
                }
            })),
        );

        assert_validation_error_contains(result, "unknown field");
    }

    #[test]
    fn normalize_launch_params_rejects_combined_resolution_option() {
        let result = normalize_launch_params(
            &ConnectionProtocol::Rdp,
            Some(json!({
                "host": "10.10.1.230",
                "port": 3389,
                "options": {
                    "screen_resolution": "1280x720"
                }
            })),
        );

        assert_validation_error_contains(result, "unknown field");
    }

    #[test]
    fn merge_session_metadata_rejects_unknown_keys() {
        let result = merge_session_metadata(Some(json!({ "foo": "bar" })), None);
        assert_validation_error_contains(result, "unknown field");
    }

    #[test]
    fn merge_session_metadata_accepts_entry_id_and_template_id() {
        let result = merge_session_metadata(
            Some(
                json!({ "entryId": "user:config-context:ssh:0:01957fce-a4f7-7a80-9ac2-f6dc041ce772" }),
            ),
            Some("01957fce-a4f7-7a80-9ac2-f6dc041ce772".to_string()),
        );
        assert!(result.is_ok(), "metadata should be valid: {result:?}");
        let merged = result.unwrap_or_default();

        assert_eq!(
            merged.get("entryId").and_then(|value| value.as_str()),
            Some("user:config-context:ssh:0:01957fce-a4f7-7a80-9ac2-f6dc041ce772")
        );
        assert_eq!(
            merged.get("template_id").and_then(|value| value.as_str()),
            Some("01957fce-a4f7-7a80-9ac2-f6dc041ce772")
        );
    }

    #[test]
    fn merge_session_metadata_accepts_credential_merge_policy() {
        let result = merge_session_metadata(
            Some(json!({ "credentialMergePolicy": "PERSONAL_FIRST" })),
            None,
        );
        assert!(result.is_ok(), "metadata should be valid: {result:?}");
        let merged = result.unwrap_or_default();

        assert_eq!(
            merged
                .get("credentialMergePolicy")
                .and_then(|value| value.as_str()),
            Some("personal_first")
        );
    }

    #[test]
    fn merge_session_metadata_rejects_invalid_credential_merge_policy() {
        let result = merge_session_metadata(
            Some(json!({ "credentialMergePolicy": "retired_policy" })),
            None,
        );
        assert_validation_error_contains(result, "credentialMergePolicy");
    }

    #[test]
    fn live_health_for_direct_route_ignores_embedded_client_proxy_link() {
        let session = sample_session(ConnectionProtocol::Ssh, None, ConnectionStatus::Active);
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let metrics = sample_proxy_metrics("healthy", "healthy");

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            None,
        );

        let leg_ids: Vec<&str> = response.legs.iter().map(|leg| leg.id.as_str()).collect();
        assert_eq!(response.route_mode, ROUTE_MODE_DIRECT);
        assert_eq!(response.state, HEALTH_STATE_OK);
        assert_eq!(leg_ids, vec![LEG_CLIENT_BACKEND, LEG_BACKEND_TARGET]);
        assert!(
            response
                .legs
                .iter()
                .all(|leg| leg.id.as_str() != LEG_BACKEND_PROXY)
        );
        let backend_target = response
            .legs
            .iter()
            .find(|leg| leg.id == LEG_BACKEND_TARGET)
            .unwrap();
        assert_eq!(backend_target.latency_ms, Some(22));
        assert_eq!(backend_target.rx_bytes, Some(300));
        assert_eq!(backend_target.tx_bytes, Some(400));
    }

    #[test]
    fn live_health_for_open_direct_route_ignores_stale_proxy_connecting_state() {
        let session = sample_session(ConnectionProtocol::Ssh, None, ConnectionStatus::Active);
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let mut metrics = sample_proxy_metrics("healthy", "healthy");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            None,
        );

        assert_eq!(response.route_mode, ROUTE_MODE_DIRECT);
        assert_eq!(response.state, HEALTH_STATE_OK);
        assert_eq!(response.worst_leg_id, None);
        assert_eq!(
            response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_BACKEND_TARGET)
                .unwrap()
                .state,
            HEALTH_STATE_OK
        );
    }

    #[test]
    fn live_health_for_open_direct_route_promotes_connecting_link_with_activity() {
        let session = sample_session(ConnectionProtocol::Ssh, None, ConnectionStatus::Active);
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let mut metrics = sample_proxy_metrics("healthy", "connecting");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            None,
        );

        let backend_target = response
            .legs
            .iter()
            .find(|leg| leg.id == LEG_BACKEND_TARGET)
            .unwrap();

        assert_eq!(response.route_mode, ROUTE_MODE_DIRECT);
        assert_eq!(response.state, HEALTH_STATE_OK);
        assert_eq!(response.worst_leg_id, None);
        assert_eq!(backend_target.state, HEALTH_STATE_OK);
        assert_eq!(backend_target.latency_ms, Some(22));
        assert_eq!(backend_target.rx_bytes, Some(300));
        assert_eq!(backend_target.tx_bytes, Some(400));
    }

    #[test]
    fn live_health_for_proxied_route_maps_both_proxy_links() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let mut metrics = sample_proxy_metrics("degraded", "healthy");
        metrics.links[0].latency_ms = Some(11);

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            Some(37),
            None,
            Some("pxm-proxy01".to_string()),
        );

        let leg_ids: Vec<&str> = response.legs.iter().map(|leg| leg.id.as_str()).collect();
        assert_eq!(response.route_mode, ROUTE_MODE_PROXIED);
        assert_eq!(response.proxy_node_name.as_deref(), Some("pxm-proxy01"));
        assert_eq!(
            leg_ids,
            vec![LEG_CLIENT_BACKEND, LEG_BACKEND_PROXY, LEG_PROXY_TARGET]
        );
        assert_eq!(
            response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_BACKEND_PROXY)
                .unwrap()
                .state,
            "degraded"
        );
        assert_eq!(
            response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_BACKEND_PROXY)
                .unwrap()
                .latency_ms,
            Some(37)
        );
        let fallback_response = build_live_connection_health_response(
            &session,
            Some(sample_relay_snapshot(RelayHealthState::Ok)),
            Some(&metrics),
            None,
            None,
            Some("pxm-proxy01".to_string()),
        );
        assert_eq!(
            fallback_response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_BACKEND_PROXY)
                .unwrap()
                .latency_ms,
            Some(11)
        );
        assert_eq!(
            response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_PROXY_TARGET)
                .unwrap()
                .state,
            HEALTH_STATE_OK
        );
    }

    #[test]
    fn live_health_for_open_proxied_session_ignores_stale_proxy_connecting_state() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let mut metrics = sample_proxy_metrics("degraded", "degraded");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            Some("pxm-proxy01".to_string()),
        );

        assert_eq!(response.state, HEALTH_STATE_DEGRADED);
        assert_eq!(response.worst_leg_id.as_deref(), Some(LEG_BACKEND_PROXY));
        assert_eq!(
            response
                .legs
                .iter()
                .filter(|leg| leg.id != LEG_CLIENT_BACKEND)
                .map(|leg| leg.state.as_str())
                .collect::<Vec<_>>(),
            vec![HEALTH_STATE_DEGRADED, HEALTH_STATE_DEGRADED]
        );
    }

    #[test]
    fn live_health_open_proxied_healthy_links_ignore_stale_connecting() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let mut metrics = sample_proxy_metrics("healthy", "healthy");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            Some("pxm-proxy01".to_string()),
        );

        assert_eq!(response.state, HEALTH_STATE_OK);
        assert_eq!(response.worst_leg_id, None);
        assert_eq!(
            response
                .legs
                .iter()
                .filter(|leg| leg.id != LEG_CLIENT_BACKEND)
                .map(|leg| leg.state.as_str())
                .collect::<Vec<_>>(),
            vec![HEALTH_STATE_OK, HEALTH_STATE_OK]
        );
    }

    #[test]
    fn live_health_for_open_proxied_route_promotes_connecting_links_with_activity() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Ok);
        let mut metrics = sample_proxy_metrics("connecting", "connecting");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            Some(37),
            None,
            Some("pxm-proxy01".to_string()),
        );

        assert_eq!(response.route_mode, ROUTE_MODE_PROXIED);
        assert_eq!(response.state, HEALTH_STATE_OK);
        assert_eq!(response.worst_leg_id, None);
        assert_eq!(
            response
                .legs
                .iter()
                .filter(|leg| leg.id != LEG_CLIENT_BACKEND)
                .map(|leg| leg.state.as_str())
                .collect::<Vec<_>>(),
            vec![HEALTH_STATE_OK, HEALTH_STATE_OK]
        );
        assert_eq!(
            response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_BACKEND_PROXY)
                .unwrap()
                .latency_ms,
            Some(37)
        );
    }

    #[test]
    fn live_health_for_active_session_without_relay_snapshot_does_not_mask_links_as_connecting() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let mut metrics = sample_proxy_metrics("healthy", "healthy");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            None,
            Some(&metrics),
            None,
            None,
            Some("pxm-proxy01".to_string()),
        );

        assert_eq!(response.state, HEALTH_STATE_UNKNOWN);
        assert_eq!(response.worst_leg_id.as_deref(), Some(LEG_CLIENT_BACKEND));
        assert_eq!(
            response
                .legs
                .iter()
                .filter(|leg| leg.id != LEG_CLIENT_BACKEND)
                .map(|leg| leg.state.as_str())
                .collect::<Vec<_>>(),
            vec![HEALTH_STATE_OK, HEALTH_STATE_OK]
        );
    }

    #[test]
    fn live_health_for_proxied_connect_phase_preserves_connecting_state_until_relay_opens() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Connecting);
        let mut metrics = sample_proxy_metrics("degraded", "degraded");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            Some("pxm-proxy01".to_string()),
        );

        assert_eq!(response.state, HEALTH_STATE_CONNECTING);
        assert_eq!(response.worst_leg_id.as_deref(), Some(LEG_CLIENT_BACKEND));
        assert_eq!(
            response
                .legs
                .iter()
                .map(|leg| leg.state.as_str())
                .collect::<Vec<_>>(),
            vec![
                HEALTH_STATE_CONNECTING,
                HEALTH_STATE_CONNECTING,
                HEALTH_STATE_CONNECTING
            ]
        );
    }

    #[test]
    fn live_health_proxy_link_failure_overrides_connecting_overall_state() {
        let session = sample_session(
            ConnectionProtocol::Rdp,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Connecting);
        let mut metrics = sample_proxy_metrics("failed", "healthy");
        metrics.overall_status = "connecting".to_string();

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            Some(&metrics),
            None,
            None,
            Some("pxm-proxy01".to_string()),
        );

        assert_eq!(response.state, HEALTH_STATE_FAILED);
        assert_eq!(response.worst_leg_id.as_deref(), Some(LEG_BACKEND_PROXY));
        assert_eq!(
            response
                .legs
                .iter()
                .find(|leg| leg.id == LEG_BACKEND_PROXY)
                .unwrap()
                .state,
            HEALTH_STATE_FAILED
        );
    }

    #[test]
    fn live_health_preserves_200_contract_when_proxy_metrics_are_unavailable() {
        let session = sample_session(
            ConnectionProtocol::Vnc,
            Some(Uuid::now_v7()),
            ConnectionStatus::Active,
        );
        let relay = sample_relay_snapshot(RelayHealthState::Ok);

        let response = build_live_connection_health_response(
            &session,
            Some(relay),
            None,
            None,
            Some(REASON_PROXY_METRICS_UNAVAILABLE.to_string()),
            None,
        );

        assert_eq!(
            response.reason_code.as_deref(),
            Some(REASON_PROXY_METRICS_UNAVAILABLE)
        );
        assert_eq!(response.state, HEALTH_STATE_UNKNOWN);
        assert_eq!(response.worst_leg_id.as_deref(), Some(LEG_BACKEND_PROXY));
        assert_eq!(
            response
                .legs
                .iter()
                .filter(|leg| leg.id != LEG_CLIENT_BACKEND)
                .map(|leg| leg.state.as_str())
                .collect::<Vec<_>>(),
            vec![HEALTH_STATE_UNKNOWN, HEALTH_STATE_UNKNOWN]
        );
    }

    #[test]
    fn live_health_failed_session_overrides_stale_proxy_metrics() {
        let session = sample_session(ConnectionProtocol::Ssh, None, ConnectionStatus::Failed);
        let metrics = sample_proxy_metrics("healthy", "healthy");

        let response =
            build_live_connection_health_response(&session, None, Some(&metrics), None, None, None);

        assert_eq!(response.state, HEALTH_STATE_FAILED);
        assert_eq!(response.worst_leg_id.as_deref(), Some(LEG_CLIENT_BACKEND));
        assert!(
            response
                .legs
                .iter()
                .all(|leg| leg.state == HEALTH_STATE_FAILED)
        );
    }

    #[test]
    fn live_health_proxy_node_lookup_error_is_optional_metadata() {
        let session_id = Uuid::now_v7();
        let proxy_node_id = Uuid::now_v7();

        let name = optional_proxy_node_name_from_lookup(
            session_id,
            proxy_node_id,
            Err(AppError::External("repository unavailable".to_string())),
        );

        assert!(name.is_none());
    }

    fn sample_session(
        protocol: ConnectionProtocol,
        proxy_node_id: Option<Uuid>,
        status: ConnectionStatus,
    ) -> ConnectionSession {
        ConnectionSession {
            id: Uuid::now_v7(),
            target_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            protocol,
            status,
            proxy_id: Some(Uuid::now_v7()),
            proxy_node_id,
            routing_source: RoutingSource::EmbeddedDefault,
            client_ip: Some("127.0.0.1".to_string()),
            last_activity: Utc::now(),
            session_metadata: json!({}),
            started_at: Utc::now(),
            ended_at: None,
        }
    }

    fn sample_relay_snapshot(state: RelayHealthState) -> RelayLegHealthSnapshot {
        RelayLegHealthSnapshot {
            state,
            updated_at: Utc::now(),
            rx_bytes: 100,
            tx_bytes: 200,
            rx_bps: Some(10),
            tx_bps: Some(20),
            reason_code: None,
        }
    }

    fn sample_proxy_metrics(
        client_proxy_status: &str,
        proxy_protocol_status: &str,
    ) -> ProxyMetricsResponse {
        ProxyMetricsResponse {
            connection_id: Uuid::now_v7().to_string(),
            updated_at: Utc::now().to_rfc3339(),
            overall_status: "connected".to_string(),
            links: vec![
                ProxyLinkMetricsResponse {
                    link_type: "client_proxy".to_string(),
                    status: client_proxy_status.to_string(),
                    latency_ms: Some(11),
                    rx_bytes: Some(100),
                    tx_bytes: Some(200),
                    rx_bps: Some(10),
                    tx_bps: Some(20),
                    quality_label: Some("good".to_string()),
                    quality_score: Some(100),
                },
                ProxyLinkMetricsResponse {
                    link_type: "proxy_protocol".to_string(),
                    status: proxy_protocol_status.to_string(),
                    latency_ms: Some(22),
                    rx_bytes: Some(300),
                    tx_bytes: Some(400),
                    rx_bps: Some(30),
                    tx_bps: Some(40),
                    quality_label: Some("warning".to_string()),
                    quality_score: Some(75),
                },
            ],
            observed_host_keys: Vec::new(),
        }
    }
}
