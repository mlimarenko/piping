//! OpenID Connect integration: provider discovery, PKCE login state, code exchange, claim extraction and session cache keys.
use std::collections::{HashMap, HashSet};

use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreProviderMetadata,
};
use openidconnect::reqwest;
use openidconnect::{
    AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl, UserInfoClaims, UserInfoUrl,
};
use serde::{Deserialize, Serialize};

use crate::config::AuthConfig;
use crate::config::constants::DEFAULT_HTTP_TIMEOUT;
use crate::errors::AppError;

pub const OIDC_STATE_COOKIE_NAME: &str = "piping_oidc_state";
pub const OIDC_STATE_TTL_SECONDS: u64 = 300;

#[derive(Debug, Clone)]
pub struct OidcClient {
    pub client: OidcCoreClient,
    pub http_client: reqwest::Client,
    pub supported_scopes: Option<HashSet<String>>,
}

#[derive(Debug, Clone)]
pub struct OidcLoginState {
    pub auth_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcStateData {
    pub nonce: String,
    pub pkce_verifier: String,
    pub redirect_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcSessionMetadata {
    pub auth_method: String,
    pub oidc_groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OidcAdditionalClaims {
    #[serde(flatten)]
    pub claims: HashMap<String, serde_json::Value>,
}

impl AdditionalClaims for OidcAdditionalClaims {}

#[derive(Debug, Clone)]
pub struct OidcClaims {
    pub subject: String,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    pub groups: Vec<String>,
}

struct AuthoritativeUserInfo {
    preferred_username: Option<String>,
    email: Option<String>,
    groups: Vec<String>,
}

/// OIDC Core Client type alias with configured endpoints.
/// Type parameters represent typestates for each endpoint (compile-time safety):
/// 1. Authorization endpoint (EndpointSet - used for authorize_url)
/// 2. Device authorization endpoint (EndpointNotSet - not used)
/// 3. Introspection endpoint (EndpointNotSet - not used)
/// 4. Revocation endpoint (EndpointNotSet - not used)
/// 5. Token endpoint (EndpointSet - used for token exchange)
/// 6. UserInfo endpoint (EndpointSet - used for user info retrieval)
pub type OidcCoreClient = CoreClient<
    EndpointSet,    // Authorization endpoint
    EndpointNotSet, // Device authorization endpoint
    EndpointNotSet, // Introspection endpoint
    EndpointNotSet, // Revocation endpoint
    EndpointSet,    // Token endpoint
    EndpointSet,    // UserInfo endpoint
>;

/// Builds an HTTP client for OIDC provider communication.
///
/// The client is configured with:
/// - No redirects (prevents redirect loops and security issues)
/// - Default timeout from configuration
///
/// This client is used for OIDC discovery and token/userinfo requests.
fn build_http_client() -> Result<reqwest::Client, AppError> {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DEFAULT_HTTP_TIMEOUT)
        .build()
        .map_err(|error| AppError::External(format!("Failed to build OIDC HTTP client: {}", error)))
}

/// Discovers OIDC provider configuration and creates a configured client.
///
/// Process:
/// 1. Validates issuer URL and redirect URL from configuration
/// 2. Builds HTTP client for provider communication
/// 3. Performs OIDC discovery (fetches `.well-known/openid-configuration`)
/// 4. Extracts token and userinfo endpoints from provider metadata
/// 5. Creates CoreClient with discovered endpoints configured
///
/// The client is ready to use for authorization flows and token exchange.
pub async fn discover_client(
    config: &AuthConfig,
    client_secret: &str,
) -> Result<OidcClient, AppError> {
    let issuer_url = IssuerUrl::new(config.oidc_issuer_url.clone())
        .map_err(|error| AppError::Validation(format!("Invalid OIDC issuer URL: {}", error)))?;
    let redirect_url = RedirectUrl::new(config.oidc_redirect_url.clone())
        .map_err(|error| AppError::Validation(format!("Invalid OIDC redirect URL: {}", error)))?;
    let http_client = build_http_client()?;
    let provider_metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client)
        .await
        .map_err(|error| AppError::External(format!("OIDC discovery failed: {}", error)))?;
    let supported_scopes = provider_metadata.scopes_supported().map(|scopes| {
        scopes
            .iter()
            .map(|scope| scope.as_ref().to_ascii_lowercase())
            .collect::<HashSet<_>>()
    });
    let token_url = provider_metadata
        .token_endpoint()
        .cloned()
        .ok_or_else(|| AppError::InvalidConfig("OIDC token endpoint missing".to_string()))?;
    let userinfo_url = provider_metadata
        .userinfo_endpoint()
        .cloned()
        .ok_or_else(|| AppError::InvalidConfig("OIDC userinfo endpoint missing".to_string()))?;

    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(config.oidc_client_id.clone()),
        Some(ClientSecret::new(client_secret.to_string())),
    )
    .set_redirect_uri(redirect_url)
    .set_token_uri(
        TokenUrl::new(token_url.as_str().to_string())
            .map_err(|error| AppError::Validation(format!("Invalid OIDC token URL: {}", error)))?,
    )
    .set_user_info_url(
        UserInfoUrl::new(userinfo_url.as_str().to_string()).map_err(|error| {
            AppError::Validation(format!("Invalid OIDC userinfo URL: {}", error))
        })?,
    );

    Ok(OidcClient {
        client,
        http_client,
        supported_scopes,
    })
}

/// Builds OIDC login state for initiating an authorization flow.
///
/// Creates:
/// - Authorization URL with PKCE challenge (for secure token exchange)
/// - CSRF token (state) for request validation
/// - Nonce for ID token validation
/// - PKCE verifier (stored securely, used during token exchange)
///
/// The state and verifier must be stored (e.g., in Redis) and retrieved
/// during the callback to complete authentication.
pub async fn build_login_state(
    config: &AuthConfig,
    client_secret: &str,
) -> Result<OidcLoginState, AppError> {
    let oidc_client = discover_client(config, client_secret).await?;

    // Generate PKCE challenge/verifier pair for secure token exchange
    // PKCE prevents authorization code interception attacks
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Build authorization URL with random CSRF token and nonce
    let mut request = oidc_client.client.authorize_url(
        CoreAuthenticationFlow::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );

    // Add configured scopes, excluding "openid" (added by the library) and duplicates.
    let mut seen_scopes = HashSet::new();
    for scope in &config.oidc_scopes {
        let normalized = scope.trim().to_ascii_lowercase();
        if normalized.is_empty() || normalized == "openid" {
            continue;
        }
        if let Some(supported_scopes) = oidc_client.supported_scopes.as_ref()
            && !supported_scopes.contains(&normalized)
        {
            continue;
        }
        if !seen_scopes.insert(normalized.clone()) {
            continue;
        }
        request = request.add_scope(Scope::new(normalized));
    }

    // Generate final authorization URL with PKCE challenge
    let (auth_url, csrf_token, nonce) = request.set_pkce_challenge(pkce_challenge).url();

    Ok(OidcLoginState {
        auth_url: auth_url.to_string(),
        state: csrf_token.secret().to_string(),
        nonce: nonce.secret().to_string(),
        pkce_verifier: pkce_verifier.secret().to_string(),
    })
}

/// Exchanges OIDC authorization code for user claims.
///
/// Process:
/// 1. Exchanges authorization code for tokens using PKCE verifier
/// 2. Verifies ID token signature and nonce
/// 3. Extracts subject and email from ID token claims
/// 4. Fetches the authoritative userinfo claims (including groups)
/// 5. Returns consolidated user claims
///
/// The PKCE verifier and nonce must match what was generated during `build_login_state`.
/// This completes the OIDC authorization code flow.
pub async fn exchange_code_for_claims(
    config: &AuthConfig,
    client_secret: &str,
    code: &str,
    pkce_verifier: &str,
    nonce: &str,
) -> Result<OidcClaims, AppError> {
    let oidc_client = discover_client(config, client_secret).await?;

    // Exchange authorization code for tokens
    // PKCE verifier proves we own the original authorization request
    let token_response = oidc_client
        .client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
        .request_async(&oidc_client.http_client)
        .await
        .map_err(|error| AppError::External(format!("OIDC token exchange failed: {}", error)))?;

    // Verify ID token: checks signature, expiration, and nonce
    let id_token = token_response
        .id_token()
        .ok_or_else(|| AppError::Auth("OIDC provider did not return an ID token".to_string()))?;
    let nonce = Nonce::new(nonce.to_string());
    let verifier = oidc_client.client.id_token_verifier();
    let claims = match id_token.claims(&verifier, &nonce) {
        Ok(claims) => claims,
        Err(strict_error) => {
            let relaxed_verifier = verifier.clone().require_issuer_match(false);
            let relaxed_claims = id_token.claims(&relaxed_verifier, &nonce).map_err(|_| {
                AppError::Auth(format!("Failed to verify ID token: {}", strict_error))
            })?;
            validate_relaxed_issuer(config, relaxed_claims.issuer()).map_err(|issuer_error| {
                AppError::Auth(format!(
                    "Failed to verify ID token: {} ({})",
                    strict_error, issuer_error
                ))
            })?;
            relaxed_claims
        }
    };

    // Extract basic claims from ID token
    let subject = claims.subject().as_str().to_string();
    let email = claims
        .email()
        .map(|value: &openidconnect::EndUserEmail| value.as_str().to_string());
    let userinfo: Result<UserInfoClaims<OidcAdditionalClaims, CoreGenderClaim>, AppError> =
        oidc_client
            .client
            .user_info(token_response.access_token().to_owned(), None)
            .request_async(&oidc_client.http_client)
            .await
            .map_err(|error| AppError::External(format!("OIDC user info failed: {}", error)));

    // Userinfo is the sole authority for the configured group claim. A present
    // empty array is authoritative, while absence or an invalid value rejects
    // the login before persisted authorization can be synchronized.
    let userinfo = extract_authoritative_userinfo(email, userinfo, &config.oidc_group_claim)?;

    Ok(OidcClaims {
        subject,
        preferred_username: userinfo.preferred_username,
        email: userinfo.email,
        groups: userinfo.groups,
    })
}

/// Generates Redis cache key for OIDC state data.
///
/// Used to store PKCE verifier and nonce temporarily during OIDC flow.
/// State expires after OIDC_STATE_TTL_SECONDS.
pub fn state_cache_key(state: &str) -> String {
    format!("oidc:state:{}", state)
}

/// Generates Redis cache key for OIDC session metadata.
///
/// Stores OIDC-specific session data (auth method, groups) alongside database session.
pub fn session_cache_key(session_id: &uuid::Uuid) -> String {
    format!("oidc:session:{}", session_id)
}

fn extract_authoritative_userinfo(
    id_token_email: Option<String>,
    userinfo: Result<UserInfoClaims<OidcAdditionalClaims, CoreGenderClaim>, AppError>,
    group_claim: &str,
) -> Result<AuthoritativeUserInfo, AppError> {
    let userinfo = userinfo?;
    let email = id_token_email.or_else(|| {
        userinfo
            .email()
            .map(|value: &openidconnect::EndUserEmail| value.as_str().to_string())
    });
    let preferred_username =
        extract_string_claim(userinfo.additional_claims(), "preferred_username")
            .or_else(|| extract_string_claim(userinfo.additional_claims(), "username"))
            .or_else(|| extract_string_claim(userinfo.additional_claims(), "name"));
    let groups = extract_authoritative_groups(userinfo.additional_claims(), group_claim)?;

    Ok(AuthoritativeUserInfo {
        preferred_username,
        email,
        groups,
    })
}

/// Extracts group names from OIDC additional claims.
///
/// Supports two claim formats:
/// - Array of strings: `["group1", "group2"]`
/// - Single string: `"group1"`
///
/// A present empty array is authoritative. Missing, empty-string, mixed-type,
/// and otherwise malformed claims are rejected so they cannot be mistaken for
/// an IdP decision to remove every membership.
fn extract_authoritative_groups(
    additional: &OidcAdditionalClaims,
    claim_key: &str,
) -> Result<Vec<String>, AppError> {
    let value = additional.claims.get(claim_key).ok_or_else(|| {
        AppError::Auth(format!(
            "OIDC user info did not include configured group claim '{}'",
            claim_key
        ))
    })?;

    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| {
                let group = value
                    .as_str()
                    .map(str::trim)
                    .filter(|group| !group.is_empty());
                group.map(str::to_string).ok_or_else(|| {
                    AppError::Auth(format!(
                        "OIDC user info group claim '{}' must contain only non-empty strings",
                        claim_key
                    ))
                })
            })
            .collect(),
        serde_json::Value::String(value) if !value.trim().is_empty() => {
            Ok(vec![value.trim().to_string()])
        }
        _ => Err(AppError::Auth(format!(
            "OIDC user info group claim '{}' must be a string or an array of strings",
            claim_key
        ))),
    }
}

fn extract_string_claim(additional: &OidcAdditionalClaims, claim_key: &str) -> Option<String> {
    match additional.claims.get(claim_key) {
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

fn validate_relaxed_issuer(config: &AuthConfig, actual_issuer: &IssuerUrl) -> Result<(), String> {
    let configured = IssuerUrl::new(config.oidc_issuer_url.clone())
        .map_err(|error| format!("invalid configured issuer URL: {}", error))?;
    if configured.as_str() == actual_issuer.as_str() {
        return Ok(());
    }

    let configured_path = normalize_path(configured.url().path());
    let actual_path = normalize_path(actual_issuer.url().path());
    if configured_path != actual_path {
        return Err(format!(
            "issuer path mismatch: expected '{}', got '{}'",
            configured_path, actual_path
        ));
    }

    let redirect = RedirectUrl::new(config.oidc_redirect_url.clone())
        .map_err(|error| format!("invalid redirect URL: {}", error))?;
    let actual_host = actual_issuer.url().host_str().unwrap_or_default();
    let configured_host = configured.url().host_str().unwrap_or_default();
    let redirect_host = redirect.url().host_str().unwrap_or_default();

    if actual_host == configured_host || actual_host == redirect_host {
        return Ok(());
    }

    Err(format!(
        "issuer host '{}' is not allowed (expected '{}' or '{}')",
        actual_host, configured_host, redirect_host
    ))
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn userinfo_with_group_claim(
        group_claim: Option<serde_json::Value>,
    ) -> UserInfoClaims<OidcAdditionalClaims, CoreGenderClaim> {
        let mut claims = HashMap::new();
        if let Some(group_claim) = group_claim {
            claims.insert("groups".to_string(), group_claim);
        }
        UserInfoClaims::new(
            openidconnect::StandardClaims::new(openidconnect::SubjectIdentifier::new(
                "subject-1".to_string(),
            )),
            OidcAdditionalClaims { claims },
        )
    }

    #[test]
    fn present_empty_group_array_is_authoritative() -> Result<(), AppError> {
        let userinfo = userinfo_with_group_claim(Some(json!([])));

        let extracted = extract_authoritative_userinfo(None, Ok(userinfo), "groups")?;

        assert!(extracted.groups.is_empty());
        Ok(())
    }

    #[test]
    fn userinfo_failure_is_not_an_authoritative_empty_group_set() {
        let result = extract_authoritative_userinfo(
            None,
            Err(AppError::External("userinfo unavailable".to_string())),
            "groups",
        );

        assert!(matches!(
            result,
            Err(AppError::External(message)) if message == "userinfo unavailable"
        ));
    }

    #[test]
    fn missing_group_claim_is_not_authoritative() {
        let userinfo = userinfo_with_group_claim(None);

        let result = extract_authoritative_userinfo(None, Ok(userinfo), "groups");

        assert!(matches!(
            result,
            Err(AppError::Auth(message))
                if message.contains("did not include configured group claim")
        ));
    }

    #[test]
    fn wrong_group_claim_types_are_not_authoritative() {
        for invalid_claim in [json!(null), json!(false), json!(42), json!({"name": "ops"})] {
            let userinfo = userinfo_with_group_claim(Some(invalid_claim));
            let result = extract_authoritative_userinfo(None, Ok(userinfo), "groups");

            assert!(
                matches!(result, Err(AppError::Auth(_))),
                "invalid group claim was accepted"
            );
        }
    }

    #[test]
    fn partially_malformed_group_arrays_are_rejected_atomically() {
        for invalid_claim in [json!(["ops", 42]), json!(["ops", " "])] {
            let userinfo = userinfo_with_group_claim(Some(invalid_claim));
            let result = extract_authoritative_userinfo(None, Ok(userinfo), "groups");

            assert!(
                matches!(result, Err(AppError::Auth(_))),
                "partially malformed group claim was accepted"
            );
        }
    }
}
