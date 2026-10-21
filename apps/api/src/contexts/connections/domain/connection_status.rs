use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Session lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Pending,
    Active,
    Disconnected,
    Terminated,
    Failed,
}

impl ConnectionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionStatus::Pending => "pending",
            ConnectionStatus::Active => "active",
            ConnectionStatus::Disconnected => "disconnected",
            ConnectionStatus::Terminated => "terminated",
            ConnectionStatus::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        parse_connection_status(value).ok()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ConnectionStatus::Disconnected
                | ConnectionStatus::Terminated
                | ConnectionStatus::Failed
        )
    }

    pub fn can_transition_to(&self, next: &Self) -> bool {
        match self {
            ConnectionStatus::Pending => matches!(
                next,
                ConnectionStatus::Active
                    | ConnectionStatus::Disconnected
                    | ConnectionStatus::Terminated
                    | ConnectionStatus::Failed
            ),
            ConnectionStatus::Active => matches!(
                next,
                ConnectionStatus::Disconnected
                    | ConnectionStatus::Terminated
                    | ConnectionStatus::Failed
            ),
            ConnectionStatus::Disconnected
            | ConnectionStatus::Terminated
            | ConnectionStatus::Failed => false,
        }
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConnectionStatusParseError {
    #[error("connection status cannot be empty")]
    Empty,
    #[error("invalid connection status: {0}")]
    Invalid(String),
}

pub fn parse_connection_status(
    value: &str,
) -> Result<ConnectionStatus, ConnectionStatusParseError> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(ConnectionStatusParseError::Empty);
    }

    match normalized.as_str() {
        "pending" => Ok(ConnectionStatus::Pending),
        "active" => Ok(ConnectionStatus::Active),
        "disconnected" => Ok(ConnectionStatus::Disconnected),
        "terminated" => Ok(ConnectionStatus::Terminated),
        "failed" => Ok(ConnectionStatus::Failed),
        _ => Err(ConnectionStatusParseError::Invalid(value.to_string())),
    }
}

pub fn parse_optional_connection_status(
    value: Option<String>,
) -> Result<Option<ConnectionStatus>, ConnectionStatusParseError> {
    match value {
        Some(raw) => parse_connection_status(&raw).map(Some),
        None => Ok(None),
    }
}
