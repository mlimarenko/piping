use axum::{
    extract::{Request, State},
    http::Method,
    http::header::{AUTHORIZATION, COOKIE},
    middleware::Next,
    response::{IntoResponse, Response},
};
use cookie::Cookie;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::app::AppState;
use crate::contexts::access_control::application::AuthService;
use crate::contexts::shared::request_context::{
    ApiTokenScopeContext, ApiTokenScopeResourceOverride,
};
use crate::integrations::oidc::{OidcSessionMetadata, session_cache_key};
use crate::observability::{EventFields, LogLevel, log_event, metrics, set_attribute};
use crate::storage::cache::redis::cache as redis_cache;

/// Authentication context for the current request.
///
/// Contains information about the authenticated user (if any) and how they authenticated.
/// This is always present in request extensions, even for unauthenticated requests.
#[derive(Clone, Debug)]
pub struct AuthContext {
    /// User ID if authenticated, None otherwise
    pub user_id: Option<Uuid>,
    /// Authentication method: "session", "oidc", or "none"
    pub auth_method: String,
    /// OIDC groups if authenticated via OIDC, empty otherwise
    pub oidc_groups: Vec<String>,
    /// API token id if authenticated via personal API token
    pub api_token_id: Option<String>,
    /// API token scopes for policy narrowing
    pub api_token_scopes: Vec<ApiTokenScopeContext>,
    /// Internal one-request resource id override for legacy coarse guards.
    pub api_token_scope_resource_override: Option<ApiTokenScopeResourceOverride>,
}

impl AuthContext {
    /// Creates an anonymous (unauthenticated) auth context.
    ///
    /// Used for requests without valid authentication.
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            auth_method: "none".to_string(),
            oidc_groups: Vec::new(),
            api_token_id: None,
            api_token_scopes: Vec::new(),
            api_token_scope_resource_override: None,
        }
    }
}

/// Extracts a cookie value from the Cookie header.
///
/// Parses the Cookie header string and searches for a cookie with the given name.
/// Returns None if the cookie is not found or cannot be parsed.
fn extract_cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let header_value = headers.get(COOKIE)?.to_str().ok()?;
    // Cookie header format: "name1=value1; name2=value2; ..."
    for part in header_value.split(';') {
        let cookie: Cookie<'_> = match Cookie::parse(part.trim()) {
            Ok(cookie) => cookie,
            Err(_) => continue, // Skip malformed cookies
        };
        if cookie.name() == name {
            return Some(cookie.value().to_string());
        }
    }
    None
}

fn extract_bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BearerTokenKind {
    PersonalApiToken,
    PlaybookJobToken,
    Unknown,
}

fn classify_bearer_token(token: &str) -> BearerTokenKind {
    if token.starts_with("pat_") {
        BearerTokenKind::PersonalApiToken
    } else if token.starts_with("pjt_") {
        BearerTokenKind::PlaybookJobToken
    } else {
        BearerTokenKind::Unknown
    }
}

/// Authentication middleware that extracts and validates session from cookies.
///
/// Process:
/// 1. Skips authentication for health check endpoints
/// 2. Extracts session ID from configured cookie name
/// 3. Validates session in database (checks expiration, revocation, user status)
/// 4. Retrieves OIDC metadata from Redis if available (for OIDC sessions)
/// 5. Sets AuthContext in request extensions for downstream handlers
///
/// If no valid session is found, AuthContext is set to anonymous (user_id = None).
/// Handlers can check authentication via `Extension<AuthContext>`.
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    // Skip authentication for endpoints that authenticate themselves.
    if is_auth_skipped_endpoint(request.method(), path) {
        return next.run(request).await;
    }

    // Prefer bearer token auth if provided, and never fall back to session auth
    // for malformed or disabled bearer-token flows.
    if let Some(token) = extract_bearer_token(request.headers()) {
        return authenticate_bearer_token(&state, request, next, token).await;
    }

    let mut auth_context = AuthContext::anonymous();

    // If session ID found, validate it
    if let Some(session_id) = session_id_from_request(&state, &request) {
        resolve_session_into(&state, &mut auth_context, session_id).await;
    }

    // Always set AuthContext (even if anonymous) so handlers can check authentication
    request.extensions_mut().insert(auth_context);
    next.run(request).await
}

/// Endpoints whose own guards replace session/bearer authentication.
fn is_auth_skipped_endpoint(method: &Method, path: &str) -> bool {
    path == "/api/v1/health"
        || path == "/api/v1/ready"
        || path == "/api/v1/live"
        || is_monitoring_token_endpoint(path)
        || is_playbook_job_token_endpoint(method, path)
}

/// Dispatch a bearer token to the flow its prefix selects.
async fn authenticate_bearer_token(
    state: &AppState,
    request: Request,
    next: Next,
    token: String,
) -> Response {
    if !state.settings.auth.api_tokens.enabled {
        return handle_app_error(crate::errors::AppError::Auth("TOKEN_INVALID".to_string()))
            .into_response();
    }

    match classify_bearer_token(token.as_str()) {
        BearerTokenKind::PersonalApiToken => {
            authenticate_personal_api_token(state, request, next, token).await
        }
        BearerTokenKind::PlaybookJobToken => {
            authenticate_playbook_job_token(state, request, next, token).await
        }
        BearerTokenKind::Unknown => {
            handle_app_error(crate::errors::AppError::Auth("TOKEN_INVALID".to_string()))
                .into_response()
        }
    }
}

/// Validate a personal API token and populate `AuthContext` on success.
async fn authenticate_personal_api_token(
    state: &AppState,
    mut request: Request,
    next: Next,
    token: String,
) -> Response {
    let token_service = state.services.user_api_tokens.clone();
    match token_service
        .validate_bearer_token(token.as_str(), chrono::Utc::now())
        .await
    {
        Ok(validated) => {
            metrics::record_api_token_auth_success();
            let token_id = validated.token_id.to_string();
            let user_id = validated.user_id;
            let request_id = request_id_from_request(&request);
            set_attribute("auth.method", "api_token");
            set_attribute("auth.api_token.id", token_id.as_str());
            log_event(
                LogLevel::Info,
                "access_control.api_token.auth.succeeded",
                EventFields::default()
                    .with_message("API token authentication succeeded")
                    .with_request_id(request_id.clone())
                    .with_user_id(user_id.to_string())
                    .with_operation("auth.api_token.validate")
                    .with_resource("user_api_token", token_id)
                    .with_context("scope_count", validated.scopes.len().to_string()),
            );
            let auth_context = AuthContext {
                user_id: Some(user_id),
                auth_method: "api_token".to_string(),
                oidc_groups: Vec::new(),
                api_token_id: Some(validated.token_id.to_string()),
                api_token_scopes: validated
                    .scopes
                    .into_iter()
                    .map(|scope| ApiTokenScopeContext {
                        action: action_key(&scope.action).to_string(),
                        resource_type: scope.resource_type.as_str().to_string(),
                        resource_pattern: scope.resource_pattern,
                    })
                    .collect(),
                api_token_scope_resource_override: None,
            };
            request.extensions_mut().insert(auth_context);
            next.run(request).await
        }
        Err(error) => {
            let reason = token_auth_reason(&error);
            let request_id = request_id_from_request(&request);
            metrics::record_api_token_auth_denied(reason);
            set_attribute("auth.method", "api_token");
            set_attribute("auth.api_token.result", "denied");
            set_attribute("auth.api_token.reason", reason);
            log_event(
                LogLevel::Warning,
                "access_control.api_token.auth.denied",
                EventFields::default()
                    .with_message("API token authentication denied")
                    .with_request_id(request_id)
                    .with_operation("auth.api_token.validate")
                    .with_status("denied")
                    .with_context("reason", reason),
            );
            handle_app_error(error).into_response()
        }
    }
}

/// Validate a playbook job token, which is only accepted on inventory REST writes/reads.
async fn authenticate_playbook_job_token(
    state: &AppState,
    mut request: Request,
    next: Next,
    token: String,
) -> Response {
    if !is_playbook_inventory_rest_endpoint(request.method(), request.uri().path()) {
        return handle_app_error(crate::errors::AppError::Auth("TOKEN_INVALID".to_string()))
            .into_response();
    }

    match state
        .services
        .playbooks
        .validate_inventory_rest_job_token(token.as_str(), chrono::Utc::now())
        .await
    {
        Ok(validated) => {
            let token_id = validated.id.to_string();
            set_attribute("auth.method", "playbook_job_token");
            set_attribute("auth.playbook_job_token.id", token_id.as_str());
            let auth_context = AuthContext {
                user_id: None,
                auth_method: "playbook_job_token".to_string(),
                oidc_groups: Vec::new(),
                api_token_id: Some(token_id),
                api_token_scopes: inventory_job_token_scope_contexts(&validated.scopes),
                api_token_scope_resource_override: None,
            };
            request.extensions_mut().insert(auth_context);
            next.run(request).await
        }
        Err(error) => {
            set_attribute("auth.method", "playbook_job_token");
            set_attribute("auth.playbook_job_token.result", "denied");
            handle_app_error(error).into_response()
        }
    }
}

/// Resolve the session cookie into a session id, if one is present and parseable.
fn session_id_from_request(state: &AppState, request: &Request) -> Option<Uuid> {
    let session_cookie_name = state.settings.auth.session_cookie_name.as_str();
    extract_cookie_value(request.headers(), session_cookie_name)
        .and_then(|value| Uuid::parse_str(&value).ok())
}

/// Validate a session and fill `auth_context` from the user plus cached OIDC metadata.
async fn resolve_session_into(state: &AppState, auth_context: &mut AuthContext, session_id: Uuid) {
    let auth_service: std::sync::Arc<AuthService> = state.services.auth.clone();

    // Validate session: checks expiration, revocation, and user status
    let Ok(Some((user, _session))) = auth_service
        .resolve_session(session_id, chrono::Utc::now())
        .await
    else {
        return;
    };

    auth_context.user_id = Some(user.id);
    auth_context.auth_method = "session".to_string();
    auth_context.api_token_id = None;
    auth_context.api_token_scopes.clear();

    // Try to retrieve OIDC metadata from Redis cache. This contains OIDC groups
    // and auth method if the session was created via OIDC.
    let mut redis = state.redis.clone();
    if let Ok(Some(metadata)) = redis_cache::get(&mut redis, &session_cache_key(&session_id)).await
        && let Ok(metadata) = serde_json::from_str::<OidcSessionMetadata>(&metadata)
    {
        auth_context.auth_method = metadata.auth_method;
        auth_context.oidc_groups = metadata.oidc_groups;
    }
}

fn action_key(action: &crate::contexts::access_control::domain::Action) -> &'static str {
    use crate::contexts::access_control::domain::Action;

    match action {
        Action::Create => "create",
        Action::Read => "read",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Connect => "connect",
        Action::Observe => "observe",
        Action::Execute => "execute",
        Action::CctvView => "view",
        Action::CctvRecordings => "recordings",
        Action::CctvExport => "export",
        Action::DeviceUsers => "device_users",
        Action::CctvSettings => "settings",
        Action::CctvPtz => "ptz",
        Action::ManageRoles => "manage_roles",
        Action::ManagePolicies => "manage_policies",
        Action::ViewAudit => "view_audit",
        Action::Secret => "secret",
    }
}

fn token_auth_reason(error: &crate::errors::AppError) -> &'static str {
    match error {
        crate::errors::AppError::Auth(message) if message == "TOKEN_REVOKED" => "TOKEN_REVOKED",
        crate::errors::AppError::Auth(message) if message == "TOKEN_EXPIRED" => "TOKEN_EXPIRED",
        crate::errors::AppError::Auth(message) if message == "TOKEN_INVALID" => "TOKEN_INVALID",
        crate::errors::AppError::Forbidden(message)
            if message.starts_with("TOKEN_SCOPE_DENIED:") =>
        {
            "TOKEN_SCOPE_DENIED"
        }
        _ => "TOKEN_INVALID",
    }
}

fn request_id_from_request(request: &Request) -> String {
    if let Some(request_id) = request.extensions().get::<RequestId>() {
        return request_id.0.clone();
    }
    request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "missing-request-id".to_string())
}

fn is_monitoring_token_endpoint(path: &str) -> bool {
    matches!(
        path,
        "/metrics" | "/api/v1/metrics" | "/api/v1/monitoring/sd"
    )
}

/// Identify playbook job-token callback endpoints that bypass central
/// session/bearer auth.
///
/// SECURITY (H1): the bypass is method-aware. The artifacts/events paths serve
/// BOTH the job-token `POST` callbacks (which carry their own bearer auth and
/// must skip central auth) AND session-authenticated `GET` read endpoints
/// (`list_run_artifacts`/`list_run_events`). A path-only bypass let the `GET`
/// reads skip `AuthContext` insertion and always 401 (dead), and let every
/// method on those paths skip central auth. Only `POST` callbacks may bypass.
fn is_playbook_job_token_endpoint(method: &Method, path: &str) -> bool {
    if method != Method::POST {
        return false;
    }
    if let Some(path) = path.strip_prefix("/api/v1/playbooks/runs/") {
        let mut parts = path.split('/');
        let Some(run_id) = parts.next() else {
            return false;
        };
        return !run_id.is_empty()
            && matches!(
                (parts.next(), parts.next()),
                (Some("artifacts" | "events" | "completion"), None)
            );
    }
    let Some(path) = path.strip_prefix("/api/v1/playbooks/run-groups/") else {
        return false;
    };
    let mut parts = path.split('/');
    let Some(group_id) = parts.next() else {
        return false;
    };
    !group_id.is_empty()
        && matches!(
            (parts.next(), parts.next(), parts.next()),
            (Some("events"), None, None)
                | (Some("events"), Some("batch"), None)
                | (Some("completion-batch"), None, None)
        )
}

fn is_playbook_inventory_rest_endpoint(method: &Method, path: &str) -> bool {
    matches!(
        *method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH
    ) && path.starts_with("/api/v1/inventory/")
}

fn inventory_job_token_scope_contexts(scopes: &serde_json::Value) -> Vec<ApiTokenScopeContext> {
    scopes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|scope| match scope.as_str()? {
            "inventory:read" => Some("read"),
            "inventory:create" => Some("create"),
            "inventory:update" => Some("update"),
            _ => None,
        })
        .map(|action| ApiTokenScopeContext {
            action: action.to_string(),
            resource_type: "inventory".to_string(),
            resource_pattern: "*".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BearerTokenKind, Method, classify_bearer_token, is_monitoring_token_endpoint,
        is_playbook_inventory_rest_endpoint, is_playbook_job_token_endpoint,
    };

    #[test]
    fn monitoring_collector_routes_skip_user_api_token_middleware() {
        assert!(is_monitoring_token_endpoint("/metrics"));
        assert!(is_monitoring_token_endpoint("/api/v1/metrics"));
        assert!(is_monitoring_token_endpoint("/api/v1/monitoring/sd"));
    }

    #[test]
    fn monitoring_ui_routes_still_use_regular_auth_middleware() {
        assert!(!is_monitoring_token_endpoint(
            "/api/v1/monitoring/sd/preview"
        ));
        assert!(!is_monitoring_token_endpoint("/api/v1/monitoring/latest"));
        assert!(!is_monitoring_token_endpoint(
            "/api/v1/monitoring/targets/target-1"
        ));
    }

    #[test]
    fn playbook_job_token_post_callbacks_skip_regular_auth_middleware() {
        assert!(is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/artifacts"
        ));
        assert!(is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/events"
        ));
        assert!(is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/completion"
        ));
        assert!(is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/run-groups/019c2f1b-f106-7f91-9697-826530777788/events"
        ));
        assert!(is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/run-groups/019c2f1b-f106-7f91-9697-826530777788/events/batch"
        ));
        assert!(is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/run-groups/019c2f1b-f106-7f91-9697-826530777788/completion-batch"
        ));
        // Legacy facts-ingest callback URLs must not be classified as
        // run-bound job-token lifecycle callbacks.
        assert!(!is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/inventory-facts"
        ));
        assert!(!is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/facts"
        ));
        assert!(!is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/019c2f1b-f106-7f91-9697-826530777788/runs"
        ));
        assert!(!is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/artifacts/extra"
        ));
        assert!(!is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788"
        ));
        assert!(!is_playbook_job_token_endpoint(
            &Method::POST,
            "/api/v1/playbooks/run-groups/019c2f1b-f106-7f91-9697-826530777788/events/extra"
        ));
    }

    #[test]
    fn playbook_run_read_get_endpoints_do_not_skip_regular_auth_middleware() {
        // H1: GET list_run_artifacts/list_run_events share the artifacts/events
        // paths with the POST callbacks but must flow through central session auth.
        for method in [Method::GET, Method::PUT, Method::DELETE, Method::PATCH] {
            assert!(!is_playbook_job_token_endpoint(
                &method,
                "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/artifacts"
            ));
            assert!(!is_playbook_job_token_endpoint(
                &method,
                "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/events"
            ));
            assert!(!is_playbook_job_token_endpoint(
                &method,
                "/api/v1/playbooks/run-groups/019c2f1b-f106-7f91-9697-826530777788/events"
            ));
            assert!(!is_playbook_job_token_endpoint(
                &method,
                "/api/v1/playbooks/run-groups/019c2f1b-f106-7f91-9697-826530777788/events/batch"
            ));
        }
    }

    #[test]
    fn bearer_token_classifier_is_prefix_explicit() {
        assert_eq!(
            classify_bearer_token("pat_019c2f1b-f106-7f91-9697-826530777788_secret"),
            BearerTokenKind::PersonalApiToken
        );
        assert_eq!(
            classify_bearer_token("pjt_019c2f1b-f106-7f91-9697-826530777788_secret"),
            BearerTokenKind::PlaybookJobToken
        );
        assert_eq!(classify_bearer_token("opaque"), BearerTokenKind::Unknown);
    }

    #[test]
    fn playbook_job_tokens_are_accepted_only_on_inventory_rest_writes_and_reads() {
        assert!(is_playbook_inventory_rest_endpoint(
            &Method::GET,
            "/api/v1/inventory/devices"
        ));
        assert!(is_playbook_inventory_rest_endpoint(
            &Method::POST,
            "/api/v1/inventory/ip-addresses"
        ));
        assert!(is_playbook_inventory_rest_endpoint(
            &Method::PUT,
            "/api/v1/inventory/devices/019c2f1b-f106-7f91-9697-826530777788"
        ));
        assert!(!is_playbook_inventory_rest_endpoint(
            &Method::DELETE,
            "/api/v1/inventory/devices/019c2f1b-f106-7f91-9697-826530777788"
        ));
        // Inventory REST access is only for normal `/api/v1/inventory/*`
        // endpoints, never for the retired playbook facts-ingest callback.
        assert!(!is_playbook_inventory_rest_endpoint(
            &Method::POST,
            "/api/v1/playbooks/runs/019c2f1b-f106-7f91-9697-826530777788/inventory-facts"
        ));
    }
}
