//! Single trust root for entitlement-JWT verification.
//!
//! The verification public key is baked into the binary at compile time. Release
//! builds fail to compile unless `PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM` is provided
//! as a build-time env (docker build-arg), so shipped images cannot be coaxed into
//! trusting a different key or skipping verification via runtime config. Debug/test
//! builds fall back to the committed dev key.

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

use crate::contexts::licensing::infrastructure::control_plane_dto::EntitlementClaims;
use crate::errors::AppError;
use crate::observability::metrics;

/// Committed dev/test Ed25519 public key. Matches the signing key used by the
/// local `com-backend` in `docker-compose.licensing.yml` and the test fixtures.
#[cfg(debug_assertions)]
const DEV_FALLBACK_LICENSE_JWT_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA6wfrFIl5II8AGPRbrIZqj2FeKAt3uiXeYyh/pAlWMKA=\n-----END PUBLIC KEY-----\n";

/// Baked entitlement-JWT verification key. Debug builds fall back to the dev key;
/// release builds MUST bake a key at compile time or fail to compile.
#[cfg(debug_assertions)]
pub const LICENSE_JWT_PUBLIC_KEY_PEM: &str =
    match option_env!("PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM") {
        Some(key) => key,
        None => DEV_FALLBACK_LICENSE_JWT_PUBLIC_KEY_PEM,
    };

/// Baked entitlement-JWT verification key for release builds. A missing build-arg
/// is a compile-time error (fail-closed): release images cannot ship without a key.
#[cfg(not(debug_assertions))]
pub const LICENSE_JWT_PUBLIC_KEY_PEM: &str = match option_env!(
    "PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM"
) {
    Some(key) => key,
    None => panic!(
        "release builds must bake the license public key: set the PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM build-time env (docker build-arg)"
    ),
};

/// Expected issuer (`iss`) claim in entitlement JWTs.
pub const LICENSE_JWT_ISSUER: &str = "piping-com-backend";

/// Expected audience (`aud`) claim in entitlement JWTs.
pub const LICENSE_JWT_AUDIENCE: &str = "piping-instance";

/// Allowed clock skew (seconds) for entitlement-JWT validation.
pub const LICENSE_JWT_CLOCK_SKEW_SECONDS: u64 = 30;

/// Maximum accepted entitlement-JWT size in bytes.
const MAX_ENTITLEMENT_JWT_BYTES: usize = 32 * 1024;

/// Verifies a fresh entitlement JWT against the baked public key and returns its claims.
///
/// Activation and refresh must fail on expired tokens. Stored entitlement reads
/// use [`verify_stored_entitlement_jwt`] so the service can still classify an
/// expired but correctly signed token as expired instead of invalid.
pub fn verify_entitlement_jwt(entitlement_jwt: &str) -> Result<EntitlementClaims, AppError> {
    verify_entitlement_jwt_with_exp_policy(entitlement_jwt, true)
}

/// Verifies a persisted entitlement JWT without accepting DB-cached seat/expiry values.
///
/// This path still enforces size, algorithm, signature, issuer, audience, `nbf`
/// and domain claim invariants. It intentionally does not reject solely because
/// `exp` is in the past; callers must derive expiry policy from the signed `exp`
/// claim and deny seats when it has elapsed.
pub fn verify_stored_entitlement_jwt(entitlement_jwt: &str) -> Result<EntitlementClaims, AppError> {
    verify_entitlement_jwt_with_exp_policy(entitlement_jwt, false)
}

fn verify_entitlement_jwt_with_exp_policy(
    entitlement_jwt: &str,
    validate_exp: bool,
) -> Result<EntitlementClaims, AppError> {
    if entitlement_jwt.len() > MAX_ENTITLEMENT_JWT_BYTES {
        return Err(AppError::Validation(
            "Entitlement JWT size exceeds maximum allowed limit".to_string(),
        ));
    }

    let header = decode_header(entitlement_jwt).map_err(|error| {
        AppError::Validation(format!("Invalid entitlement JWT header: {error}"))
    })?;
    if header.alg != Algorithm::EdDSA {
        return Err(AppError::Validation(
            "Entitlement JWT algorithm must be EdDSA".to_string(),
        ));
    }

    let decoding_key =
        DecodingKey::from_ed_pem(LICENSE_JWT_PUBLIC_KEY_PEM.as_bytes()).map_err(|error| {
            AppError::Validation(format!("Invalid license JWT public key: {error}"))
        })?;

    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_issuer(&[LICENSE_JWT_ISSUER]);
    validation.set_audience(&[LICENSE_JWT_AUDIENCE]);
    validation.leeway = LICENSE_JWT_CLOCK_SKEW_SECONDS;
    validation.validate_exp = validate_exp;
    validation.validate_nbf = true;
    validation.required_spec_claims = std::collections::HashSet::from([
        "iss".to_string(),
        "aud".to_string(),
        "sub".to_string(),
        "exp".to_string(),
        "iat".to_string(),
        "nbf".to_string(),
    ]);

    let claims = decode::<EntitlementClaims>(entitlement_jwt, &decoding_key, &validation)
        .map(|token| token.claims)
        .map_err(|error| {
            metrics::record_license_anomaly(
                "jwt_signature_or_claim_validation_failed",
                "baked_key",
            );
            AppError::Validation(format!("Entitlement JWT verification failed: {error}"))
        })?;
    validate_entitlement_claim_invariants(&claims)?;
    Ok(claims)
}

fn validate_entitlement_claim_invariants(claims: &EntitlementClaims) -> Result<(), AppError> {
    if claims.sub.trim().is_empty() || claims.organization_id.trim().is_empty() {
        metrics::record_license_anomaly("jwt_subject_or_org_missing", "claim_invariants");
        return Err(AppError::Validation(
            "Entitlement JWT claims are missing subject or organization_id".to_string(),
        ));
    }
    if claims.instance_fingerprint.trim().is_empty() {
        metrics::record_license_anomaly("jwt_instance_fingerprint_missing", "claim_invariants");
        return Err(AppError::Validation(
            "Entitlement JWT instance_fingerprint is required".to_string(),
        ));
    }
    if claims.max_seats < 0 {
        metrics::record_license_anomaly("jwt_negative_seats", "claim_invariants");
        return Err(AppError::Validation(
            "Entitlement JWT max_seats cannot be negative".to_string(),
        ));
    }

    let leeway = i64::try_from(LICENSE_JWT_CLOCK_SKEW_SECONDS).unwrap_or(i64::MAX);
    let now = chrono::Utc::now().timestamp();
    if claims.iat > now.saturating_add(leeway) {
        metrics::record_license_anomaly("jwt_iat_in_future", "claim_invariants");
        return Err(AppError::Validation(
            "Entitlement JWT iat is too far in the future".to_string(),
        ));
    }
    if claims.nbf > claims.exp {
        metrics::record_license_anomaly("jwt_nbf_after_exp", "claim_invariants");
        return Err(AppError::Validation(
            "Entitlement JWT nbf cannot be later than exp".to_string(),
        ));
    }
    if claims.iat > claims.exp {
        metrics::record_license_anomaly("jwt_iat_after_exp", "claim_invariants");
        return Err(AppError::Validation(
            "Entitlement JWT iat cannot be later than exp".to_string(),
        ));
    }

    Ok(())
}
