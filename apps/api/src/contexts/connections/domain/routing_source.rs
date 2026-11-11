use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSource {
    RequestOverride,
    ProxyAssignment,
    EmbeddedDefault,
}

impl RoutingSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoutingSource::RequestOverride => "request_override",
            RoutingSource::ProxyAssignment => "proxy_assignment",
            RoutingSource::EmbeddedDefault => "embedded_default",
        }
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RoutingSourceParseError {
    #[error("routing source cannot be empty")]
    Empty,
    #[error("invalid routing source: {0}")]
    Invalid(String),
}

pub fn parse_routing_source(value: &str) -> Result<RoutingSource, RoutingSourceParseError> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(RoutingSourceParseError::Empty);
    }

    match normalized.as_str() {
        "request_override" => Ok(RoutingSource::RequestOverride),
        "proxy_assignment" => Ok(RoutingSource::ProxyAssignment),
        "embedded_default" => Ok(RoutingSource::EmbeddedDefault),
        _ => Err(RoutingSourceParseError::Invalid(value.to_string())),
    }
}
