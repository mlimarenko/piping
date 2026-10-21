use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use tokio::sync::RwLock;

#[path = "license_http_contract.rs"]
mod license_http_contract;

use crate::common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie,
};

pub(crate) const LICENSE_ACTIVATE_PATH: &str = "/api/v1/license/activate";
#[allow(dead_code)]
pub(crate) const MALICIOUS_CLIENT_INSTANCE_FINGERPRINT: &str = "client-controlled-instance";
pub(crate) const LICENSE_MOCK_ORGANIZATION_ID: &str = "00000000-0000-7000-8000-000000000099";
pub(crate) const LICENSE_MAX_SEATS_DEFAULT: i32 = 5;
pub(crate) const LICENSE_REFRESH_AFTER_SECONDS: u64 = 60;
pub(crate) const LICENSE_CLOCK_SKEW_SECONDS: i64 = 5;
pub(crate) const LICENSE_TOKEN_VALIDITY_HOURS: i64 = 1;
const MOCK_LICENSE_BIND_ADDR: &str = "127.0.0.1:0";

pub(crate) const MOCK_MODE_ACTIVE: u8 = 1;
pub(crate) const MOCK_MODE_REVOKED: u8 = 4;

pub(crate) struct LicenseAdminHarness {
    pub app: Router,
    pub db_pool: sqlx::PgPool,
    pub admin_cookie: String,
    #[allow(dead_code)]
    pub mock: MockLicenseServer,
}

pub(crate) async fn build_license_admin_harness(
    mock_mode: u8,
) -> Result<LicenseAdminHarness, String> {
    ensure_jsonwebtoken_provider_installed();
    let mock = start_mock_license_server()
        .await
        .map_err(|e| format!("start mock license server: {e}"))?;
    mock.mode.store(mock_mode, Ordering::SeqCst);

    let state = build_test_app_state()
        .await
        .map_err(|error| format!("integration stack unavailable: {error}"))?;
    let db_pool = state.db_pool.clone();
    let state = with_license_settings(state, &mock.base_url)
        .await
        .map_err(|e| format!("with license settings: {e}"))?;
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "integration stack unavailable: admin login failed".to_string())?;

    Ok(LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
        mock,
    })
}

pub(crate) async fn reset_license_tables(
    pool: &sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("DELETE FROM license_seat_ledger")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_seat_ledger: {e}"))?;
    sqlx::query("DELETE FROM license_seat_assignment")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_seat_assignment: {e}"))?;
    sqlx::query("DELETE FROM license_state")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_state: {e}"))?;
    sqlx::query("DELETE FROM license_instance_identity")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_instance_identity: {e}"))?;
    sqlx::query("DELETE FROM audit_event WHERE event_type LIKE 'timeline.activation_%'")
        .execute(pool)
        .await
        .map_err(|e| format!("delete activation timeline events: {e}"))?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) async fn latest_license_instance_id(
    pool: &sqlx::PgPool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT instance_id
        FROM license_state
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("latest license instance id: {e}"))?)
}

pub(crate) async fn latest_timeline_metadata(
    pool: &sqlx::PgPool,
    event_type: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, Value>(
        r#"
        SELECT metadata
        FROM audit_event
        WHERE event_type = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(event_type)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("timeline metadata: {e}"))?)
}

#[allow(dead_code)]
pub(crate) async fn active_assignment_count(
    pool: &sqlx::PgPool,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM license_seat_assignment
        WHERE revoked_at IS NULL
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("active assignments count: {e}"))?)
}

async fn with_license_settings(
    state: piping_backend::app::AppState,
    base_url: &str,
) -> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    let mut updated = (*state.settings).clone();
    updated.license.base_url = base_url.to_string();
    Ok(piping_backend::app::AppState::build(
        updated.into(),
        state.db_pool.clone(),
        state.redis.clone(),
    )
    .await
    .map_err(|e| format!("rebuild app state with license settings: {e}"))?)
}

#[derive(Clone)]
pub(crate) struct MockLicenseServer {
    pub base_url: String,
    pub mode: Arc<AtomicU8>,
}

#[derive(Clone)]
struct MockLicenseState {
    mode: Arc<AtomicU8>,
    last_instance_fingerprint: Arc<RwLock<Option<String>>>,
}

#[derive(Debug, Deserialize)]
struct MockActivateRequest {
    instance_fingerprint: String,
}

#[derive(Debug, Deserialize)]
struct MockRefreshRequest {
    activation_token: String,
}

#[derive(Debug, Serialize)]
struct MockEntitlementResponse {
    entitlement_jwt: String,
    refresh_after_seconds: u64,
    max_seats: i32,
    entitlement_version: String,
    entitlement_expires_at: String,
}

#[derive(Debug, Serialize)]
struct MockClaims {
    iss: String,
    aud: String,
    sub: String,
    iat: i64,
    nbf: i64,
    exp: i64,
    organization_id: String,
    instance_fingerprint: String,
    max_seats: i32,
    entitlement_version: String,
}

async fn start_mock_license_server()
-> Result<MockLicenseServer, Box<dyn std::error::Error + Send + Sync>> {
    let mode = Arc::new(AtomicU8::new(MOCK_MODE_ACTIVE));
    let state = MockLicenseState {
        mode: mode.clone(),
        last_instance_fingerprint: Arc::new(RwLock::new(None)),
    };
    let app = Router::new()
        .route("/api/v1/instances/activate", post(mock_activate_handler))
        .route("/api/v1/instances/refresh", post(mock_refresh_handler))
        .route_layer(axum::middleware::from_fn(
            license_http_contract::require_cloud_csrf,
        ))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(MOCK_LICENSE_BIND_ADDR)
        .await
        .map_err(|e| format!("bind mock server: {e}"))?;
    let addr: SocketAddr = listener
        .local_addr()
        .map_err(|e| format!("local addr: {e}"))?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(MockLicenseServer {
        base_url: format!("http://{}", addr),
        mode,
    })
}

async fn mock_activate_handler(
    State(state): State<MockLicenseState>,
    Json(payload): Json<MockActivateRequest>,
) -> Response {
    match state.mode.load(Ordering::SeqCst) {
        MOCK_MODE_REVOKED => (
            axum::http::StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": "TOKEN_REVOKED",
                "message": "Token has been revoked",
                "reasonCode": "TOKEN_REVOKED",
            })),
        )
            .into_response(),
        _ => {
            *state.last_instance_fingerprint.write().await =
                Some(payload.instance_fingerprint.clone());
            match build_mock_entitlement_response(&payload.instance_fingerprint) {
                Ok(response) => Json(response).into_response(),
                Err(error) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                )
                    .into_response(),
            }
        }
    }
}

async fn mock_refresh_handler(
    State(state): State<MockLicenseState>,
    Json(payload): Json<MockRefreshRequest>,
) -> Response {
    let _ = payload.activation_token;
    match state.mode.load(Ordering::SeqCst) {
        MOCK_MODE_REVOKED => (
            axum::http::StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": "TOKEN_REVOKED",
                "message": "Token has been revoked",
                "reasonCode": "TOKEN_REVOKED",
            })),
        )
            .into_response(),
        _ => {
            let instance_fingerprint = state
                .last_instance_fingerprint
                .read()
                .await
                .clone()
                .unwrap_or_else(|| "mock-unactivated-instance".to_string());
            match build_mock_entitlement_response(&instance_fingerprint) {
                Ok(response) => Json(response).into_response(),
                Err(error) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                )
                    .into_response(),
            }
        }
    }
}

fn build_mock_entitlement_response(
    instance_fingerprint: &str,
) -> Result<MockEntitlementResponse, Box<dyn std::error::Error + Send + Sync>> {
    let expires_at = Utc::now() + chrono::Duration::hours(LICENSE_TOKEN_VALIDITY_HOURS);
    let token =
        build_test_entitlement_token(instance_fingerprint, LICENSE_MAX_SEATS_DEFAULT, expires_at)?;

    Ok(MockEntitlementResponse {
        entitlement_jwt: token,
        refresh_after_seconds: LICENSE_REFRESH_AFTER_SECONDS,
        max_seats: LICENSE_MAX_SEATS_DEFAULT,
        entitlement_version: "v1".to_string(),
        entitlement_expires_at: expires_at.to_rfc3339(),
    })
}

pub(crate) fn build_test_entitlement_token(
    instance_fingerprint: &str,
    max_seats: i32,
    expires_at: chrono::DateTime<Utc>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    ensure_jsonwebtoken_provider_installed();
    let now = Utc::now();
    let issued_at = (now - chrono::Duration::seconds(LICENSE_CLOCK_SKEW_SECONDS))
        .min(expires_at - chrono::Duration::minutes(1));
    let claims = MockClaims {
        iss: "piping-com-backend".to_string(),
        aud: "piping-instance".to_string(),
        sub: "org:test".to_string(),
        iat: issued_at.timestamp(),
        nbf: issued_at.timestamp(),
        exp: expires_at.timestamp(),
        organization_id: LICENSE_MOCK_ORGANIZATION_ID.to_string(),
        instance_fingerprint: instance_fingerprint.to_string(),
        max_seats,
        entitlement_version: "v1".to_string(),
    };
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some("k1".to_string());
    encode(
        &header,
        &claims,
        &EncodingKey::from_ed_pem(TEST_PRIVATE_KEY_1.as_bytes())
            .map_err(|e| format!("private key: {e}"))?,
    )
    .map_err(|e| format!("encode entitlement: {e}").into())
}

fn ensure_jsonwebtoken_provider_installed() {
    // No-op for this crate build: jsonwebtoken provider is selected by enabled features.
}

const TEST_PRIVATE_KEY_1: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEID7TRf9+jjgEXCFuwTjFPuEpWqPTMu/Gljl46gTm48lT
-----END PRIVATE KEY-----";
