use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

pub const OIDC_CLIENT_SECRET_CONSUMER_TYPE: &str = "oidc_settings";
pub const OIDC_CLIENT_SECRET_FIELD_KEY: &str = "client_secret";
pub const OIDC_CLIENT_SECRET_OWNER_SCOPE_TYPE: &str = "system";

const OIDC_CLIENT_SECRET_CONSUMER_ID_RAW: &str = "00000000-0000-7000-8000-0000000000a1";
const OIDC_CLIENT_SECRET_OWNER_SCOPE_ID_RAW: &str = "00000000-0000-7000-8000-0000000000a1";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OidcProvisioningMode {
    #[default]
    Manual,
    Jit,
}

impl OidcProvisioningMode {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Ok(Self::Manual),
            "jit" => Ok(Self::Jit),
            _ => Err(AppError::Validation(
                "Invalid OIDC provisioning_mode; expected manual or jit".to_string(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Jit => "jit",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcSettings {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub group_claim: String,
    pub provisioning_mode: OidcProvisioningMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OidcClientSecretState {
    pub enabled: bool,
    pub has_value: bool,
}

pub fn oidc_client_secret_consumer_id() -> Result<Uuid, AppError> {
    Uuid::parse_str(OIDC_CLIENT_SECRET_CONSUMER_ID_RAW).map_err(|error| {
        AppError::Internal(format!(
            "Invalid OIDC client secret consumer UUID constant: {error}"
        ))
    })
}

pub fn oidc_client_secret_owner_scope_id() -> Result<Uuid, AppError> {
    Uuid::parse_str(OIDC_CLIENT_SECRET_OWNER_SCOPE_ID_RAW).map_err(|error| {
        AppError::Internal(format!(
            "Invalid OIDC client secret owner scope UUID constant: {error}"
        ))
    })
}
