use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::application::CreateUserApiTokenInput;
use crate::contexts::access_control::domain::{
    Action, Principal, ResourceType, UserApiToken, UserApiTokenScope,
};
use crate::contexts::access_control::infrastructure::persistence::{
    UserApiTokenListFilter, UserRepository,
};
use crate::contexts::access_control::infrastructure::persistence::repositories::user_api_token_repository::USER_API_TOKEN_COLUMNS;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use tracing::warn;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListUserApiTokensQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    /// Comma-separated `action:resource_type` scope filters.
    pub scope: Option<String>,
    /// Comma-separated token status filters.
    pub status: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateUserApiTokenRequest {
    pub name: String,
    pub scopes: Vec<UserApiTokenScopeRequest>,
    pub expires_in_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct UserApiTokenScopeRequest {
    pub action: String,
    pub resource_type: String,
    pub resource_pattern: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserApiTokenScopeResponse {
    pub action: String,
    pub resource_type: String,
    pub resource_pattern: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserApiTokenResponse {
    pub id: String,
    pub name: String,
    pub token_prefix: String,
    pub status: String,
    pub scopes: Vec<UserApiTokenScopeResponse>,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserApiTokenListResponse {
    pub items: Vec<UserApiTokenResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedUserApiTokenResponse {
    #[serde(flatten)]
    pub token: UserApiTokenResponse,
    pub token_value: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserApiTokenScopeCatalogItemResponse {
    pub action: String,
    pub resource_type: String,
    pub label: String,
    /// Canonical scope value. CCTV uses `cctv:<operation>`; other domains use
    /// `<action>:<resource_type>`.
    pub value: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserApiTokenScopeCatalogResponse {
    pub items: Vec<UserApiTokenScopeCatalogItemResponse>,
}

/// List the current user's API tokens.
#[utoipa::path(
    get,
    path = "/api/v1/user/settings/api-tokens",
    tag = "user-settings",
    params(
        ("limit" = Option<u32>, Query, description = "Page size"),
        ("offset" = Option<u32>, Query, description = "Page offset"),
        ("search" = Option<String>, Query, description = "Search token name, prefix, or scope"),
        ("scope" = Option<String>, Query, description = "Comma-separated action:resource_type scope filter"),
        ("status" = Option<String>, Query, description = "Comma-separated token status filter"),
        ("sort" = Option<String>, Query, description = "Allowlisted sort key"),
        ("order" = Option<String>, Query, description = "Sort direction: asc or desc"),
    ),
    responses(
        (status = 200, description = "User API tokens", body = UserApiTokenListResponse),
        (status = 400, description = "Invalid API token list query", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_user_api_tokens(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListUserApiTokensQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let limit = clamp_limit(query.limit);
    let offset = clamp_offset(query.offset);
    let scopes = match parse_scope_filter_list(query.scope.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        USER_API_TOKEN_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.user_api_tokens.clone();
    match service
        .list_for_user(
            user_id,
            limit,
            offset,
            UserApiTokenListFilter {
                search: normalize_search(query.search),
                scopes,
                status,
                sort,
            },
        )
        .await
    {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(UserApiTokenListResponse {
                items: items.into_iter().map(map_token).collect(),
                total,
                limit,
                offset,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a user API token.
#[utoipa::path(
    post,
    path = "/api/v1/user/settings/api-tokens",
    tag = "user-settings",
    request_body = CreateUserApiTokenRequest,
    responses(
        (status = 201, description = "User API token created", body = CreatedUserApiTokenResponse),
        (status = 400, description = "Invalid API token payload", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Requested scope exceeds the issuer's own permissions", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_user_api_token(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateUserApiTokenRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let scopes = match payload
        .scopes
        .iter()
        .map(parse_scope)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if let Err(error) =
        sync_oidc_group_memberships_for_token_issuer(&state, user_id, &request_context).await
    {
        return handle_app_error(error);
    }

    if let Err(error) = ensure_issuer_grants_requested_scopes(&state, user_id, &scopes).await {
        return handle_app_error(error);
    }

    let service = state.services.user_api_tokens.clone();
    match service
        .create_for_user(
            user_id,
            CreateUserApiTokenInput {
                name: payload.name,
                scopes,
                expires_in_seconds: payload.expires_in_seconds,
            },
            Utc::now(),
        )
        .await
    {
        Ok(result) => (
            StatusCode::CREATED,
            Json(CreatedUserApiTokenResponse {
                token: map_token(result.token),
                token_value: result.raw_token,
            }),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Revoke a user API token.
#[utoipa::path(
    post,
    path = "/api/v1/user/settings/api-tokens/{id}/revoke",
    tag = "user-settings",
    params(
        ("id" = String, Path, description = "API token UUID"),
    ),
    responses(
        (status = 200, description = "User API token revoked", body = UserApiTokenResponse),
        (status = 400, description = "Invalid API token UUID", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "API token not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn revoke_user_api_token(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(token_id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let token_id = match Uuid::parse_str(token_id.as_str()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(AppError::ValidationUuid(error)),
    };

    let service = state.services.user_api_tokens.clone();
    match service.revoke_for_user(user_id, token_id, Utc::now()).await {
        Ok(token) => (StatusCode::OK, Json(map_token(token))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List the scopes available for user API tokens.
#[utoipa::path(
    get,
    path = "/api/v1/user/settings/api-tokens/scope-catalog",
    tag = "user-settings",
    responses(
        (status = 200, description = "User API token scope catalog", body = UserApiTokenScopeCatalogResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_user_api_token_scope_catalog(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if request_context.require_authenticated_user_id().is_err() {
        return handle_app_error(AppError::Auth("Authentication required".to_string()));
    }

    let service = state.services.user_api_tokens.clone();
    let items = service
        .scope_catalog()
        .into_iter()
        .map(|scope| {
            let (action, resource_type) =
                if scope.resource_type == ResourceType::Cctv && scope.action.is_cctv_permission() {
                    ("cctv".to_string(), action_key(&scope.action).to_string())
                } else {
                    (
                        action_key(&scope.action).to_string(),
                        scope.resource_type.as_str().to_string(),
                    )
                };
            UserApiTokenScopeCatalogItemResponse {
                action,
                resource_type,
                label: scope_wire_value(&scope.action, &scope.resource_type),
                value: scope_wire_value(&scope.action, &scope.resource_type),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(UserApiTokenScopeCatalogResponse { items }),
    )
        .into_response()
}

fn map_token(value: UserApiToken) -> UserApiTokenResponse {
    let status = if value.is_expired(Utc::now()) && value.status.as_str() == "active" {
        "expired".to_string()
    } else {
        value.status.as_str().to_string()
    };

    UserApiTokenResponse {
        id: value.id.to_string(),
        name: value.name,
        token_prefix: value.token_prefix,
        status,
        scopes: value.scopes.iter().map(map_scope).collect(),
        created_at: value.created_at.to_rfc3339(),
        updated_at: value.updated_at.to_rfc3339(),
        expires_at: value.expires_at.map(|v| v.to_rfc3339()),
        last_used_at: value.last_used_at.map(|v| v.to_rfc3339()),
        revoked_at: value.revoked_at.map(|v| v.to_rfc3339()),
    }
}

fn map_scope(value: &UserApiTokenScope) -> UserApiTokenScopeResponse {
    if value.resource_type == ResourceType::Cctv && value.action.is_cctv_permission() {
        return UserApiTokenScopeResponse {
            action: "cctv".to_string(),
            resource_type: action_key(&value.action).to_string(),
            resource_pattern: value.resource_pattern.clone(),
        };
    }
    UserApiTokenScopeResponse {
        action: action_key(&value.action).to_string(),
        resource_type: value.resource_type.as_str().to_string(),
        resource_pattern: value.resource_pattern.clone(),
    }
}

/// Sync the issuer's OIDC group memberships and invalidate its policy cache
/// before minting a personal API token. A group-sync failure is fatal; a
/// cache-invalidation failure is logged and tolerated.
async fn sync_oidc_group_memberships_for_token_issuer(
    state: &AppState,
    user_id: Uuid,
    request_context: &RequestContext,
) -> Result<(), AppError> {
    if request_context.auth_method != "oidc" || request_context.oidc_groups.is_empty() {
        return Ok(());
    }
    state
        .repositories
        .users
        .sync_oidc_group_memberships(user_id, &request_context.oidc_groups)
        .await?;
    if let Err(error) = state
        .services
        .access_control_admin
        .invalidate_policy_cache_for_user(user_id)
        .await
    {
        warn!(
            %user_id,
            error = %error,
            "failed to invalidate user policy cache after API-token OIDC group sync"
        );
    }
    Ok(())
}

/// Defense-in-depth: refuse to mint a personal API token whose `(action,
/// resource_type)` pairs the issuer's own RBAC does not grant. Use-time
/// narrowing (`api_token_scope_allows`) already ANDs the token scope with the
/// live policy decision, so this is hardening rather than the primary control,
/// but it prevents an issuer from minting a token broader than themselves.
///
/// The check is intentionally type-level only. `effective_authorization`
/// reports `(action, resource_type)` capabilities with no resource ids, so a
/// scope's `resource_pattern` is never evaluated here — a type-level allow plus
/// a resource pattern is valid, and use-time narrowing enforces per-id authority.
async fn ensure_issuer_grants_requested_scopes(
    state: &AppState,
    user_id: Uuid,
    scopes: &[UserApiTokenScope],
) -> Result<(), AppError> {
    let authorization = state
        .services
        .access_control
        .effective_authorization(&Principal::user(user_id))
        .await?;

    for scope in scopes {
        let granted = authorization.capabilities.iter().any(|capability| {
            capability.action == scope.action && capability.resource_type == scope.resource_type
        });
        if !granted {
            return Err(AppError::Forbidden(format!(
                "Requested token scope {} is not granted by your own permissions",
                scope.wire_value()
            )));
        }
    }

    Ok(())
}

fn parse_scope(value: &UserApiTokenScopeRequest) -> Result<UserApiTokenScope, AppError> {
    let (action, resource_type) = if value.action.eq_ignore_ascii_case("cctv") {
        (
            parse_cctv_action(value.resource_type.as_str())?,
            ResourceType::Cctv,
        )
    } else {
        (
            parse_action(value.action.as_str())?,
            parse_resource_type(value.resource_type.as_str())?,
        )
    };
    Ok(UserApiTokenScope {
        action,
        resource_type,
        resource_pattern: normalize_resource_pattern(value.resource_pattern.as_deref()),
    })
}

fn normalize_resource_pattern(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .unwrap_or("*")
        .to_string()
}

fn normalize_search(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn parse_scope_filter_list(raw: Option<&str>) -> Result<Vec<String>, AppError> {
    let mut scopes = Vec::new();
    for value in parse_csv_strings(raw) {
        let (left, right) = value.split_once(':').ok_or_else(|| {
            AppError::Validation(format!("Invalid API token scope filter {value:?}"))
        })?;
        let (action, resource_type) = if left.eq_ignore_ascii_case("cctv") {
            (parse_cctv_action(right)?, ResourceType::Cctv)
        } else {
            (parse_action(left)?, parse_resource_type(right)?)
        };
        let normalized = scope_wire_value(&action, &resource_type);
        if !scopes.contains(&normalized) {
            scopes.push(normalized);
        }
    }
    Ok(scopes)
}

fn parse_cctv_action(value: &str) -> Result<Action, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "view" => Ok(Action::CctvView),
        "recordings" => Ok(Action::CctvRecordings),
        "export" => Ok(Action::CctvExport),
        "settings" => Ok(Action::CctvSettings),
        "ptz" => Ok(Action::CctvPtz),
        _ => Err(AppError::Validation(format!(
            "Invalid CCTV token scope: cctv:{value}"
        ))),
    }
}

fn parse_action(value: &str) -> Result<Action, AppError> {
    let normalized = value.trim().to_uppercase();
    serde_json::from_str::<Action>(&format!("\"{}\"", normalized))
        .map_err(|_| AppError::Validation(format!("Invalid token scope action: {value}")))
}

fn parse_resource_type(value: &str) -> Result<ResourceType, AppError> {
    let normalized = value.trim();
    let canonical = normalized.to_lowercase().replace('_', "-");
    if let Some(resource_type) = ResourceType::parse(canonical.as_str()) {
        return Ok(resource_type);
    }

    let enum_variant = normalized.to_uppercase().replace('-', "_");
    serde_json::from_str::<ResourceType>(&format!("\"{}\"", enum_variant))
        .map_err(|_| AppError::Validation(format!("Invalid token scope resource type: {value}")))
}

fn scope_wire_value(action: &Action, resource_type: &ResourceType) -> String {
    crate::contexts::access_control::domain::pat_scope_wire_value(action, resource_type)
}

fn action_key(action: &Action) -> &'static str {
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
