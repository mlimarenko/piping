//! Guard tests for the OIDC login perimeter (`src/integrations/oidc`).
//!
//! Every property here is one where a regression means "an attacker can log in
//! as somebody else", not "a feature stopped working": ID-token signature,
//! nonce binding, expiry, audience, issuer, and the per-request freshness of
//! state/nonce/PKCE.
//!
//! The tests drive the real `build_login_state` / `exchange_code_for_claims`
//! against an in-process fake IdP. The fake IdP is deliberately *permissive* —
//! it signs whatever it is told to sign — so a rejection observed by a test can
//! only come from the product's own verification, never from the mock refusing
//! to play along. Each negative test is the happy path with exactly one
//! mutation, and `happy_path_exchange_returns_claims` is the positive control:
//! if the harness rots, that test goes red first and the negatives are known to
//! be meaningless.

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Router, http::StatusCode};
use base64::Engine;
use chrono::{TimeDelta, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJwsSigningAlgorithm, CoreRsaPrivateSigningKey,
};
use openidconnect::{
    AccessToken, Audience, EmptyAdditionalClaims, EndUserEmail, IssuerUrl, JsonWebKeyId, Nonce,
    PrivateSigningKey, StandardClaims, SubjectIdentifier,
};
use piping_backend::config::AuthConfig;
use piping_backend::errors::AppError;
use piping_backend::integrations::oidc;
use serde_json::json;
use sha2::{Digest, Sha256};

mod common;

#[path = "oidc_login_guards/claim_failures.rs"]
mod claim_failures;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type BoxError = Box<dyn Error + Send + Sync>;

const CLIENT_ID: &str = "piping-test-client";
const CLIENT_SECRET: &str = "test-client-secret";
const SIGNING_KID: &str = "test-signing-key";
const SUBJECT: &str = "user-subject-42";
const EMAIL: &str = "oidc-user@example.test";

// ---------------------------------------------------------------------------
// Fake IdP
// ---------------------------------------------------------------------------

/// What the fake token endpoint should mint on the next exchange.
///
/// `TokenPlan::valid()` is the baseline every test starts from; a negative test
/// overrides exactly one field so the observed rejection is attributable.
#[derive(Clone)]
struct TokenPlan {
    /// Nonce embedded in the ID token. The caller of `exchange_code_for_claims`
    /// must present the same value for verification to succeed.
    nonce: String,
    /// `iss` claim of the ID token. `None` means "use the discovery issuer".
    id_token_issuer: Option<String>,
    /// `aud` claim of the ID token. `None` means "use the registered client id".
    audience: Option<String>,
    /// Offset of `exp` from now. Negative values mint an already-expired token.
    lifetime: TimeDelta,
    /// Sign with a key that is *not* published in the JWKS, while keeping the
    /// same `kid` — so the client finds a candidate key and must fall over on
    /// the signature itself rather than on "no matching key".
    sign_with_foreign_key: bool,
    /// Return a token response with no `id_token` field at all.
    omit_id_token: bool,
    /// When set, the token endpoint rejects a request whose `code_verifier`
    /// does not match this value (i.e. a normally-behaving IdP).
    require_pkce_verifier: Option<String>,
    subject: String,
    userinfo: UserinfoPlan,
}

#[derive(Clone)]
enum UserinfoPlan {
    Groups(Vec<String>),
    MissingGroupClaim,
    ServerError,
}

impl TokenPlan {
    fn valid(nonce: &str) -> Self {
        Self {
            nonce: nonce.to_string(),
            id_token_issuer: None,
            audience: None,
            lifetime: TimeDelta::minutes(5),
            sign_with_foreign_key: false,
            omit_id_token: false,
            require_pkce_verifier: None,
            subject: SUBJECT.to_string(),
            userinfo: UserinfoPlan::Groups(vec!["piping-admins".to_string()]),
        }
    }
}

struct IdpState {
    issuer: String,
    /// Key published in the JWKS and used for signing on the happy path.
    published_key: CoreRsaPrivateSigningKey,
    /// Same `kid`, different key material, never published.
    foreign_key: CoreRsaPrivateSigningKey,
    plan: Mutex<TokenPlan>,
    /// Form fields of the most recent `/token` request, for asserting on what
    /// the client actually transmitted (PKCE verifier, grant type, code).
    last_token_request: Mutex<Option<HashMap<String, String>>>,
}

struct FakeIdp {
    base_url: String,
    state: Arc<IdpState>,
}

impl FakeIdp {
    fn set_plan(&self, plan: TokenPlan) -> Result<(), BoxError> {
        let mut guard = self
            .state
            .plan
            .lock()
            .map_err(|error| format!("plan mutex poisoned: {error}"))?;
        *guard = plan;
        Ok(())
    }

    fn last_token_request(&self) -> Result<HashMap<String, String>, BoxError> {
        let guard = self
            .state
            .last_token_request
            .lock()
            .map_err(|error| format!("token-request mutex poisoned: {error}"))?;
        guard
            .clone()
            .ok_or_else(|| BoxError::from("token endpoint was never called"))
    }

    fn auth_config(&self) -> Result<AuthConfig, BoxError> {
        let config = serde_json::from_value::<AuthConfig>(json!({
            "oidc_enabled": true,
            "oidc_issuer_url": self.base_url.clone(),
            "oidc_client_id": CLIENT_ID,
            "oidc_redirect_url": format!("{}/callback", self.base_url),
            "oidc_scopes": ["openid", "profile", "email", "groups"],
            "oidc_group_claim": "groups",
        }))?;
        Ok(config)
    }
}

fn generate_signing_key() -> Result<CoreRsaPrivateSigningKey, BoxError> {
    let rsa = openssl::rsa::Rsa::generate(2048)?;
    let pem = String::from_utf8(rsa.private_key_to_pem()?)?;
    CoreRsaPrivateSigningKey::from_pem(&pem, Some(JsonWebKeyId::new(SIGNING_KID.to_string())))
        .map_err(BoxError::from)
}

async fn spawn_fake_idp() -> Result<FakeIdp, BoxError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let issuer = format!("http://{addr}");

    let state = Arc::new(IdpState {
        issuer: issuer.clone(),
        published_key: generate_signing_key()?,
        foreign_key: generate_signing_key()?,
        plan: Mutex::new(TokenPlan::valid("unset-nonce")),
        last_token_request: Mutex::new(None),
    });

    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery_document))
        .route("/jwks", get(jwks_document))
        .route("/token", post(token_endpoint))
        .route("/userinfo", get(userinfo_endpoint))
        .with_state(Arc::clone(&state));

    tokio::spawn(async move {
        #[allow(
            clippy::expect_used,
            reason = "detached background task backing a test-only mock IdP; a serve failure has no caller left to propagate to, so panicking the task is the correct failure mode"
        )]
        axum::serve(listener, app).await.expect("serve fake idp");
    });

    Ok(FakeIdp {
        base_url: issuer,
        state,
    })
}

async fn discovery_document(State(state): State<Arc<IdpState>>) -> impl IntoResponse {
    let issuer = &state.issuer;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "userinfo_endpoint": format!("{issuer}/userinfo"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid", "profile", "email", "groups"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "claims_supported": ["sub", "email", "preferred_username", "groups"],
    }))
}

async fn jwks_document(State(state): State<Arc<IdpState>>) -> impl IntoResponse {
    Json(json!({ "keys": [state.published_key.as_verification_key()] }))
}

async fn userinfo_endpoint(State(state): State<Arc<IdpState>>) -> impl IntoResponse {
    let plan = {
        #[allow(
            clippy::expect_used,
            reason = "test-only mock handler with no caller to propagate to; a poisoned mutex means the test harness is already broken and must fail loudly"
        )]
        state.plan.lock().expect("plan mutex poisoned").clone()
    };
    match plan.userinfo {
        UserinfoPlan::Groups(groups) => Json(json!({
            "sub": plan.subject,
            "email": EMAIL,
            "preferred_username": "oidc-user",
            "groups": groups,
        }))
        .into_response(),
        UserinfoPlan::MissingGroupClaim => Json(json!({
            "sub": plan.subject,
            "email": EMAIL,
            "preferred_username": "oidc-user",
        }))
        .into_response(),
        UserinfoPlan::ServerError => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "userinfo unavailable" })),
        )
            .into_response(),
    }
}

async fn token_endpoint(State(state): State<Arc<IdpState>>, body: String) -> impl IntoResponse {
    let fields: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    let plan = {
        #[allow(
            clippy::expect_used,
            reason = "test-only mock handler with no caller to propagate to; a poisoned mutex means the test harness is already broken and must fail loudly"
        )]
        let mut recorded = state
            .last_token_request
            .lock()
            .expect("token-request mutex poisoned");
        *recorded = Some(fields.clone());

        #[allow(
            clippy::expect_used,
            reason = "test-only mock handler with no caller to propagate to; a poisoned mutex means the test harness is already broken and must fail loudly"
        )]
        let plan = state.plan.lock().expect("plan mutex poisoned");
        plan.clone()
    };

    if let Some(expected_verifier) = plan.require_pkce_verifier.as_ref()
        && fields.get("code_verifier") != Some(expected_verifier)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }

    let access_token = AccessToken::new("fake-access-token".to_string());
    if plan.omit_id_token {
        return Json(json!({
            "access_token": access_token.secret(),
            "token_type": "Bearer",
        }))
        .into_response();
    }

    match mint_id_token(&state, &plan, &access_token) {
        Ok(id_token) => Json(json!({
            "access_token": access_token.secret(),
            "token_type": "Bearer",
            "id_token": id_token,
        }))
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("mock could not mint id_token: {error}") })),
        )
            .into_response(),
    }
}

fn mint_id_token(
    state: &IdpState,
    plan: &TokenPlan,
    access_token: &AccessToken,
) -> Result<String, BoxError> {
    let issuer = plan
        .id_token_issuer
        .clone()
        .unwrap_or_else(|| state.issuer.clone());
    let audience = plan
        .audience
        .clone()
        .unwrap_or_else(|| CLIENT_ID.to_string());
    let now = Utc::now();

    let claims = CoreIdTokenClaims::new(
        IssuerUrl::new(issuer)?,
        vec![Audience::new(audience)],
        now + plan.lifetime,
        now,
        StandardClaims::new(SubjectIdentifier::new(plan.subject.clone()))
            .set_email(Some(EndUserEmail::new(EMAIL.to_string()))),
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(plan.nonce.clone())));

    let signing_key = if plan.sign_with_foreign_key {
        &state.foreign_key
    } else {
        &state.published_key
    };

    let id_token = CoreIdToken::new(
        claims,
        signing_key,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        Some(access_token),
        None,
    )?;
    Ok(id_token.to_string())
}

// ---------------------------------------------------------------------------
// Shared flow helpers
// ---------------------------------------------------------------------------

/// Runs the full product flow: authorization request, then code exchange, with
/// `plan` describing what the IdP mints and `nonce_presented` describing what
/// the callback claims the nonce was.
async fn exchange_with(
    idp: &FakeIdp,
    plan_for: impl FnOnce(&oidc::OidcLoginState) -> TokenPlan,
    nonce_for: impl FnOnce(&oidc::OidcLoginState) -> String,
) -> Result<Result<oidc::OidcClaims, AppError>, BoxError> {
    let config = idp.auth_config()?;
    let login = oidc::build_login_state(&config, CLIENT_SECRET).await?;
    idp.set_plan(plan_for(&login))?;
    let nonce = nonce_for(&login);
    Ok(oidc::exchange_code_for_claims(
        &config,
        CLIENT_SECRET,
        "authorization-code",
        &login.pkce_verifier,
        &nonce,
    )
    .await)
}

/// Requires an authentication rejection *for the stated reason*.
///
/// Matching on the reason matters: without it, a token rejected because the
/// harness failed to serve its JWKS would satisfy a test that claims to prove
/// signature verification.
fn require_auth_rejection(
    result: &Result<oidc::OidcClaims, AppError>,
    expected_substring: &str,
) -> Result<(), BoxError> {
    match result {
        Ok(claims) => Err(format!(
            "expected the exchange to be rejected, but it authenticated subject {:?}",
            claims.subject
        )
        .into()),
        Err(AppError::Auth(message)) => {
            let lowered = message.to_ascii_lowercase();
            if lowered.contains(expected_substring) {
                Ok(())
            } else {
                Err(format!(
                    "rejection happened, but not for the expected reason: wanted a message containing {expected_substring:?}, got {message:?}"
                )
                .into())
            }
        }
        Err(other) => Err(format!("expected AppError::Auth rejection, got {other:?}").into()),
    }
}

// ---------------------------------------------------------------------------
// Positive control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_exchange_returns_claims() -> TestResult {
    let idp = spawn_fake_idp().await?;

    let result = exchange_with(
        &idp,
        |login| TokenPlan::valid(&login.nonce),
        |login| login.nonce.clone(),
    )
    .await?;

    let claims = result.map_err(|error| format!("happy path must authenticate: {error:?}"))?;
    assert_eq!(claims.subject, SUBJECT);
    assert_eq!(claims.email.as_deref(), Some(EMAIL));
    // Groups are the authorization-bearing part of the payload, so the control
    // has to prove they survive the round trip and are not silently empty.
    assert_eq!(claims.groups, vec!["piping-admins".to_string()]);
    // `preferred_username` is deliberately not asserted: `openidconnect` parses
    // it into the *standard* claims, while the product only looks for it among
    // the *additional* claims, so a spec-compliant userinfo response never
    // populates it. That is a real defect, but a display-name one — pinning it
    // here would freeze the bug instead of guarding the login perimeter.
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 1 — per-request freshness of state / nonce / PKCE verifier
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_state_is_freshly_random_on_every_authorization_request() -> TestResult {
    let idp = spawn_fake_idp().await?;
    let config = idp.auth_config()?;

    let mut states = Vec::new();
    let mut nonces = Vec::new();
    let mut verifiers = Vec::new();
    for _ in 0..8 {
        let login = oidc::build_login_state(&config, CLIENT_SECRET).await?;
        // A CSRF token / nonce short enough to enumerate is as good as absent;
        // the library's default is 128 bits base64url-encoded (22 chars).
        assert!(
            login.state.len() >= 16,
            "state is too short to be unguessable: {:?}",
            login.state
        );
        assert!(
            login.nonce.len() >= 16,
            "nonce is too short to be unguessable: {:?}",
            login.nonce
        );
        // RFC 7636 requires a code verifier of at least 43 characters.
        assert!(
            login.pkce_verifier.len() >= 43,
            "PKCE verifier is shorter than RFC 7636 allows: {:?}",
            login.pkce_verifier
        );
        states.push(login.state);
        nonces.push(login.nonce);
        verifiers.push(login.pkce_verifier);
    }

    assert_unique(&states, "state");
    assert_unique(&nonces, "nonce");
    assert_unique(&verifiers, "pkce_verifier");
    Ok(())
}

fn assert_unique(values: &[String], label: &str) {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        values.len(),
        "{label} repeated across authorization requests: {values:?}"
    );
}

#[tokio::test]
async fn authorization_url_carries_the_generated_state_nonce_and_s256_challenge() -> TestResult {
    let idp = spawn_fake_idp().await?;
    let config = idp.auth_config()?;
    let login = oidc::build_login_state(&config, CLIENT_SECRET).await?;

    let query: HashMap<String, String> = url::Url::parse(&login.auth_url)?
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    assert_eq!(query.get("state"), Some(&login.state));
    assert_eq!(query.get("nonce"), Some(&login.nonce));
    assert_eq!(
        query.get("code_challenge_method").map(String::as_str),
        Some("S256"),
        "a plain (or absent) PKCE challenge method makes the verifier worthless"
    );
    // The challenge must actually be derived from the verifier we kept; if the
    // two are unrelated, PKCE protects nothing while still looking present.
    let expected_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(login.pkce_verifier.as_bytes()));
    assert_eq!(
        query.get("code_challenge"),
        Some(&expected_challenge),
        "code_challenge is not S256(pkce_verifier)"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 2 — nonce binding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exchange_rejects_id_token_whose_nonce_does_not_match() -> TestResult {
    let idp = spawn_fake_idp().await?;

    // The IdP mints a perfectly valid, correctly signed token — it just carries
    // a different nonce than the one the callback stored. This is the replay of
    // a token minted for another authorization request.
    let result = exchange_with(
        &idp,
        |_| TokenPlan::valid("nonce-from-a-different-login-attempt"),
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "nonce")?;
    Ok(())
}

#[tokio::test]
async fn exchange_rejects_id_token_with_no_nonce_at_all() -> TestResult {
    let idp = spawn_fake_idp().await?;

    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            nonce: String::new(),
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "nonce")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 3 — PKCE (client half; see the test docs for what this can prove)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exchange_transmits_the_pkce_verifier_it_was_given() -> TestResult {
    // Whether a *wrong* verifier is refused is the IdP's decision, not ours, so
    // a mock refusing it would only prove the mock works. What is genuinely the
    // client's responsibility — and the realistic regression, since the flow
    // keeps working against a lenient IdP once it breaks — is that the verifier
    // is transmitted at all, and that it is the one bound to this login.
    let idp = spawn_fake_idp().await?;
    let config = idp.auth_config()?;
    let login = oidc::build_login_state(&config, CLIENT_SECRET).await?;
    idp.set_plan(TokenPlan::valid(&login.nonce))?;

    let claims = oidc::exchange_code_for_claims(
        &config,
        CLIENT_SECRET,
        "authorization-code",
        &login.pkce_verifier,
        &login.nonce,
    )
    .await
    .map_err(|error| format!("exchange should succeed here: {error:?}"))?;
    assert_eq!(claims.subject, SUBJECT);

    let request = idp.last_token_request()?;
    assert_eq!(
        request.get("code_verifier"),
        Some(&login.pkce_verifier),
        "the token request did not carry the PKCE verifier bound to this login"
    );
    assert_eq!(
        request.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    assert_eq!(
        request.get("code").map(String::as_str),
        Some("authorization-code")
    );
    Ok(())
}

#[tokio::test]
async fn exchange_surfaces_provider_rejection_of_a_mismatched_pkce_verifier() -> TestResult {
    // Complements the test above: when a normally-behaving IdP refuses the
    // exchange because the verifier does not match the challenge, the failure
    // must propagate as an error and must not fall through to a session.
    let idp = spawn_fake_idp().await?;
    let config = idp.auth_config()?;
    let login = oidc::build_login_state(&config, CLIENT_SECRET).await?;
    idp.set_plan(TokenPlan {
        require_pkce_verifier: Some("the-verifier-from-the-real-login".to_string()),
        ..TokenPlan::valid(&login.nonce)
    })?;

    let result = oidc::exchange_code_for_claims(
        &config,
        CLIENT_SECRET,
        "authorization-code",
        "an-attacker-supplied-verifier",
        &login.nonce,
    )
    .await;

    match result {
        Ok(claims) => {
            return Err(format!(
                "provider refused the PKCE verifier, yet the exchange authenticated {:?}",
                claims.subject
            )
            .into());
        }
        Err(error) => {
            let message = error.to_string().to_ascii_lowercase();
            assert!(
                message.contains("token exchange failed"),
                "expected the token-exchange failure to propagate, got {error:?}"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 4 — signature verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exchange_rejects_id_token_signed_by_an_unpublished_key() -> TestResult {
    let idp = spawn_fake_idp().await?;

    // Same `kid` as the published JWKS entry, different key material: the
    // client finds a candidate key and must reject on the signature itself,
    // not on "no key matched".
    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            sign_with_foreign_key: true,
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "signature")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Property 5 — expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exchange_rejects_expired_id_token() -> TestResult {
    let idp = spawn_fake_idp().await?;

    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            lifetime: TimeDelta::hours(-1),
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "expire")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Adjacent bypasses on the same code path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exchange_rejects_id_token_issued_for_another_client() -> TestResult {
    let idp = spawn_fake_idp().await?;

    // A token the IdP legitimately minted for a different relying party must
    // not be accepted here.
    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            audience: Some("some-other-relying-party".to_string()),
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "audience")?;
    Ok(())
}

#[tokio::test]
async fn exchange_rejects_id_token_from_an_unrelated_issuer() -> TestResult {
    let idp = spawn_fake_idp().await?;

    // `exchange_code_for_claims` retries verification with issuer matching
    // disabled and then re-checks the issuer itself. That relaxed path must
    // still refuse an issuer unrelated to both the configured issuer host and
    // the redirect host.
    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            id_token_issuer: Some("http://attacker.example.test".to_string()),
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "issuer")?;
    Ok(())
}

#[tokio::test]
async fn exchange_rejects_token_response_without_an_id_token() -> TestResult {
    let idp = spawn_fake_idp().await?;

    // Without an ID token nothing about the user has been proven, so the
    // access token alone must not produce claims.
    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            omit_id_token: true,
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    require_auth_rejection(&result, "id token")?;
    Ok(())
}
