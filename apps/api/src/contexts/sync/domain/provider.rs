use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Configuration for a user-defined (non-NetBox) provider.
///
/// Validation probes `base_url` with an HTTP HEAD request.
/// Polling / event-subscription support is not implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProviderConfig {
    pub base_url: String,
    /// Bearer token sent as `Authorization: Bearer <token>` on probe requests.
    pub api_token: Option<String>,
    /// Override the auth header name (default: `Authorization`).
    pub auth_header: Option<String>,
    /// Whether to verify the TLS certificate (default: true).
    pub ssl_verify: Option<bool>,
}

/// Parse a `CustomProviderConfig` from the provider's `config` JSON value.
pub fn parse_custom_provider_config(config: &Value) -> Result<CustomProviderConfig, String> {
    let map = config
        .as_object()
        .ok_or_else(|| "Provider config must be a JSON object".to_string())?;

    let base_url = match map.get("base_url") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::String(_)) | None => {
            return Err("Provider config base_url is required for Custom provider".to_string());
        }
        Some(_) => {
            return Err("Provider config base_url must be a string".to_string());
        }
    };

    let api_token = match map.get("api_token") {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::String(_)) | None => None,
        Some(_) => return Err("Provider config api_token must be a string".to_string()),
    };

    let auth_header = match map.get("auth_header") {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::String(_)) | None => None,
        Some(_) => return Err("Provider config auth_header must be a string".to_string()),
    };

    let ssl_verify = match map.get("ssl_verify") {
        Some(Value::Bool(b)) => Some(*b),
        None => None,
        Some(_) => return Err("Provider config ssl_verify must be a boolean".to_string()),
    };

    Ok(CustomProviderConfig {
        base_url,
        api_token,
        auth_header,
        ssl_verify,
    })
}

pub const SYNC_PROVIDER_CONSUMER_TYPE: &str = "sync_provider";
pub const SYNC_PROVIDER_OWNER_SCOPE_TYPE: &str = "provider";
pub const SYNC_PROVIDER_FIELD_KEY_API_TOKEN: &str = "api_token";
pub const SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET: &str = "webhook_secret";

/// Provider type enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Netbox,
    Custom,
}

/// Provider domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: Uuid,
    pub name: String,
    pub provider_type: ProviderType,
    pub config: Value,
    pub is_enabled: bool,
}

impl Provider {
    /// Remote mutations require an explicitly bidirectional NetBox source.
    pub fn allows_remote_writes(&self) -> bool {
        self.provider_type == ProviderType::Netbox
            && self.config.get("sync_mode").and_then(Value::as_str) == Some("bidirectional")
    }
}
