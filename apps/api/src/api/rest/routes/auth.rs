use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::CookieJar;
use cookie::{Cookie, SameSite, time::Duration as CookieDuration};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::access_control_helpers::access_control_service;
use crate::api::middleware::error::handle_app_error;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Query};
use crate::api::rest::error_mapping::build_error_response;
use crate::app::AppState;
use crate::contexts::access_control::application::{AuthService, EffectiveOidcConfig};
use crate::contexts::access_control::domain::{Action, Principal, ResourceType};
use crate::contexts::audit::application::{
    AuditRecorder, OperationTimelineActionKind, OperationTimelineResult, OperationTimelineService,
    OperationTimelineWriteRequest, build_operation_timeline_event,
};
use crate::contexts::shared::domain::reason_codes;
use crate::errors::AppError;
use crate::integrations::oidc::{
    self, OIDC_STATE_COOKIE_NAME, OIDC_STATE_TTL_SECONDS, OidcSessionMetadata, OidcStateData,
};
use crate::observability::{EventFields, LogLevel, instrument_span, log_event, operation_span};
use crate::storage::cache::redis::cache as redis_cache;
use tracing::warn;

const AUTH_COOKIE_PATH: &str = "/";
const AUTH_DEFAULT_REDIRECT_PATH: &str = "/";
const AUTH_LOGIN_REDIRECT_PATH: &str = "/auth/login";
const AUTH_REDIRECT_PATH: &str = "/auth/redirect";
const AUTH_REDIRECT_QUERY_PARAM: &str = "&redirect=";
const AUTH_INTERNAL_REDIRECT_MAX_LENGTH: usize = 1024;
const OIDC_DISABLED_ERROR_MESSAGE: &str = "OIDC authentication is disabled";
const LOCAL_AUTH_DISABLED_ERROR_MESSAGE: &str = "Local authentication is disabled";
const MISSING_CREDENTIALS_ERROR_MESSAGE: &str = "Identifier and password are required";
const SESSION_REFRESH_STATUS_EXPIRED: &str = "expired";
const SESSION_REFRESH_STATUS_REFRESHED: &str = "refreshed";
const OIDC_ERROR_CODE_AUTH_FAILED: &str = "auth_failed";
const OIDC_ERROR_CODE_INVALID_REQUEST: &str = "invalid_request";
const OIDC_ERROR_CODE_INVALID_STATE: &str = "invalid_state";
const OIDC_ERROR_CODE_LICENSE_SEAT_LIMIT: &str = "license_seat_limit";
const OIDC_ERROR_CODE_LICENSE_REFRESH_REQUIRED: &str = "license_refresh_required";
const OIDC_ERROR_CODE_LICENSE_GRACE_ENDED: &str = "license_grace_ended";
const OIDC_ERROR_CODE_LICENSE_UNAVAILABLE: &str = "license_unavailable";
const OIDC_ERROR_CODE_SERVER_ERROR: &str = "server_error";
const LICENSE_ERROR_PREFIX_SEAT_LIMIT_EXCEEDED: &str = "LICENSE_SEAT_LIMIT_EXCEEDED:";
const LICENSE_ERROR_PREFIX_REFRESH_REQUIRED: &str = "LICENSE_REFRESH_REQUIRED:";
const LICENSE_ERROR_PREFIX_GRACE_ENDED: &str = "LICENSE_GRACE_ENDED:";
const LICENSE_ERROR_PREFIX_ENTITLEMENT_BINDING_INVALID: &str = "ENTITLEMENT_BINDING_INVALID:";
const LICENSE_ERROR_CODE_SEAT_LIMIT_EXCEEDED: &str = "LICENSE_SEAT_LIMIT_EXCEEDED";
const LICENSE_ERROR_CODE_REFRESH_REQUIRED: &str = "LICENSE_REFRESH_REQUIRED";
const LICENSE_ERROR_CODE_GRACE_ENDED: &str = "LICENSE_GRACE_ENDED";
const LICENSE_ERROR_CODE_UNAVAILABLE: &str = "LICENSE_UNAVAILABLE";
const LICENSE_MESSAGE_SEAT_LIMIT: &str =
    "No license seats are available. Contact your administrator.";
const LICENSE_MESSAGE_REFRESH_REQUIRED: &str =
    "License refresh is required. Contact your administrator.";
const LICENSE_MESSAGE_GRACE_ENDED: &str =
    "License access is temporarily unavailable. Contact your administrator.";
const LICENSE_MESSAGE_UNAVAILABLE: &str =
    "Sign-in is temporarily unavailable because of a license issue. Contact your administrator.";
const DEFAULT_FORWARDED_PROTO: &str = "https";
const OIDC_REWRITABLE_AUTHORITIES: [&str; 3] = ["keycloak", "nginx", "backend"];

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub identifier: String,
    pub password: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuthSessionResponse {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub status: String,
    pub auth_source: String,
    pub expires_at: String,
    /// Role names the authenticated principal resolves to (direct + group).
    pub roles: Vec<String>,
    /// Type-level capability grid the SPA uses to gate navigation and controls.
    pub capabilities: Vec<CapabilityDto>,
}

/// One allowed `(action, resource_type)` pair in the caller's effective
/// authorization. Type-level: presence means "allowed on some resource of this
/// type"; per-object authority remains an API-side check.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CapabilityDto {
    pub action: Action,
    pub resource_type: ResourceType,
}

/// Response for session-refresh endpoint. Matches frontend SessionRefreshResponse.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SessionRefreshResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuthProvidersResponse {
    pub local_enabled: bool,
    pub oidc_enabled: bool,
    pub ldap_enabled: bool,
    pub ad_enabled: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OidcCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ValidatedOidcCallback {
    code: String,
    state: String,
}

#[derive(Debug, PartialEq, Eq)]
struct OidcCallbackValidationError {
    code: &'static str,
    message: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OidcLoginQuery {
    pub redirect_to: Option<String>,
}

fn parse_same_site(value: &str) -> SameSite {
    match value.to_lowercase().as_str() {
        "none" => SameSite::None,
        "lax" => SameSite::Lax,
        "strict" => SameSite::Strict,
        // Config validation rejects unknown values; keep a safe fallback for defense-in-depth.
        _ => SameSite::Lax,
    }
}

fn build_auth_service(state: &AppState) -> std::sync::Arc<AuthService> {
    state.services.auth.clone()
}

async fn invalidate_oidc_membership_policy_cache(state: &AppState, user_id: Uuid) {
    if let Err(error) = state
        .services
        .access_control_admin
        .invalidate_policy_cache_for_user(user_id)
        .await
    {
        warn!(%user_id, error = %error, "failed to invalidate user policy cache after OIDC group sync");
        log_event(
            LogLevel::Warning,
            "access_control.oidc_group_membership.cache_invalidate.failed",
            EventFields::default()
                .with_operation("access_control.oidc_group_membership.cache_invalidate")
                .with_resource("user_policy_cache", user_id.to_string())
                .with_error("policy_cache_invalidation_error", error.to_string())
                .with_message(
                    "OIDC group membership was synced but user policy cache invalidation failed",
                ),
        );
    }
}

async fn load_enabled_oidc_config(state: &AppState) -> Result<EffectiveOidcConfig, AppError> {
    let effective = state
        .services
        .oidc_settings
        .get_effective(&state.settings.auth)
        .await?;
    if !effective.auth.oidc_enabled {
        return Err(AppError::Validation(
            OIDC_DISABLED_ERROR_MESSAGE.to_string(),
        ));
    }
    Ok(effective)
}

fn build_session_cookie(state: &AppState, session_id: String) -> Cookie<'static> {
    let auth_settings = &state.settings.auth;
    let mut cookie_builder = Cookie::build((auth_settings.session_cookie_name.clone(), session_id))
        .path(AUTH_COOKIE_PATH)
        .http_only(true)
        .secure(auth_settings.cookie_secure)
        .same_site(parse_same_site(&auth_settings.cookie_same_site));

    if let Some(domain) = auth_settings.cookie_domain.as_ref()
        && !domain.is_empty()
    {
        cookie_builder = cookie_builder.domain(domain.clone());
    }

    cookie_builder
        .max_age(CookieDuration::seconds(
            auth_settings.session_ttl_seconds as i64,
        ))
        .build()
}

fn build_state_cookie(state: &AppState, value: String) -> Cookie<'static> {
    let auth_settings = &state.settings.auth;
    let mut cookie_builder = Cookie::build((OIDC_STATE_COOKIE_NAME.to_string(), value))
        .path(AUTH_COOKIE_PATH)
        .http_only(true)
        .secure(auth_settings.cookie_secure)
        .same_site(parse_same_site(&auth_settings.cookie_same_site));

    if let Some(domain) = auth_settings.cookie_domain.as_ref()
        && !domain.is_empty()
    {
        cookie_builder = cookie_builder.domain(domain.clone());
    }

    cookie_builder
        .max_age(CookieDuration::seconds(OIDC_STATE_TTL_SECONDS as i64))
        .build()
}

fn build_state_removal_cookie(state: &AppState) -> Cookie<'static> {
    let auth_settings = &state.settings.auth;
    let mut cookie_builder = Cookie::build((OIDC_STATE_COOKIE_NAME.to_string(), ""))
        .path(AUTH_COOKIE_PATH)
        .http_only(true)
        .secure(auth_settings.cookie_secure)
        .same_site(parse_same_site(&auth_settings.cookie_same_site));

    if let Some(domain) = auth_settings.cookie_domain.as_ref()
        && !domain.is_empty()
    {
        cookie_builder = cookie_builder.domain(domain.clone());
    }

    let mut cookie = cookie_builder.build();
    cookie.make_removal();
    cookie
}

fn build_session_removal_cookie(state: &AppState) -> Cookie<'static> {
    let auth_settings = &state.settings.auth;
    let mut cookie_builder = Cookie::build((auth_settings.session_cookie_name.clone(), ""))
        .path(AUTH_COOKIE_PATH)
        .http_only(true)
        .secure(auth_settings.cookie_secure)
        .same_site(parse_same_site(&auth_settings.cookie_same_site));
    if let Some(domain) = auth_settings.cookie_domain.as_ref()
        && !domain.is_empty()
    {
        cookie_builder = cookie_builder.domain(domain.clone());
    }
    let mut cookie = cookie_builder.build();
    cookie.make_removal();
    cookie
}

fn parse_session_id_from_cookie(jar: &CookieJar, session_cookie_name: &str) -> Option<Uuid> {
    jar.get(session_cookie_name)
        .and_then(|cookie| Uuid::parse_str(cookie.value()).ok())
}

/// Log in with username and password and open a session.
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Local login succeeded", body = AuthSessionResponse),
        (status = 400, description = "Invalid login request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Invalid credentials", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Login rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "linear login flow; post-session seat confirmation is extracted into finalize_local_login"
)]
pub async fn login_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    EnvelopeJson(payload): EnvelopeJson<LoginRequest>,
) -> Response {
    let timeline = OperationTimelineService::new(state.services.audit.clone());
    if !state.settings.auth.local_enabled {
        let error = AppError::Validation(LOCAL_AUTH_DISABLED_ERROR_MESSAGE.to_string());
        if let Err(audit_error) = record_signin_timeline_event(
            &timeline,
            SigninTimelineEvent {
                organization_id: None,
                actor_user_id: None,
                result: OperationTimelineResult::Rejected,
                reason_code: None,
                identifier: Some(payload.identifier.trim()),
                auth_method: "local",
            },
        )
        .await
        {
            return handle_app_error(audit_error);
        }
        return handle_app_error(error);
    }
    if payload.identifier.trim().is_empty() || payload.password.trim().is_empty() {
        let error = AppError::Validation(MISSING_CREDENTIALS_ERROR_MESSAGE.to_string());
        if let Err(audit_error) = record_signin_timeline_event(
            &timeline,
            SigninTimelineEvent {
                organization_id: None,
                actor_user_id: None,
                result: OperationTimelineResult::Rejected,
                reason_code: None,
                identifier: Some(payload.identifier.trim()),
                auth_method: "local",
            },
        )
        .await
        {
            return handle_app_error(audit_error);
        }
        return handle_app_error(error);
    }

    let auth_service = build_auth_service(&state);
    let result = auth_service
        .authenticate_local(payload.identifier.trim(), payload.password.trim())
        .await;

    let user = match result {
        Ok(value) => value,
        Err(error) => {
            let reason_code = auth_timeline_reason_code(&error);
            if let Err(audit_error) = record_signin_timeline_event(
                &timeline,
                SigninTimelineEvent {
                    organization_id: None,
                    actor_user_id: None,
                    result: OperationTimelineResult::Rejected,
                    reason_code,
                    identifier: Some(payload.identifier.trim()),
                    auth_method: "local",
                },
            )
            .await
            {
                return handle_app_error(audit_error);
            }
            return handle_app_error(error);
        }
    };

    if let Err(error) = state
        .services
        .license_login_gate
        .enforce_login(user.id)
        .await
    {
        let reason_code = classify_license_auth_failure(&error)
            .map(|failure| failure.reason_code)
            .or_else(|| auth_timeline_reason_code(&error));
        if let Err(audit_error) = record_signin_timeline_event(
            &timeline,
            SigninTimelineEvent {
                organization_id: None,
                actor_user_id: Some(user.id),
                result: OperationTimelineResult::Rejected,
                reason_code,
                identifier: Some(payload.identifier.trim()),
                auth_method: "local",
            },
        )
        .await
        {
            return handle_app_error(audit_error);
        }
        return map_license_error_response(error);
    }

    let (roles, capabilities) = match session_authorization(&state, user.id).await {
        Ok(authorization) => authorization,
        Err(error) => return handle_app_error(error),
    };

    let audit_event = build_signin_timeline_event(SigninTimelineEvent {
        organization_id: None,
        actor_user_id: Some(user.id),
        result: OperationTimelineResult::Success,
        reason_code: None,
        identifier: Some(payload.identifier.trim()),
        auth_method: "local",
    });
    let session = match auth_service
        .open_session(user.id, audit_event.clone())
        .await
    {
        Ok(session) => session,
        Err(error) => return handle_app_error(error),
    };

    // The seat gate commits before the session row exists. Re-check after
    // creating the session so an eviction that lands in that window cannot
    // leave a post-eviction session behind; eviction already revoking sessions
    // covers the opposite order. The confirmation also advances the LIFO
    // timestamp only after successful session creation, and rejects a session
    // that eviction revoked before capacity returned.
    if let Err(response) = finalize_local_login(&state, &auth_service, session.id, user.id).await {
        return *response;
    }

    state.services.audit.fanout_recorded_event(audit_event);

    let cookie = build_session_cookie(&state, session.id.to_string());

    let response = AuthSessionResponse {
        user_id: user.id.to_string(),
        username: user.username,
        email: user.email,
        status: user.status,
        auth_source: user.auth_source,
        expires_at: session.expires_at.to_rfc3339(),
        roles,
        capabilities,
    };

    (jar.add(cookie), Json(response)).into_response()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LicenseAuthFailure {
    api_code: &'static str,
    browser_code: &'static str,
    message: &'static str,
    reason_code: &'static str,
}

fn classify_license_auth_failure(error: &AppError) -> Option<LicenseAuthFailure> {
    let AppError::Forbidden(message) = error else {
        return None;
    };

    if message.starts_with(LICENSE_ERROR_PREFIX_SEAT_LIMIT_EXCEEDED) {
        return Some(LicenseAuthFailure {
            api_code: LICENSE_ERROR_CODE_SEAT_LIMIT_EXCEEDED,
            browser_code: OIDC_ERROR_CODE_LICENSE_SEAT_LIMIT,
            message: LICENSE_MESSAGE_SEAT_LIMIT,
            reason_code: reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED,
        });
    }
    if message.starts_with(LICENSE_ERROR_PREFIX_REFRESH_REQUIRED) {
        return Some(LicenseAuthFailure {
            api_code: LICENSE_ERROR_CODE_REFRESH_REQUIRED,
            browser_code: OIDC_ERROR_CODE_LICENSE_REFRESH_REQUIRED,
            message: LICENSE_MESSAGE_REFRESH_REQUIRED,
            reason_code: reason_codes::ENTITLEMENT_EXPIRED,
        });
    }
    if message.starts_with(LICENSE_ERROR_PREFIX_GRACE_ENDED) {
        return Some(LicenseAuthFailure {
            api_code: LICENSE_ERROR_CODE_GRACE_ENDED,
            browser_code: OIDC_ERROR_CODE_LICENSE_GRACE_ENDED,
            message: LICENSE_MESSAGE_GRACE_ENDED,
            reason_code: reason_codes::ENTITLEMENT_EXPIRED,
        });
    }
    if message.starts_with(LICENSE_ERROR_PREFIX_ENTITLEMENT_BINDING_INVALID)
        || message.starts_with(reason_codes::ENTITLEMENT_BINDING_INVALID)
    {
        return Some(license_unavailable_failure());
    }

    Some(license_unavailable_failure())
}

fn license_unavailable_failure() -> LicenseAuthFailure {
    LicenseAuthFailure {
        api_code: LICENSE_ERROR_CODE_UNAVAILABLE,
        browser_code: OIDC_ERROR_CODE_LICENSE_UNAVAILABLE,
        message: LICENSE_MESSAGE_UNAVAILABLE,
        reason_code: reason_codes::ENTITLEMENT_BINDING_INVALID,
    }
}

fn forbidden_license_response(failure: LicenseAuthFailure) -> Response {
    build_error_response(
        StatusCode::FORBIDDEN,
        failure.api_code,
        failure.message,
        failure.reason_code,
    )
}

fn map_license_error_response(error: AppError) -> Response {
    if let Some(failure) = classify_license_auth_failure(&error) {
        return forbidden_license_response(failure);
    }
    handle_app_error(error)
}

async fn confirm_login_after_session(
    state: &AppState,
    _auth_service: &AuthService,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    state
        .services
        .license_login_gate
        .confirm_login(user_id, session_id)
        .await
}

async fn finalize_local_login(
    state: &AppState,
    auth_service: &AuthService,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<(), Box<Response>> {
    if let Err(error) = confirm_login_after_session(state, auth_service, session_id, user_id).await
    {
        if let Err(revoke_error) = auth_service.logout(session_id, chrono::Utc::now()).await {
            return Err(Box::new(handle_app_error(revoke_error)));
        }
        return Err(Box::new(map_license_error_response(error)));
    }
    Ok(())
}

/// Begin OIDC login by redirecting to the provider.
#[utoipa::path(
    get,
    path = "/api/v1/auth/login",
    tag = "auth",
    params(
        ("redirect_to" = Option<String>, Query, description = "Internal redirect target after successful OIDC login"),
    ),
    responses(
        (status = 307, description = "Redirect to the configured OIDC authorization endpoint"),
        (status = 400, description = "Invalid OIDC login request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn oidc_login_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<OidcLoginQuery>,
    headers: HeaderMap,
) -> Response {
    let effective = match load_enabled_oidc_config(&state).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let redirect_to = match normalize_redirect_to(query.redirect_to.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let login_state = match oidc::build_login_state(&effective.auth, &effective.client_secret).await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let state_data = OidcStateData {
        nonce: login_state.nonce.clone(),
        pkce_verifier: login_state.pkce_verifier.clone(),
        redirect_to,
    };
    let mut redis = state.redis.clone();
    let state_key = oidc::state_cache_key(&login_state.state);
    let serialized = match serde_json::to_string(&state_data) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationJson(error)),
    };
    if let Err(error) = redis_cache::set(
        &mut redis,
        &state_key,
        &serialized,
        Some(OIDC_STATE_TTL_SECONDS),
    )
    .await
    {
        return handle_app_error(error);
    }

    let state_cookie = build_state_cookie(&state, login_state.state.clone());
    let redirect_url = rewrite_oidc_authorization_url(&login_state.auth_url, &headers);

    (jar.add(state_cookie), Redirect::temporary(&redirect_url)).into_response()
}

/// Complete the OIDC login redirect and open a session.
#[utoipa::path(
    get,
    path = "/api/v1/auth/sso/callback",
    tag = "auth",
    params(
        ("code" = Option<String>, Query, description = "OIDC authorization code"),
        ("state" = Option<String>, Query, description = "OIDC state returned by the provider"),
        ("error" = Option<String>, Query, description = "OIDC provider error code"),
        ("error_description" = Option<String>, Query, description = "OIDC provider error description"),
    ),
    responses(
        (status = 303, description = "Redirect to the application after OIDC callback handling"),
        (status = 400, description = "Invalid OIDC callback payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn oidc_callback_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<OidcCallbackQuery>,
) -> Response {
    let effective = match load_enabled_oidc_config(&state).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let cookie_state = jar
        .get(OIDC_STATE_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string());
    let ValidatedOidcCallback {
        code,
        state: state_value,
    } = match validate_oidc_callback_query(query, cookie_state.as_deref()) {
        Ok(callback) => callback,
        Err(error) => {
            return callback_error_response(error.code, None, error.message, jar, &state);
        }
    };

    let state_data = match consume_oidc_login_state(&state, &state_value, &jar).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    let redirect_to = normalize_redirect_to(state_data.redirect_to.as_deref()).unwrap_or_default();

    let claims = match oidc::exchange_code_for_claims(
        &effective.auth,
        &effective.client_secret,
        &code,
        &state_data.pkce_verifier,
        &state_data.nonce,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return callback_error_response(
                map_oidc_error_code(&error),
                redirect_to.as_deref(),
                error.to_string(),
                jar,
                &state,
            );
        }
    };

    let auth_service = build_auth_service(&state);
    let user = match auth_service
        .authenticate_oidc(
            &claims.subject,
            claims.preferred_username.as_deref(),
            claims.email.as_deref(),
            &claims.groups,
            effective.provisioning_mode,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            let identifier = claims
                .email
                .as_deref()
                .or(claims.preferred_username.as_deref());
            return reject_oidc_signin(&state, jar, redirect_to.as_deref(), identifier, error)
                .await;
        }
    };
    invalidate_oidc_membership_policy_cache(&state, user.id).await;

    if let Err(error) = state
        .services
        .license_login_gate
        .enforce_login(user.id)
        .await
    {
        let identifier = claims
            .email
            .as_deref()
            .or(claims.preferred_username.as_deref());
        return reject_license_gated_signin(
            &state,
            jar,
            redirect_to.as_deref(),
            identifier,
            user.id,
            error,
        )
        .await;
    }

    open_oidc_session(&state, &auth_service, jar, user.id, claims, redirect_to).await
}

/// Commit the successful sign-in before issuing cookies and the application redirect.
async fn open_oidc_session(
    state: &AppState,
    auth_service: &AuthService,
    jar: CookieJar,
    user_id: Uuid,
    claims: oidc::OidcClaims,
    redirect_to: Option<String>,
) -> Response {
    let audit_event = build_signin_timeline_event(SigninTimelineEvent {
        organization_id: None,
        actor_user_id: Some(user_id),
        result: OperationTimelineResult::Success,
        reason_code: None,
        identifier: claims
            .email
            .as_deref()
            .or(claims.preferred_username.as_deref()),
        auth_method: "oidc",
    });
    let session = match auth_service
        .open_session(user_id, audit_event.clone())
        .await
    {
        Ok(session) => session,
        Err(error) => return handle_app_error(error),
    };

    // Same seat-gate/session creation race as the local login path. Confirm
    // after the session exists, advance the LIFO timestamp only on success, and
    // reject a session that was revoked before capacity returned.
    if let Err(error) = confirm_login_after_session(state, auth_service, session.id, user_id).await
    {
        if let Err(revoke_error) = auth_service.logout(session.id, chrono::Utc::now()).await {
            return handle_app_error(revoke_error);
        }
        let identifier = claims
            .email
            .as_deref()
            .or(claims.preferred_username.as_deref());
        return reject_license_gated_signin(
            state,
            jar,
            redirect_to.as_deref(),
            identifier,
            user_id,
            error,
        )
        .await;
    }

    state.services.audit.fanout_recorded_event(audit_event);

    let metadata = OidcSessionMetadata {
        auth_method: "oidc".to_string(),
        oidc_groups: claims.groups,
    };
    let mut redis = state.redis.clone();
    let metadata_key = oidc::session_cache_key(&session.id);
    if let Ok(serialized) = serde_json::to_string(&metadata) {
        let _ = redis_cache::set(
            &mut redis,
            &metadata_key,
            &serialized,
            Some(state.settings.auth.session_ttl_seconds),
        )
        .await;
    }

    let state_removal_cookie = build_state_removal_cookie(state);
    let session_cookie = build_session_cookie(state, session.id.to_string());

    let redirect_to = redirect_to.unwrap_or_else(|| AUTH_DEFAULT_REDIRECT_PATH.to_string());

    (
        jar.add(state_removal_cookie).add(session_cookie),
        Redirect::to(&redirect_to),
    )
        .into_response()
}

/// Consume the one-time callback state before any code exchange or session creation.
async fn consume_oidc_login_state(
    state: &AppState,
    state_value: &str,
    jar: &CookieJar,
) -> Result<OidcStateData, Box<Response>> {
    let mut redis = state.redis.clone();
    let state_key = oidc::state_cache_key(state_value);
    let state_payload = match redis_cache::get(&mut redis, &state_key).await {
        Ok(Some(payload)) => payload,
        Ok(None) => {
            return Err(Box::new(callback_error_response(
                OIDC_ERROR_CODE_INVALID_STATE,
                None,
                "OIDC login state has expired".to_string(),
                jar.clone(),
                state,
            )));
        }
        Err(error) => {
            return Err(Box::new(callback_error_response(
                OIDC_ERROR_CODE_SERVER_ERROR,
                None,
                error.to_string(),
                jar.clone(),
                state,
            )));
        }
    };
    let state_data = serde_json::from_str(&state_payload)
        .map_err(|error| Box::new(handle_app_error(AppError::ValidationJson(error))))?;
    let _ = redis_cache::delete(&mut redis, &state_key).await;
    Ok(state_data)
}

/// Record a rejected OIDC sign-in and return the callback error redirect.
async fn reject_oidc_signin(
    state: &AppState,
    jar: CookieJar,
    redirect_to: Option<&str>,
    identifier: Option<&str>,
    error: AppError,
) -> Response {
    let timeline = OperationTimelineService::new(state.services.audit.clone());
    let reason_code = auth_timeline_reason_code(&error);
    if let Err(audit_error) = record_signin_timeline_event(
        &timeline,
        SigninTimelineEvent {
            organization_id: None,
            actor_user_id: None,
            result: OperationTimelineResult::Rejected,
            reason_code,
            identifier,
            auth_method: "oidc",
        },
    )
    .await
    {
        return handle_app_error(audit_error);
    }
    callback_error_response(
        map_oidc_error_code(&error),
        redirect_to,
        error.to_string(),
        jar,
        state,
    )
}

/// Record a license-gated OIDC rejection and return the callback error redirect.
async fn reject_license_gated_signin(
    state: &AppState,
    jar: CookieJar,
    redirect_to: Option<&str>,
    identifier: Option<&str>,
    user_id: Uuid,
    error: AppError,
) -> Response {
    let timeline = OperationTimelineService::new(state.services.audit.clone());
    let license_failure = classify_license_auth_failure(&error);
    let reason_code = license_failure
        .map(|failure| failure.reason_code)
        .or_else(|| auth_timeline_reason_code(&error));
    let browser_error_code = license_failure
        .map(|failure| failure.browser_code)
        .unwrap_or(OIDC_ERROR_CODE_SERVER_ERROR);
    if let Err(audit_error) = record_signin_timeline_event(
        &timeline,
        SigninTimelineEvent {
            organization_id: None,
            actor_user_id: Some(user_id),
            result: OperationTimelineResult::Rejected,
            reason_code,
            identifier,
            auth_method: "oidc",
        },
    )
    .await
    {
        return handle_app_error(audit_error);
    }
    callback_error_response(
        browser_error_code,
        redirect_to,
        error.to_string(),
        jar,
        state,
    )
}

/// Log out and end the current session.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 204, description = "Logout succeeded"),
        (status = 401, description = "Session logout failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn logout_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    let session_cookie_name = state.settings.auth.session_cookie_name.clone();
    let session_id = parse_session_id_from_cookie(&jar, &session_cookie_name);

    if let Some(session_id) = session_id {
        let auth_service = build_auth_service(&state);
        if let Err(error) = auth_service.logout(session_id, chrono::Utc::now()).await {
            return handle_app_error(error);
        }
        let mut redis = state.redis.clone();
        let _ = redis_cache::delete(&mut redis, &oidc::session_cache_key(&session_id)).await;
    }

    let removal_cookie = build_session_removal_cookie(&state);

    (jar.add(removal_cookie), StatusCode::NO_CONTENT).into_response()
}

/// Resolve the authenticated principal's role names and type-level capability
/// grid for the session-shaped responses (`/auth/login`, `/auth/me`). Lives in
/// the handler module but delegates all data access to the access-control
/// application service (no SQL in handlers).
async fn session_authorization(
    state: &AppState,
    user_id: Uuid,
) -> Result<(Vec<String>, Vec<CapabilityDto>), AppError> {
    let effective = access_control_service(state)
        .effective_authorization(&Principal::user(user_id))
        .await?;
    let capabilities = effective
        .capabilities
        .into_iter()
        .map(|capability| CapabilityDto {
            action: capability.action,
            resource_type: capability.resource_type,
        })
        .collect();
    Ok((effective.roles, capabilities))
}

/// Get the currently authenticated user.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "Current authenticated session", body = AuthSessionResponse),
        (status = 401, description = "Missing or invalid session", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn me_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    let session_cookie_name = state.settings.auth.session_cookie_name.clone();
    let session_id = parse_session_id_from_cookie(&jar, &session_cookie_name);

    let session_id = match session_id {
        Some(session_id) => session_id,
        None => return handle_app_error(AppError::Auth("Missing session".to_string())),
    };

    let auth_service = build_auth_service(&state);
    match auth_service
        .resolve_session(session_id, chrono::Utc::now())
        .await
    {
        Ok(Some((user, session))) => {
            let (roles, capabilities) = match session_authorization(&state, user.id).await {
                Ok(authorization) => authorization,
                Err(error) => return handle_app_error(error),
            };
            let response = AuthSessionResponse {
                user_id: user.id.to_string(),
                username: user.username,
                email: user.email,
                status: user.status,
                auth_source: user.auth_source,
                expires_at: session.expires_at.to_rfc3339(),
                roles,
                capabilities,
            };
            Json(response).into_response()
        }
        Ok(None) => handle_app_error(AppError::Auth("Invalid session".to_string())),
        Err(error) => handle_app_error(error),
    }
}

fn session_refresh_response(
    status: &'static str,
    message: Option<&str>,
    redirect: Option<&str>,
) -> SessionRefreshResponse {
    SessionRefreshResponse {
        status: status.to_string(),
        message: message.map(|value| value.to_string()),
        redirect: redirect.map(|value| value.to_string()),
    }
}

fn session_refresh_expired_response(jar: CookieJar, message: &str) -> Response {
    let body = session_refresh_response(
        SESSION_REFRESH_STATUS_EXPIRED,
        Some(message),
        Some(AUTH_LOGIN_REDIRECT_PATH),
    );
    (StatusCode::UNAUTHORIZED, (jar, Json(body))).into_response()
}

/// Refreshes the current session for frontend 401 handling.
///
/// For both local and OIDC auth: validates session, extends TTL, returns refreshed cookie.
/// Returns 200 with status "refreshed" on success; 401 with status "expired" on failure.
#[utoipa::path(
    post,
    path = "/api/v1/auth/session-refresh",
    tag = "auth",
    responses(
        (status = 200, description = "Session refreshed", body = SessionRefreshResponse),
        (status = 401, description = "Session expired", body = SessionRefreshResponse),
    ),
)]
pub async fn session_refresh_handler(State(state): State<AppState>, jar: CookieJar) -> Response {
    let span = operation_span("auth.session_refresh", &[]);
    instrument_span(span, async move {
        let session_cookie_name = state.settings.auth.session_cookie_name.clone();
        let session_id = parse_session_id_from_cookie(&jar, &session_cookie_name);

        let session_id = match session_id {
            Some(id) => id,
            None => {
                log_event(
                    LogLevel::Info,
                    "rest.auth.session_refresh.completed",
                    EventFields::default()
                        .with_operation("rest.auth.session_refresh")
                        .with_resource("session_cookie", session_cookie_name.clone())
                        .with_status(SESSION_REFRESH_STATUS_EXPIRED)
                        .with_message("No session cookie found"),
                );
                return session_refresh_expired_response(jar, "No session found");
            }
        };

        let auth_service = build_auth_service(&state);
        let now = chrono::Utc::now();

        match auth_service.refresh_session(session_id, now).await {
            Ok(Some((user, session))) => {
                log_event(
                    LogLevel::Info,
                    "rest.auth.session_refresh.completed",
                    EventFields::default()
                        .with_operation("rest.auth.session_refresh")
                        .with_resource("session", session.id.to_string())
                        .with_user_id(user.id.to_string())
                        .with_status(SESSION_REFRESH_STATUS_REFRESHED)
                        .with_message("Session token refreshed successfully"),
                );
                let cookie = build_session_cookie(&state, session.id.to_string());
                let body = session_refresh_response(
                    SESSION_REFRESH_STATUS_REFRESHED,
                    Some("Session token refreshed successfully"),
                    None,
                );
                (jar.add(cookie), Json(body)).into_response()
            }
            Ok(None) => {
                log_event(
                    LogLevel::Info,
                    "rest.auth.session_refresh.completed",
                    EventFields::default()
                        .with_operation("rest.auth.session_refresh")
                        .with_resource("session", session_id.to_string())
                        .with_status(SESSION_REFRESH_STATUS_EXPIRED)
                        .with_message("Session expired or invalid"),
                );
                session_refresh_expired_response(
                    jar,
                    "Session expired or refresh failed. Please log in again.",
                )
            }
            Err(error) => handle_app_error(error),
        }
    })
    .await
}

/// List the available authentication providers.
#[utoipa::path(
    get,
    path = "/api/v1/auth/providers",
    tag = "auth",
    responses(
        (status = 200, description = "Available authentication providers", body = AuthProvidersResponse),
    ),
)]
pub async fn auth_providers_handler(State(state): State<AppState>) -> Response {
    let oidc_enabled = match state
        .services
        .oidc_settings
        .get_effective(&state.settings.auth)
        .await
    {
        Ok(value) => value.auth.oidc_enabled,
        Err(_) => state.settings.auth.oidc_enabled,
    };
    let response = AuthProvidersResponse {
        local_enabled: state.settings.auth.local_enabled,
        oidc_enabled,
        ldap_enabled: false,
        ad_enabled: false,
    };
    Json(response).into_response()
}

fn normalize_redirect_to(input: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(input) = input else {
        return Ok(None);
    };
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    if input.contains("://") || input.starts_with("//") {
        return Err(AppError::Validation(
            "Invalid redirect target; only internal paths are allowed".to_string(),
        ));
    }

    let normalized = if input.starts_with('/') {
        input.to_string()
    } else {
        format!("{}{}", AUTH_COOKIE_PATH, input)
    };
    if normalized.starts_with("/api/") {
        return Err(AppError::Validation(
            "Invalid redirect target; API routes are not allowed".to_string(),
        ));
    }
    if normalized.starts_with(AUTH_LOGIN_REDIRECT_PATH)
        || normalized.starts_with(AUTH_REDIRECT_PATH)
    {
        return Ok(Some(AUTH_DEFAULT_REDIRECT_PATH.to_string()));
    }
    if normalized.len() > AUTH_INTERNAL_REDIRECT_MAX_LENGTH {
        return Err(AppError::Validation(
            "Invalid redirect target; path is too long".to_string(),
        ));
    }

    Ok(Some(normalized))
}

fn map_oidc_error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Validation(_) | AppError::ValidationJson(_) | AppError::ValidationUuid(_) => {
            OIDC_ERROR_CODE_INVALID_REQUEST
        }
        AppError::Auth(message) => {
            let lowered = message.to_ascii_lowercase();
            if lowered.contains("state") {
                OIDC_ERROR_CODE_INVALID_STATE
            } else {
                OIDC_ERROR_CODE_AUTH_FAILED
            }
        }
        AppError::External(_) | AppError::ExternalSource(_) | AppError::Provider(_) => {
            OIDC_ERROR_CODE_SERVER_ERROR
        }
        _ => OIDC_ERROR_CODE_AUTH_FAILED,
    }
}

fn validate_oidc_callback_query(
    query: OidcCallbackQuery,
    cookie_state: Option<&str>,
) -> Result<ValidatedOidcCallback, OidcCallbackValidationError> {
    if let Some(error) = query.error {
        let message = query.error_description.map_or_else(
            || format!("OIDC error: {error}"),
            |description| format!("OIDC error: {error} ({description})"),
        );
        return Err(OidcCallbackValidationError {
            code: OIDC_ERROR_CODE_AUTH_FAILED,
            message,
        });
    }

    let code = query
        .code
        .filter(|code| !code.trim().is_empty())
        .ok_or_else(|| OidcCallbackValidationError {
            code: OIDC_ERROR_CODE_INVALID_REQUEST,
            message: "Missing OIDC code".to_string(),
        })?;
    let state = query
        .state
        .filter(|state| !state.trim().is_empty())
        .ok_or_else(|| OidcCallbackValidationError {
            code: OIDC_ERROR_CODE_INVALID_STATE,
            message: "Missing OIDC state".to_string(),
        })?;

    if cookie_state != Some(state.as_str()) {
        return Err(OidcCallbackValidationError {
            code: OIDC_ERROR_CODE_INVALID_STATE,
            message: "Invalid OIDC state".to_string(),
        });
    }

    Ok(ValidatedOidcCallback { code, state })
}

fn callback_error_redirect(
    error_code: &str,
    redirect: Option<&str>,
    message: String,
    jar: CookieJar,
    state: &AppState,
) -> (CookieJar, Redirect) {
    log_event(
        LogLevel::Info,
        "rest.auth.oidc_callback.failed",
        EventFields::default()
            .with_operation("rest.auth.oidc_callback")
            .with_resource("oidc_callback", error_code.to_string())
            .with_status(error_code.to_string())
            .with_message(message.clone()),
    );
    let mut target = format!("{}?error={}", AUTH_REDIRECT_PATH, error_code);
    if let Some(redirect) = redirect {
        target.push_str(AUTH_REDIRECT_QUERY_PARAM);
        target.push_str(redirect);
    }
    let state_removal_cookie = build_state_removal_cookie(state);
    (jar.add(state_removal_cookie), Redirect::to(&target))
}

fn callback_error_response(
    error_code: &str,
    redirect: Option<&str>,
    message: String,
    jar: CookieJar,
    state: &AppState,
) -> Response {
    callback_error_redirect(error_code, redirect, message, jar, state).into_response()
}

/// Fields shared by the sign-in timeline event builders.
struct SigninTimelineEvent<'a> {
    organization_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    result: OperationTimelineResult,
    reason_code: Option<&'static str>,
    identifier: Option<&'a str>,
    auth_method: &'a str,
}

async fn record_signin_timeline_event<R>(
    timeline: &OperationTimelineService<R>,
    event: SigninTimelineEvent<'_>,
) -> Result<(), AppError>
where
    R: AuditRecorder,
{
    timeline.write(signin_timeline_write_request(event)).await?;
    Ok(())
}

fn build_signin_timeline_event(
    event: SigninTimelineEvent<'_>,
) -> crate::contexts::audit::domain::AuditEvent {
    build_operation_timeline_event(signin_timeline_write_request(event))
}

fn signin_timeline_write_request(event: SigninTimelineEvent<'_>) -> OperationTimelineWriteRequest {
    let SigninTimelineEvent {
        organization_id,
        actor_user_id,
        result,
        reason_code,
        identifier,
        auth_method,
    } = event;
    let action_kind = match result {
        OperationTimelineResult::Success => OperationTimelineActionKind::SigninCompleted,
        OperationTimelineResult::Rejected => OperationTimelineActionKind::SigninRejected,
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert("auth_method".to_string(), json!(auth_method));
    if let Some(identifier) = identifier.filter(|value| !value.trim().is_empty()) {
        metadata.insert("identifier".to_string(), json!(identifier));
    }
    OperationTimelineWriteRequest {
        organization_id,
        actor_user_id,
        action_kind,
        result,
        reason_code: reason_code.map(str::to_string),
        metadata: serde_json::Value::Object(metadata),
    }
}

fn auth_timeline_reason_code(error: &AppError) -> Option<&'static str> {
    match error {
        AppError::Auth(message) if message == "Invalid credentials" => {
            Some(reason_codes::AUTH_INVALID_CREDENTIALS)
        }
        AppError::Auth(message) if message == "User is inactive" => {
            Some(reason_codes::AUTH_ACCOUNT_DISABLED)
        }
        AppError::Forbidden(message)
            if message.starts_with(LICENSE_ERROR_PREFIX_SEAT_LIMIT_EXCEEDED) =>
        {
            Some(reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED)
        }
        AppError::Forbidden(message)
            if message.starts_with(LICENSE_ERROR_PREFIX_REFRESH_REQUIRED)
                || message.starts_with(LICENSE_ERROR_PREFIX_GRACE_ENDED) =>
        {
            Some(reason_codes::ENTITLEMENT_EXPIRED)
        }
        AppError::Forbidden(message)
            if message.starts_with(LICENSE_ERROR_PREFIX_ENTITLEMENT_BINDING_INVALID)
                || message.starts_with(reason_codes::ENTITLEMENT_BINDING_INVALID) =>
        {
            Some(reason_codes::ENTITLEMENT_BINDING_INVALID)
        }
        _ => None,
    }
}

fn is_oidc_rewritable_authority(authority: &str) -> bool {
    OIDC_REWRITABLE_AUTHORITIES
        .iter()
        .any(|candidate| authority.starts_with(candidate))
}

fn non_empty_header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod timeline_tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::contexts::audit::domain::AuditEvent;

    #[derive(Clone, Default)]
    struct TestRecorder {
        events: Arc<Mutex<Vec<AuditEvent>>>,
    }

    #[async_trait]
    impl AuditRecorder for TestRecorder {
        async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("test recorder mutex should not be poisoned")
                .push(event);
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    struct FailingRecorder;

    #[async_trait]
    impl AuditRecorder for FailingRecorder {
        async fn record_event(&self, _event: AuditEvent) -> Result<(), AppError> {
            Err(AppError::Database(
                "simulated sign-in audit failure".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn rejected_local_and_oidc_signin_paths_propagate_audit_failures() {
        let timeline = OperationTimelineService::new(FailingRecorder);
        let user_id = Uuid::now_v7();
        let cases = [
            (None, None, None, "local"),
            (
                None,
                None,
                Some(reason_codes::AUTH_INVALID_CREDENTIALS),
                "local",
            ),
            (
                Some(user_id),
                Some("owner@example.test"),
                Some(reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED),
                "local",
            ),
            (
                None,
                None,
                Some(reason_codes::AUTH_INVALID_CREDENTIALS),
                "oidc",
            ),
            (
                Some(user_id),
                Some("owner@example.test"),
                Some(reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED),
                "oidc",
            ),
        ];

        for (actor_user_id, identifier, reason_code, auth_method) in cases {
            let error = record_signin_timeline_event(
                &timeline,
                SigninTimelineEvent {
                    organization_id: None,
                    actor_user_id,
                    result: OperationTimelineResult::Rejected,
                    reason_code,
                    identifier,
                    auth_method,
                },
            )
            .await
            .expect_err("a rejected sign-in must propagate its audit failure");
            assert!(matches!(error, AppError::Database(_)));
        }
    }

    #[tokio::test]
    async fn record_signin_timeline_event_omits_empty_identifier_for_oidc() {
        let recorder = TestRecorder::default();
        let timeline = OperationTimelineService::new(recorder.clone());

        record_signin_timeline_event(
            &timeline,
            SigninTimelineEvent {
                organization_id: None,
                actor_user_id: None,
                result: OperationTimelineResult::Rejected,
                reason_code: Some(reason_codes::AUTH_INVALID_CREDENTIALS),
                identifier: None,
                auth_method: "oidc",
            },
        )
        .await
        .expect("timeline write should succeed");

        let events = recorder
            .events
            .lock()
            .expect("test recorder mutex should not be poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "timeline.signin_rejected");
        assert_eq!(
            events[0].metadata["reason_code"],
            json!(reason_codes::AUTH_INVALID_CREDENTIALS)
        );
        assert_eq!(events[0].metadata["auth_method"], json!("oidc"));
        assert!(
            events[0].metadata.get("identifier").is_none(),
            "oidc metadata should omit identifier when it is unavailable"
        );
    }

    #[test]
    fn auth_timeline_reason_code_maps_auth_and_license_failures() {
        assert_eq!(
            auth_timeline_reason_code(&AppError::Auth("Invalid credentials".to_string())),
            Some(reason_codes::AUTH_INVALID_CREDENTIALS)
        );
        assert_eq!(
            auth_timeline_reason_code(&AppError::Auth("User is inactive".to_string())),
            Some(reason_codes::AUTH_ACCOUNT_DISABLED)
        );
        assert_eq!(
            auth_timeline_reason_code(&AppError::Forbidden(format!(
                "{LICENSE_ERROR_PREFIX_SEAT_LIMIT_EXCEEDED}10"
            ))),
            Some(reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED)
        );
        assert_eq!(
            auth_timeline_reason_code(&AppError::Forbidden(format!(
                "{LICENSE_ERROR_PREFIX_REFRESH_REQUIRED}2026-03-01"
            ))),
            Some(reason_codes::ENTITLEMENT_EXPIRED)
        );
    }
}

fn rewrite_oidc_authorization_url(auth_url: &str, headers: &HeaderMap) -> String {
    let Some(scheme_separator) = auth_url.find("://") else {
        return auth_url.to_string();
    };
    let host_start = scheme_separator + 3;
    let Some(path_offset) = auth_url[host_start..].find('/') else {
        return auth_url.to_string();
    };
    let path_start = host_start + path_offset;
    let authority = &auth_url[host_start..path_start];
    if !is_oidc_rewritable_authority(authority) {
        return auth_url.to_string();
    }

    let host = non_empty_header_value(headers, "x-forwarded-host")
        .or_else(|| non_empty_header_value(headers, "host"));
    let Some(host) = host else {
        return auth_url.to_string();
    };

    let scheme =
        non_empty_header_value(headers, "x-forwarded-proto").unwrap_or(DEFAULT_FORWARDED_PROTO);
    let path_query = &auth_url[path_start..];
    format!("{}://{}{}", scheme, host, path_query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_oidc_callback_query_preserves_validation_contract() {
        struct Case {
            name: &'static str,
            query: OidcCallbackQuery,
            cookie_state: Option<&'static str>,
            expected: Result<ValidatedOidcCallback, OidcCallbackValidationError>,
        }

        let cases = [
            Case {
                name: "provider error with description takes precedence",
                query: OidcCallbackQuery {
                    code: None,
                    state: None,
                    error: Some("access_denied".to_string()),
                    error_description: Some("user cancelled".to_string()),
                },
                cookie_state: None,
                expected: Err(OidcCallbackValidationError {
                    code: OIDC_ERROR_CODE_AUTH_FAILED,
                    message: "OIDC error: access_denied (user cancelled)".to_string(),
                }),
            },
            Case {
                name: "provider error without description",
                query: OidcCallbackQuery {
                    code: None,
                    state: None,
                    error: Some("access_denied".to_string()),
                    error_description: None,
                },
                cookie_state: None,
                expected: Err(OidcCallbackValidationError {
                    code: OIDC_ERROR_CODE_AUTH_FAILED,
                    message: "OIDC error: access_denied".to_string(),
                }),
            },
            Case {
                name: "missing code precedes missing state",
                query: OidcCallbackQuery {
                    code: Some("  ".to_string()),
                    state: None,
                    error: None,
                    error_description: None,
                },
                cookie_state: None,
                expected: Err(OidcCallbackValidationError {
                    code: OIDC_ERROR_CODE_INVALID_REQUEST,
                    message: "Missing OIDC code".to_string(),
                }),
            },
            Case {
                name: "missing state",
                query: OidcCallbackQuery {
                    code: Some("code".to_string()),
                    state: Some("  ".to_string()),
                    error: None,
                    error_description: None,
                },
                cookie_state: None,
                expected: Err(OidcCallbackValidationError {
                    code: OIDC_ERROR_CODE_INVALID_STATE,
                    message: "Missing OIDC state".to_string(),
                }),
            },
            Case {
                name: "state cookie mismatch",
                query: OidcCallbackQuery {
                    code: Some("code".to_string()),
                    state: Some("callback-state".to_string()),
                    error: None,
                    error_description: None,
                },
                cookie_state: Some("cookie-state"),
                expected: Err(OidcCallbackValidationError {
                    code: OIDC_ERROR_CODE_INVALID_STATE,
                    message: "Invalid OIDC state".to_string(),
                }),
            },
            Case {
                name: "valid input is preserved verbatim",
                query: OidcCallbackQuery {
                    code: Some(" code ".to_string()),
                    state: Some(" state ".to_string()),
                    error: None,
                    error_description: None,
                },
                cookie_state: Some(" state "),
                expected: Ok(ValidatedOidcCallback {
                    code: " code ".to_string(),
                    state: " state ".to_string(),
                }),
            },
        ];

        for case in cases {
            assert_eq!(
                validate_oidc_callback_query(case.query, case.cookie_state),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn normalize_redirect_to_rejects_external_and_api_routes() {
        assert!(normalize_redirect_to(Some("https://example.com")).is_err());
        assert!(normalize_redirect_to(Some("//example.com")).is_err());
        assert!(normalize_redirect_to(Some("/api/users")).is_err());
    }

    #[test]
    fn normalize_redirect_to_normalizes_and_clamps_auth_paths() {
        assert_eq!(
            normalize_redirect_to(Some("workspace/targets")).expect("should parse"),
            Some("/workspace/targets".to_string())
        );
        assert_eq!(
            normalize_redirect_to(Some("/auth/login")).expect("should parse"),
            Some(AUTH_DEFAULT_REDIRECT_PATH.to_string())
        );
        assert_eq!(
            normalize_redirect_to(Some("/auth/redirect?error=invalid_state"))
                .expect("should parse"),
            Some(AUTH_DEFAULT_REDIRECT_PATH.to_string())
        );
    }

    #[test]
    fn normalize_redirect_to_rejects_overlong_path() {
        let over_limit = format!("/{}", "a".repeat(AUTH_INTERNAL_REDIRECT_MAX_LENGTH + 1));
        assert!(normalize_redirect_to(Some(&over_limit)).is_err());
    }

    #[test]
    fn rewrite_oidc_authorization_url_rewrites_trusted_authority() {
        let auth_url = "http://keycloak:8080/realms/main/protocol/openid-connect/auth?x=1";
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-host",
            "auth.example.test".parse().expect("valid host"),
        );
        headers.insert("x-forwarded-proto", "https".parse().expect("valid proto"));

        let rewritten = rewrite_oidc_authorization_url(auth_url, &headers);
        assert_eq!(
            rewritten,
            "https://auth.example.test/realms/main/protocol/openid-connect/auth?x=1"
        );
    }

    #[test]
    fn rewrite_oidc_authorization_url_keeps_untrusted_authority() {
        let auth_url = "http://idp.internal/realms/main/protocol/openid-connect/auth?x=1";
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-host",
            "auth.example.test".parse().expect("valid host"),
        );
        headers.insert("x-forwarded-proto", "https".parse().expect("valid proto"));

        let rewritten = rewrite_oidc_authorization_url(auth_url, &headers);
        assert_eq!(rewritten, auth_url);
    }

    #[test]
    fn map_oidc_error_code_maps_validation_and_state_errors() {
        assert_eq!(
            map_oidc_error_code(&AppError::Validation("bad".to_string())),
            OIDC_ERROR_CODE_INVALID_REQUEST
        );
        assert_eq!(
            map_oidc_error_code(&AppError::Auth("state mismatch".to_string())),
            OIDC_ERROR_CODE_INVALID_STATE
        );
        assert_eq!(
            map_oidc_error_code(&AppError::Auth("denied".to_string())),
            OIDC_ERROR_CODE_AUTH_FAILED
        );
    }

    #[test]
    fn classify_license_auth_failure_maps_safe_codes_and_messages() {
        let cases = [
            (
                AppError::Forbidden(format!("{LICENSE_ERROR_PREFIX_SEAT_LIMIT_EXCEEDED}none")),
                LICENSE_ERROR_CODE_SEAT_LIMIT_EXCEEDED,
                OIDC_ERROR_CODE_LICENSE_SEAT_LIMIT,
                reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED,
            ),
            (
                AppError::Forbidden(format!("{LICENSE_ERROR_PREFIX_REFRESH_REQUIRED}expired")),
                LICENSE_ERROR_CODE_REFRESH_REQUIRED,
                OIDC_ERROR_CODE_LICENSE_REFRESH_REQUIRED,
                reason_codes::ENTITLEMENT_EXPIRED,
            ),
            (
                AppError::Forbidden(format!("{LICENSE_ERROR_PREFIX_GRACE_ENDED}blocked")),
                LICENSE_ERROR_CODE_GRACE_ENDED,
                OIDC_ERROR_CODE_LICENSE_GRACE_ENDED,
                reason_codes::ENTITLEMENT_EXPIRED,
            ),
            (
                AppError::Forbidden(format!(
                    "{}: invalid",
                    reason_codes::ENTITLEMENT_BINDING_INVALID
                )),
                LICENSE_ERROR_CODE_UNAVAILABLE,
                OIDC_ERROR_CODE_LICENSE_UNAVAILABLE,
                reason_codes::ENTITLEMENT_BINDING_INVALID,
            ),
        ];

        for (error, api_code, browser_code, reason_code) in cases {
            let failure = classify_license_auth_failure(&error).expect("license failure");
            assert_eq!(failure.api_code, api_code);
            assert_eq!(failure.browser_code, browser_code);
            assert_eq!(failure.reason_code, reason_code);
        }
    }

    #[test]
    fn license_auth_messages_do_not_expose_internal_details() {
        let forbidden_terms = [
            "key",
            "signature",
            "jwt",
            "build",
            "activation",
            "entitlement",
        ];
        let messages = [
            LICENSE_MESSAGE_SEAT_LIMIT,
            LICENSE_MESSAGE_REFRESH_REQUIRED,
            LICENSE_MESSAGE_GRACE_ENDED,
            LICENSE_MESSAGE_UNAVAILABLE,
        ];

        for message in messages {
            let lowered = message.to_ascii_lowercase();
            for term in forbidden_terms {
                assert!(
                    !lowered.contains(term),
                    "license auth message must not expose {term}: {message}"
                );
            }
        }
    }
}
