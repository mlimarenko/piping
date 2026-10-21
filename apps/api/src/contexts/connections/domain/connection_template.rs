use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use uuid::Uuid;

/// Protocols supported for connection templates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionProtocol {
    App,
    Rdp,
    Ssh,
    Vnc,
    Cctv,
}

impl ConnectionProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionProtocol::App => "app",
            ConnectionProtocol::Rdp => "rdp",
            ConnectionProtocol::Ssh => "ssh",
            ConnectionProtocol::Vnc => "vnc",
            ConnectionProtocol::Cctv => "cctv",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        parse_connection_protocol(value).ok()
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConnectionProtocolParseError {
    #[error("connection protocol cannot be empty")]
    Empty,
    #[error("invalid connection protocol: {0}")]
    Invalid(String),
}

pub fn parse_connection_protocol(
    value: &str,
) -> Result<ConnectionProtocol, ConnectionProtocolParseError> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(ConnectionProtocolParseError::Empty);
    }

    match normalized.as_str() {
        "app" => Ok(ConnectionProtocol::App),
        "rdp" => Ok(ConnectionProtocol::Rdp),
        "ssh" => Ok(ConnectionProtocol::Ssh),
        "vnc" => Ok(ConnectionProtocol::Vnc),
        "cctv" => Ok(ConnectionProtocol::Cctv),
        _ => Err(ConnectionProtocolParseError::Invalid(value.to_string())),
    }
}

pub fn parse_optional_connection_protocol(
    value: Option<String>,
) -> Result<Option<ConnectionProtocol>, ConnectionProtocolParseError> {
    match value {
        Some(raw) => parse_connection_protocol(&raw).map(Some),
        None => Ok(None),
    }
}

/// Canonical launch-params object used by connection session create flow.
pub type ConnectionLaunchParams = Map<String, Value>;

/// Connection template definition with protocol defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionTemplate {
    /// Unique identifier (UUID)
    pub id: Uuid,
    /// Template display name
    pub name: String,
    /// Protocol identifier
    pub protocol: ConnectionProtocol,
    /// Default connection parameters in JSON form
    pub defaults: Value,
}
