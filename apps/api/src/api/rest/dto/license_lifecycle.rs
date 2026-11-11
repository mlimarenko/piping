use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LicenseActivateRequest {
    pub activation_token: String,
}

impl LicenseActivateRequest {
    pub const SCHEMA_NAME: &str = "LicenseActivateRequest";
    pub const FIELD_NAMES: &[&str] = &["activationToken"];
    pub const REQUIRED_FIELDS: &[&str] = &["activationToken"];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementState {
    pub status: String,
    pub seat_limit: i32,
    pub seats_used: i32,
    pub valid_from: String,
    pub valid_until: String,
    pub version: i64,
}

impl EntitlementState {
    pub const SCHEMA_NAME: &str = "EntitlementState";
    pub const FIELD_NAMES: &[&str] = &[
        "status",
        "seatLimit",
        "seatsUsed",
        "validFrom",
        "validUntil",
        "version",
    ];
    pub const REQUIRED_FIELDS: &[&str] = Self::FIELD_NAMES;
}
