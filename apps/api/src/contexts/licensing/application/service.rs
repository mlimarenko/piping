use std::time::Duration as StdDuration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::config::Settings;
use crate::contexts::licensing::domain::state::LicenseHealthSnapshot;
use crate::contexts::licensing::domain::state::LicenseLifecycleState;
use crate::contexts::licensing::infrastructure::control_plane_dto::{
    CommercialActivateRequest, CommercialEntitlementResponse, EntitlementClaims,
};
use crate::contexts::licensing::infrastructure::repositories::{
    EntitlementMetadata, LicenseStateRecord, LicensingRepository, SqlxLicensingRepository,
    SqlxSeatLedgerRepository,
};
use crate::contexts::licensing::license_verifier::{
    verify_entitlement_jwt, verify_stored_entitlement_jwt,
};
use crate::contexts::secrets::infrastructure::decrypt_secret;
use crate::contexts::secrets::infrastructure::encrypt_secret;
use crate::contexts::shared::domain::reason_codes;
use crate::contexts::shared::row::col;
use crate::errors::AppError;
use crate::observability::metrics;

mod refresh;
mod seats;

const DEFAULT_MAX_SEATS: i32 = 10;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommercialErrorEnvelope {
    code: String,
    // Validate the producer envelope, but show canonical local text instead of
    // reflecting an untrusted upstream message to users or persisted health.
    #[serde(rename = "message")]
    _message: String,
    reason_code: Option<String>,
}

#[derive(Debug)]
struct ResolvedEntitlement {
    entitlement_jwt: String,
    max_seats: i32,
    entitlement_version: String,
    entitlement_expires_at: chrono::DateTime<Utc>,
    instance_fingerprint: String,
}

#[derive(Debug)]
struct StoredEntitlement {
    max_seats: i32,
    entitlement_expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LicenseRefreshResult {
    pub status: String,
    pub synced_at: String,
    pub refresh_after_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct LicenseStatusResult {
    pub active: bool,
    pub instance_id: Option<String>,
    pub status: String,
    pub max_seats: i32,
    pub active_seats: i64,
    pub available_seats: i64,
    pub entitlement_expires_at: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LicenseActivateResult {
    pub status: String,
    pub instance_id: String,
    pub max_seats: i32,
    pub entitlement_expires_at: String,
    pub refresh_after_seconds: u64,
}

#[derive(Clone)]
pub struct LicensingService {
    settings: std::sync::Arc<Settings>,
    db_pool: PgPool,
    licensing_repo: SqlxLicensingRepository,
    seat_ledger_repo: SqlxSeatLedgerRepository,
}

impl LicensingService {
    pub fn new(settings: std::sync::Arc<Settings>, db_pool: PgPool) -> Self {
        Self {
            settings,
            licensing_repo: SqlxLicensingRepository::new(db_pool.clone()),
            seat_ledger_repo: SqlxSeatLedgerRepository::new(),
            db_pool,
        }
    }

    pub async fn load_latest_health_snapshot(
        &self,
    ) -> Result<Option<LicenseHealthSnapshot>, AppError> {
        self.licensing_repo.load_latest_health_snapshot().await
    }

    pub async fn load_entitlement_metadata(&self) -> Result<Option<EntitlementMetadata>, AppError> {
        self.licensing_repo.load_entitlement_metadata().await
    }

    pub async fn load_instance_fingerprint(&self) -> Result<Option<String>, AppError> {
        self.licensing_repo.load_instance_fingerprint().await
    }

    pub async fn get_license_status(&self) -> Result<LicenseStatusResult, AppError> {
        let license_state = self.licensing_repo.load_latest_state().await?;

        let (status, expiry) = if let Some(state) = license_state {
            let entitlement = effective_stored_entitlement(&state)?;
            let lifecycle_state = effective_lifecycle_state(&state, &entitlement, Utc::now());
            backfill_seats_for_active_sessions(&self.db_pool).await?;
            let active_seats = self.licensing_repo.count_active_seats().await?;
            let row = sqlx::query(
                r#"
                SELECT last_sync_at, last_sync_error
                FROM license_state
                WHERE instance_id = $1
                ORDER BY updated_at DESC
                LIMIT 1
                "#,
            )
            .bind(&state.instance_id)
            .fetch_optional(&self.db_pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to load status sync metadata: {error}"))
            })?;
            let (last_sync_at, last_sync_error) = match row.as_ref() {
                Some(row) => decode_status_sync_metadata(
                    col(row, "last_sync_at"),
                    col(row, "last_sync_error"),
                )?,
                None => decode_status_sync_metadata(Ok(None), Ok(None))?,
            };

            let expiry = entitlement.entitlement_expires_at;
            (
                LicenseStatusResult {
                    active: matches!(
                        lifecycle_state,
                        LicenseLifecycleState::Active
                            | LicenseLifecycleState::Degraded
                            | LicenseLifecycleState::Grace
                            | LicenseLifecycleState::Unknown
                    ),
                    instance_id: Some(state.instance_id.clone()),
                    status: lifecycle_state.as_str().to_string(),
                    max_seats: entitlement.max_seats,
                    active_seats,
                    available_seats: if lifecycle_state == LicenseLifecycleState::Suspended {
                        0
                    } else {
                        (i64::from(entitlement.max_seats) - active_seats).max(0)
                    },
                    entitlement_expires_at: Some(entitlement.entitlement_expires_at.to_rfc3339()),
                    last_sync_at,
                    last_sync_error,
                },
                Some(expiry),
            )
        } else {
            backfill_seats_for_active_sessions(&self.db_pool).await?;
            let active_seats = self.licensing_repo.count_active_seats().await?;
            (
                LicenseStatusResult {
                    active: false,
                    instance_id: None,
                    status: "inactive".to_string(),
                    max_seats: DEFAULT_MAX_SEATS,
                    active_seats,
                    available_seats: (i64::from(DEFAULT_MAX_SEATS) - active_seats).max(0),
                    entitlement_expires_at: None,
                    last_sync_at: None,
                    last_sync_error: None,
                },
                None,
            )
        };
        metrics::set_license_seat_state(
            status.active_seats,
            status.max_seats,
            status.available_seats,
            expiry,
        );
        Ok(status)
    }

    pub async fn activate_license(
        &self,
        activation_token: &str,
    ) -> Result<LicenseActivateResult, AppError> {
        let activation_token = activation_token.trim();
        if activation_token.is_empty() {
            return Err(AppError::Validation(
                "activation_token is required".to_string(),
            ));
        }

        let existing_license_state = self.licensing_repo.load_latest_state().await?;
        let (instance_id, entitlement) = if let Some(state) = existing_license_state {
            let instance_id = state.instance_id;
            let entitlement = self
                .request_entitlement_activate(activation_token, &instance_id)
                .await?;
            (instance_id, entitlement)
        } else if let Some(entitlement) = resolve_local_entitlement_token(activation_token, "")? {
            let instance_id = entitlement.instance_fingerprint.clone();
            self.licensing_repo
                .store_instance_fingerprint(&instance_id)
                .await?;
            (instance_id, entitlement)
        } else {
            let instance_id = self
                .licensing_repo
                .get_or_create_instance_fingerprint()
                .await?;
            let entitlement = self
                .request_entitlement_activate(activation_token, &instance_id)
                .await?;
            (instance_id, entitlement)
        };
        let activation_token_encrypted = encrypt_secret(activation_token)?;
        let now = Utc::now();
        let mut transaction = self.db_pool.begin().await?;
        self.licensing_repo
            .acquire_seat_lock(&mut transaction)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO license_state (
                instance_id,
                status,
                max_seats,
                entitlement_jwt,
                entitlement_version,
                entitlement_expires_at,
                activation_token_encrypted,
                last_sync_at,
                last_successful_sync_at,
                last_sync_error,
                consecutive_failures,
                created_at,
                updated_at
            )
            VALUES ($1, 'active', $2, $3, $4, $5, $6, $7, $7, NULL, 0, NOW(), NOW())
            ON CONFLICT (instance_id) DO UPDATE
            SET
                status = 'active',
                max_seats = EXCLUDED.max_seats,
                entitlement_jwt = EXCLUDED.entitlement_jwt,
                entitlement_version = EXCLUDED.entitlement_version,
                entitlement_expires_at = EXCLUDED.entitlement_expires_at,
                activation_token_encrypted = EXCLUDED.activation_token_encrypted,
                last_sync_at = EXCLUDED.last_sync_at,
                last_successful_sync_at = EXCLUDED.last_successful_sync_at,
                last_sync_error = NULL,
                last_failure_kind = NULL,
                consecutive_failures = 0,
                grace_started_at = NULL,
                grace_ends_at = NULL,
                revision = license_state.revision + 1,
                updated_at = NOW()
            "#,
        )
        .bind(&instance_id)
        .bind(entitlement.max_seats)
        .bind(&entitlement.entitlement_jwt)
        .bind(&entitlement.entitlement_version)
        .bind(entitlement.entitlement_expires_at)
        .bind(activation_token_encrypted)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to persist license activation: {error}"))
        })?;

        backfill_seats_for_active_sessions_tx(&self.licensing_repo, &mut transaction).await?;
        self.licensing_repo
            .evict_over_capacity(&mut transaction, &instance_id, entitlement.max_seats)
            .await?;
        transaction.commit().await?;

        Ok(LicenseActivateResult {
            status: "active".to_string(),
            instance_id,
            max_seats: entitlement.max_seats,
            entitlement_expires_at: entitlement.entitlement_expires_at.to_rfc3339(),
            refresh_after_seconds: self.settings.license.refresh_interval_seconds,
        })
    }

    async fn request_entitlement_activate(
        &self,
        activation_token: &str,
        instance_id: &str,
    ) -> Result<ResolvedEntitlement, AppError> {
        if let Some(entitlement) = resolve_local_entitlement_token(activation_token, instance_id)? {
            return Ok(entitlement);
        }

        let base_url = require_license_control_plane_base_url(&self.settings.license.base_url)?;
        let payload = CommercialActivateRequest {
            activation_token: activation_token.to_string(),
            instance_fingerprint: instance_id.to_string(),
            instance_name: instance_id.to_string(),
        };
        request_entitlement_remote(
            &self.settings,
            &format!("{base_url}/api/v1/instances/activate"),
            &payload,
            instance_id,
            true,
        )
        .await
    }
}

fn decode_status_sync_metadata(
    last_sync_at: Result<Option<chrono::DateTime<Utc>>, AppError>,
    last_sync_error: Result<Option<String>, AppError>,
) -> Result<(Option<String>, Option<String>), AppError> {
    Ok((
        last_sync_at?.map(|value| value.to_rfc3339()),
        last_sync_error?,
    ))
}

fn effective_stored_entitlement(state: &LicenseStateRecord) -> Result<StoredEntitlement, AppError> {
    let verified = verify_stored_entitlement_jwt(&state.entitlement_jwt).map_err(|error| {
        metrics::record_license_anomaly("stored_entitlement_jwt_invalid", "stored_entitlement");
        AppError::Validation(format!(
            "{}: {error}",
            reason_codes::ENTITLEMENT_BINDING_INVALID
        ))
    })?;
    validate_stored_entitlement_binding(state, &verified)?;

    let entitlement_expires_at = datetime_from_unix(verified.exp).ok_or_else(|| {
        metrics::record_license_anomaly("stored_entitlement_exp_invalid", "stored_entitlement");
        AppError::Validation(format!(
            "{}: Stored entitlement exp is not a valid timestamp",
            reason_codes::ENTITLEMENT_BINDING_INVALID
        ))
    })?;

    Ok(StoredEntitlement {
        max_seats: verified.max_seats.max(0),
        entitlement_expires_at,
    })
}

fn validate_stored_entitlement_binding(
    state: &LicenseStateRecord,
    claims: &EntitlementClaims,
) -> Result<(), AppError> {
    if claims.instance_fingerprint != state.instance_id {
        metrics::record_license_anomaly("fingerprint_mismatch", "stored_entitlement");
        return Err(AppError::Validation(format!(
            "{}: Stored entitlement instance fingerprint mismatch",
            reason_codes::ENTITLEMENT_BINDING_INVALID
        )));
    }
    Ok(())
}

fn effective_lifecycle_state(
    state: &LicenseStateRecord,
    entitlement: &StoredEntitlement,
    now: chrono::DateTime<Utc>,
) -> LicenseLifecycleState {
    if state.status == "suspended" {
        return LicenseLifecycleState::Suspended;
    }
    if entitlement.entitlement_expires_at <= now {
        return LicenseLifecycleState::Expired;
    }
    LicenseLifecycleState::parse(&state.status)
}

fn resolve_local_entitlement_token(
    token: &str,
    expected_instance_fingerprint: &str,
) -> Result<Option<ResolvedEntitlement>, AppError> {
    let token = token.trim();
    if !looks_like_jwt(token) {
        return Ok(None);
    }

    let verified = verify_entitlement_jwt(token)?;
    if !expected_instance_fingerprint.trim().is_empty()
        && verified.instance_fingerprint != expected_instance_fingerprint
    {
        metrics::record_license_anomaly("fingerprint_mismatch", "local_entitlement_verify");
        return Err(AppError::Validation(
            "Entitlement instance fingerprint mismatch".to_string(),
        ));
    }

    let entitlement_expires_at = datetime_from_unix(verified.exp).ok_or_else(|| {
        AppError::Validation("Entitlement JWT exp is not a valid timestamp".to_string())
    })?;

    Ok(Some(ResolvedEntitlement {
        entitlement_jwt: token.to_string(),
        max_seats: verified.max_seats.max(0),
        entitlement_version: verified.entitlement_version,
        entitlement_expires_at,
        instance_fingerprint: verified.instance_fingerprint,
    }))
}

fn looks_like_jwt(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(header), Some(claims), Some(signature), None)
            if !header.is_empty() && !claims.is_empty() && !signature.is_empty()
    )
}

async fn backfill_seats_for_active_sessions(pool: &PgPool) -> Result<(), AppError> {
    let mut transaction = pool.begin().await.map_err(|error| {
        AppError::Database(format!(
            "Failed to begin seat backfill transaction: {error}"
        ))
    })?;
    let repository = SqlxLicensingRepository::new(pool.clone());
    backfill_seats_for_active_sessions_tx(&repository, &mut transaction).await?;
    transaction.commit().await?;
    Ok(())
}

async fn backfill_seats_for_active_sessions_tx(
    repository: &SqlxLicensingRepository,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(421_001_i64)
        .execute(&mut **transaction)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to acquire seat backfill lock: {error}"))
        })?;

    // Re-resolve under the same lock as refusal persistence and seat allocation.
    // A status read or replacement must not backfill from an obsolete limit.
    let max_seats = match repository.load_latest_state_tx(transaction).await? {
        Some(state) => {
            let entitlement = effective_stored_entitlement(&state)?;
            if !matches!(
                effective_lifecycle_state(&state, &entitlement, Utc::now()),
                LicenseLifecycleState::Active | LicenseLifecycleState::Degraded
            ) {
                return Ok(());
            }
            entitlement.max_seats
        }
        None => DEFAULT_MAX_SEATS,
    };

    let active_seat_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM license_seat_assignment lsa
        JOIN identity_user iu ON iu.id = lsa.user_id
        WHERE lsa.revoked_at IS NULL
          AND iu.status = 'active'
        "#,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to count active seats for backfill: {error}"
        ))
    })?;

    let remaining = i64::from(max_seats) - active_seat_count;
    if remaining <= 0 {
        return Ok(());
    }

    sqlx::query(
        r#"
        WITH candidate_users AS (
            SELECT DISTINCT s.user_id
            FROM auth_session s
            JOIN identity_user iu ON iu.id = s.user_id
            LEFT JOIN license_seat_assignment lsa
              ON lsa.user_id = s.user_id
             AND lsa.revoked_at IS NULL
            WHERE s.revoked_at IS NULL
              AND s.expires_at > NOW()
              AND iu.status = 'active'
              AND lsa.user_id IS NULL
            ORDER BY s.user_id
            LIMIT $1
        )
        INSERT INTO license_seat_assignment (user_id)
        SELECT user_id
        FROM candidate_users
        ON CONFLICT (user_id) WHERE revoked_at IS NULL DO NOTHING
        "#,
    )
    .bind(remaining)
    .execute(&mut **transaction)
    .await
    .map_err(|error| {
        AppError::Database(format!("Failed to backfill active session seats: {error}"))
    })?;

    Ok(())
}

fn classify_refresh_failure_kind(error: &AppError) -> &'static str {
    match error {
        AppError::LicenseIssuerDenied(reason) => reason.code(),
        AppError::LicenseEntitlementInvalid(_) => "ENTITLEMENT_INVALID",
        AppError::LicenseResponseInvalid => "CONTROL_PLANE_RESPONSE_INVALID",
        AppError::External(_) | AppError::ExternalSource(_) | AppError::Provider(_) => {
            "CONTROL_PLANE_UNAVAILABLE"
        }
        AppError::Validation(_) => "CONTROL_PLANE_RESPONSE_INVALID",
        AppError::Database(_) | AppError::DatabaseSource(_) => "LOCAL_PERSISTENCE_ERROR",
        _ => "UNKNOWN_REFRESH_FAILURE",
    }
}

fn refresh_failure_persistence_error(
    refresh_error: &AppError,
    persistence_error: AppError,
) -> AppError {
    if matches!(persistence_error, AppError::LicenseRefreshSuperseded) {
        return persistence_error;
    }
    AppError::Database(format!(
        "Failed to persist license enforcement state after refresh failure; \
         persistence error: {persistence_error}; original refresh error: {refresh_error}"
    ))
}

fn resolve_activation_token_for_refresh(stored_value: &str) -> Result<String, AppError> {
    let trimmed = stored_value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "Stored activation token is empty".to_string(),
        ));
    }
    if trimmed.starts_with("enc:v1:") {
        return decrypt_secret(trimmed);
    }
    Ok(trimmed.to_string())
}

fn require_license_control_plane_base_url(base_url: &str) -> Result<&str, AppError> {
    let normalized = base_url.trim();
    if normalized.is_empty() {
        return Err(AppError::Validation(
            "License control-plane base_url is not configured".to_string(),
        ));
    }
    Ok(normalized)
}

async fn request_entitlement_remote<T: Serialize>(
    settings: &Settings,
    url: &str,
    payload: &T,
    expected_instance_fingerprint: &str,
    activation: bool,
) -> Result<ResolvedEntitlement, AppError> {
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_millis(
            settings.license.request_timeout_ms,
        ))
        .build()
        .map_err(|error| {
            AppError::External(format!("Failed to build license HTTP client: {error}"))
        })?;

    let response = client
        .post(url)
        .header("X-Piping-CSRF", "1")
        .json(payload)
        .send()
        .await
        .map_err(|error| AppError::External(format!("License server request failed: {error}")))?;

    let status = response.status();
    let body_bytes = response
        .bytes()
        .await
        .map_err(|_| AppError::External("License service response body unavailable".to_string()))?;
    if !status.is_success() {
        if let Ok(payload) = serde_json::from_slice::<CommercialErrorEnvelope>(&body_bytes) {
            return Err(map_control_plane_error(status, payload, activation));
        }
        return Err(
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                AppError::External(format!(
                    "License service temporarily unavailable ({status})"
                ))
            } else {
                AppError::LicenseResponseInvalid
            },
        );
    }

    let body: CommercialEntitlementResponse =
        serde_json::from_slice(&body_bytes).map_err(|_| AppError::LicenseResponseInvalid)?;

    // Seat count, expiry and version are bound to the verified, signed entitlement
    // JWT only; the unsigned HTTP response body cannot inflate or override them.
    let verified = verify_entitlement_jwt(&body.entitlement_jwt)
        .map_err(|error| AppError::LicenseEntitlementInvalid(error.to_string()))?;
    if !expected_instance_fingerprint.trim().is_empty()
        && verified.instance_fingerprint != expected_instance_fingerprint
    {
        metrics::record_license_anomaly("fingerprint_mismatch", "entitlement_verify");
        return Err(AppError::LicenseEntitlementInvalid(
            "Entitlement instance fingerprint mismatch".to_string(),
        ));
    }
    let entitlement_expires_at = datetime_from_unix(verified.exp).ok_or_else(|| {
        AppError::Validation("Entitlement JWT exp is not a valid timestamp".to_string())
    })?;
    let _refresh_after_seconds = body
        .refresh_after_seconds
        .unwrap_or(settings.license.refresh_interval_seconds);

    Ok(ResolvedEntitlement {
        entitlement_jwt: body.entitlement_jwt,
        max_seats: verified.max_seats.max(0),
        entitlement_version: verified.entitlement_version,
        entitlement_expires_at,
        instance_fingerprint: verified.instance_fingerprint,
    })
}

fn map_control_plane_error(
    status: reqwest::StatusCode,
    payload: CommercialErrorEnvelope,
    activation: bool,
) -> AppError {
    use crate::contexts::licensing::domain::issuer_refusal::IssuerRefusal;
    if let Some(reason) = IssuerRefusal::from_response(
        status.as_u16(),
        &payload.code,
        payload.reason_code.as_deref(),
        activation,
    ) {
        return AppError::LicenseIssuerDenied(reason);
    }
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return AppError::External(format!(
            "License service temporarily unavailable ({status})"
        ));
    }
    AppError::LicenseResponseInvalid
}

fn datetime_from_unix(seconds: i64) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::from_timestamp(seconds, 0)
}

#[cfg(test)]
mod tests {
    use super::decode_status_sync_metadata;
    use crate::errors::AppError;

    #[test]
    fn absent_sync_metadata_remains_none_but_failed_reads_are_errors() {
        let empty = decode_status_sync_metadata(Ok(None), Ok(None))
            .expect("nullable sync metadata may be absent");
        assert_eq!(empty, (None, None));

        assert!(
            decode_status_sync_metadata(
                Err(AppError::Database("last_sync_at decode failed".to_string())),
                Ok(None),
            )
            .is_err()
        );
        assert!(
            decode_status_sync_metadata(
                Ok(None),
                Err(AppError::Database(
                    "last_sync_error decode failed".to_string(),
                )),
            )
            .is_err()
        );
    }
}
