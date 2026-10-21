//! Wire DTOs for the commercial license control plane.
//!
//! These mirror the request/response shapes exchanged with the `com-backend`
//! activate/refresh endpoints and the verified entitlement-JWT claim set. The
//! control plane is not the trust root: only the entitlement JWT (verified by
//! [`crate::contexts::licensing::license_verifier`]) is authoritative. Unsigned
//! HTTP response fields are advisory.

use serde::{Deserialize, Serialize};

/// Verified entitlement-JWT claim set (authoritative seat/expiry source).
#[derive(Debug, Deserialize)]
pub struct EntitlementClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub organization_id: String,
    pub instance_fingerprint: String,
    pub max_seats: i32,
    pub entitlement_version: String,
}

/// Activate/refresh response envelope from the control plane.
///
/// The control plane may still send advisory `max_seats`,
/// `entitlement_version` and `entitlement_expires_at`; they are deliberately
/// not modelled here. Seat count, version and expiry are taken from the
/// verified entitlement JWT only, and serde ignores the unmodelled fields.
#[derive(Debug, Deserialize)]
pub struct CommercialEntitlementResponse {
    pub entitlement_jwt: String,
    pub refresh_after_seconds: Option<u64>,
}

/// Activate request payload sent to the control plane.
#[derive(Debug, Serialize)]
pub struct CommercialActivateRequest {
    pub activation_token: String,
    pub instance_fingerprint: String,
    pub instance_name: String,
}

/// Refresh request payload sent to the control plane.
#[derive(Debug, Serialize)]
pub struct CommercialRefreshRequest {
    pub activation_token: String,
    pub last_entitlement_version: String,
}
