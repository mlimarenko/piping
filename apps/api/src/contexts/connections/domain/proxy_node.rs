use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyNode {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub base_url: String,
    pub enabled: bool,
    pub secret_version: i32,
    pub secret_rotated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProxyNode {
    pub fn validate(&self) -> Result<(), ProxyNodeValidationError> {
        if self.name.trim().is_empty() {
            return Err(ProxyNodeValidationError::EmptyName);
        }
        if self.slug.trim().is_empty() {
            return Err(ProxyNodeValidationError::EmptySlug);
        }
        validate_proxy_base_url(&self.base_url)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ProxyNodeValidationError {
    #[error("proxy node name cannot be empty")]
    EmptyName,
    #[error("proxy node slug cannot be empty")]
    EmptySlug,
    #[error("proxy node base_url cannot be empty")]
    EmptyBaseUrl,
    #[error("proxy node base_url must use http or https scheme")]
    InvalidBaseUrlScheme,
    #[error("proxy node base_url is invalid: {0}")]
    InvalidBaseUrl(String),
}

pub fn validate_proxy_base_url(base_url: &str) -> Result<(), ProxyNodeValidationError> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(ProxyNodeValidationError::EmptyBaseUrl);
    }

    let parsed = Url::parse(trimmed)
        .map_err(|error| ProxyNodeValidationError::InvalidBaseUrl(error.to_string()))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ProxyNodeValidationError::InvalidBaseUrlScheme);
    }
    Ok(())
}
