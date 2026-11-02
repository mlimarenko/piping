use axum::{
    Extension, Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{
    Action, OIDC_CLIENT_SECRET_CONSUMER_TYPE, OIDC_CLIENT_SECRET_FIELD_KEY,
    OIDC_CLIENT_SECRET_OWNER_SCOPE_TYPE, OidcClientSecretState, OidcProvisioningMode, OidcSettings,
    ResourceType, oidc_client_secret_consumer_id, oidc_client_secret_owner_scope_id,
};
use crate::contexts::secrets::application::{ApplySecretFieldIntentInput, SecretFieldIntent};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OidcSettingsInput {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret_enabled: bool,
    pub client_secret_value: Option<String>,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub group_claim: String,
    pub provisioning_mode: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OidcSecretStateResponse {
    pub enabled: bool,
    pub has_value: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OidcSettingsResponse {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret_state: OidcSecretStateResponse,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub group_claim: String,
    pub provisioning_mode: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OidcTestConnectionResponse {
    pub ok: bool,
    pub message: String,
}

/// Get the OIDC identity-provider settings.
#[utoipa::path(
    get,
    path = "/api/v1/admin/auth/oidc/settings",
    tag = "oidc",
    responses(
        (status = 200, description = "OIDC settings", body = OidcSettingsResponse),
    ),
)]
pub async fn get_oidc_settings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, Some("settings")).await
    {
        return handle_app_error(error);
    }

    let service = state.services.oidc_settings.clone();
    let secret_state = match service.get_client_secret_state(&state.settings.auth).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let payload = match service.get_settings().await {
        Ok(Some(settings)) => OidcSettingsResponse::from_settings(&settings, secret_state),
        Ok(None) => OidcSettingsResponse {
            enabled: state.settings.auth.oidc_enabled,
            issuer_url: state.settings.auth.oidc_issuer_url.clone(),
            client_id: state.settings.auth.oidc_client_id.clone(),
            client_secret_state: map_secret_state(secret_state),
            redirect_url: state.settings.auth.oidc_redirect_url.clone(),
            scopes: state.settings.auth.oidc_scopes.clone(),
            group_claim: state.settings.auth.oidc_group_claim.clone(),
            provisioning_mode: OidcProvisioningMode::Manual.as_str().to_string(),
        },
        Err(error) => return handle_app_error(error),
    };

    Json(payload).into_response()
}

/// Update the OIDC identity-provider settings.
#[utoipa::path(
    put,
    path = "/api/v1/admin/auth/oidc/settings",
    tag = "oidc",
    request_body = OidcSettingsInput,
    responses(
        (status = 200, description = "OIDC settings updated", body = OidcSettingsResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_oidc_settings(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<OidcSettingsInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Update, Some("settings")).await
    {
        return handle_app_error(error);
    }

    let provisioning_mode = match OidcProvisioningMode::parse(&payload.provisioning_mode) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if payload.enabled && !payload.client_secret_enabled {
        return handle_app_error(AppError::Validation(
            "OIDC client secret must be enabled while OIDC login is enabled".to_string(),
        ));
    }

    let service = state.services.oidc_settings.clone();
    let current_secret_state = match service.get_client_secret_state(&state.settings.auth).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let has_secret_engine_binding = match has_oidc_client_secret_binding(&state).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let incoming_secret = normalize_optional(payload.client_secret_value.as_deref());
    if let Err(error) = apply_incoming_client_secret(
        &state,
        payload.client_secret_enabled,
        incoming_secret,
        &current_secret_state,
        has_secret_engine_binding,
    )
    .await
    {
        return handle_app_error(error);
    }

    let settings = OidcSettings {
        enabled: payload.enabled,
        issuer_url: payload.issuer_url.trim().to_string(),
        client_id: payload.client_id.trim().to_string(),
        redirect_url: payload.redirect_url.trim().to_string(),
        scopes: payload
            .scopes
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        group_claim: payload.group_claim.trim().to_string(),
        provisioning_mode,
    };

    let saved = match service.upsert_settings(settings).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let secret_state = match service.get_client_secret_state(&state.settings.auth).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    Json(OidcSettingsResponse::from_settings(&saved, secret_state)).into_response()
}

/// Test connectivity to the configured OIDC provider.
#[utoipa::path(
    post,
    path = "/api/v1/admin/auth/oidc/test-connection",
    tag = "oidc",
    responses(
        (status = 200, description = "OIDC test connection result", body = OidcTestConnectionResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn test_oidc_connection(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        Some("test-connection"),
    )
    .await
    {
        return handle_app_error(error);
    }

    let service = state.services.oidc_settings.clone();
    let effective = match service.get_effective(&state.settings.auth).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if !effective.auth.oidc_enabled {
        return handle_app_error(AppError::Validation(
            "OIDC is disabled. Enable and save settings before test.".to_string(),
        ));
    }

    match service
        .test_connection(&effective.auth, &effective.client_secret)
        .await
    {
        Ok(()) => Json(OidcTestConnectionResponse {
            ok: true,
            message: "OIDC discovery succeeded".to_string(),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Apply the incoming client-secret state: rotate/set a provided value, require
/// one when the secret is enabled and none is stored, or clear the binding.
async fn apply_incoming_client_secret(
    state: &AppState,
    enabled: bool,
    incoming_secret: Option<String>,
    current_secret_state: &OidcClientSecretState,
    has_secret_engine_binding: bool,
) -> Result<(), AppError> {
    if !enabled {
        if has_secret_engine_binding {
            apply_oidc_client_secret_intent(state, SecretFieldIntent::Clear, None).await?;
        }
        return Ok(());
    }

    let Some(value) = incoming_secret else {
        if current_secret_state.has_value {
            return Ok(());
        }
        return Err(AppError::Validation(
            "OIDC client secret value is required".to_string(),
        ));
    };

    let intent = if has_secret_engine_binding {
        SecretFieldIntent::Rotate
    } else {
        SecretFieldIntent::Set
    };
    apply_oidc_client_secret_intent(state, intent, Some(value)).await?;
    Ok(())
}

impl OidcSettingsResponse {
    fn from_settings(settings: &OidcSettings, secret_state: OidcClientSecretState) -> Self {
        Self {
            enabled: settings.enabled,
            issuer_url: settings.issuer_url.clone(),
            client_id: settings.client_id.clone(),
            client_secret_state: map_secret_state(secret_state),
            redirect_url: settings.redirect_url.clone(),
            scopes: settings.scopes.clone(),
            group_claim: settings.group_claim.clone(),
            provisioning_mode: settings.provisioning_mode.as_str().to_string(),
        }
    }
}

fn map_secret_state(value: OidcClientSecretState) -> OidcSecretStateResponse {
    OidcSecretStateResponse {
        enabled: value.enabled,
        has_value: value.has_value,
    }
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    let trimmed = value.unwrap_or_default().trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

async fn has_oidc_client_secret_binding(state: &AppState) -> Result<bool, AppError> {
    let binding = state
        .repositories
        .secrets
        .get_usage_binding(
            OIDC_CLIENT_SECRET_CONSUMER_TYPE,
            oidc_client_secret_consumer_id()?,
            OIDC_CLIENT_SECRET_FIELD_KEY,
        )
        .await?;
    Ok(binding.is_some())
}

async fn apply_oidc_client_secret_intent(
    state: &AppState,
    intent: SecretFieldIntent,
    value: Option<String>,
) -> Result<(), AppError> {
    let input = ApplySecretFieldIntentInput {
        consumer_type: OIDC_CLIENT_SECRET_CONSUMER_TYPE.to_string(),
        consumer_id: oidc_client_secret_consumer_id()?,
        owner_scope_type: OIDC_CLIENT_SECRET_OWNER_SCOPE_TYPE.to_string(),
        owner_scope_id: oidc_client_secret_owner_scope_id()?,
        field_key: OIDC_CLIENT_SECRET_FIELD_KEY.to_string(),
        intent,
        value,
        secret_id: None,
    };

    let _ = state
        .services
        .secret_field_intents
        .apply_intent(input)
        .await?;
    Ok(())
}

async fn ensure_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    crate::api::permission_helpers::ensure_access_with_resource(
        state,
        request_context,
        action,
        ResourceType::Keycloak,
        resource_id,
    )
    .await
}
