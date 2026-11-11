use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    connection_status::ConnectionStatus, connection_template::ConnectionProtocol,
    routing_source::RoutingSource,
};

/// Server-owned generation pin for the proxy runtime created for this session.
pub const RUNTIME_GENERATION_METADATA_KEY: &str = "_runtime_generation";

/// Overwrite any untrusted value with the generation returned by the proxy.
pub fn pin_runtime_generation(
    metadata: &mut Value,
    generation: Uuid,
) -> Result<(), crate::errors::AppError> {
    let map = metadata.as_object_mut().ok_or_else(|| {
        crate::errors::AppError::Validation("session_metadata must be an object".to_string())
    })?;
    map.insert(
        RUNTIME_GENERATION_METADATA_KEY.to_string(),
        Value::String(generation.to_string()),
    );
    Ok(())
}

impl ConnectionSession {
    pub fn runtime_generation(&self) -> Option<Uuid> {
        self.session_metadata
            .get(RUNTIME_GENERATION_METADATA_KEY)
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
    }

    pub fn matches_runtime_generation(&self, generation: Option<&str>) -> bool {
        self.runtime_generation()
            .zip(generation.and_then(|value| Uuid::parse_str(value).ok()))
            .is_some_and(|(pinned, actual)| pinned == actual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn proxy_generation_replaces_caller_metadata() {
        let actual = Uuid::now_v7();
        let mut metadata =
            json!({"_runtime_generation": Uuid::now_v7().to_string(), "entryId": "keep"});
        pin_runtime_generation(&mut metadata, actual).unwrap();
        assert_eq!(metadata["_runtime_generation"], actual.to_string());
        assert_eq!(metadata["entryId"], "keep");
        assert!(pin_runtime_generation(&mut json!("not an object"), actual).is_err());
    }
}

/// Connection session representing an active or historical session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionSession {
    /// Unique identifier (UUID)
    pub id: Uuid,
    /// Connect target identifier (Device/VM UUID)
    pub target_id: Uuid,
    /// User who initiated the session
    pub user_id: Uuid,
    /// Protocol in use
    pub protocol: ConnectionProtocol,
    /// Current session status
    pub status: ConnectionStatus,
    /// Proxy session identifier (if allocated)
    pub proxy_id: Option<Uuid>,
    /// Proxy node used for routing (if routed via external proxy)
    pub proxy_node_id: Option<Uuid>,
    /// Source of proxy routing decision
    pub routing_source: RoutingSource,
    /// Client IP address (if available)
    pub client_ip: Option<String>,
    /// Last observed activity timestamp
    pub last_activity: DateTime<Utc>,
    /// Session metadata payload
    pub session_metadata: Value,
    /// Session start timestamp
    pub started_at: DateTime<Utc>,
    /// Session end timestamp
    pub ended_at: Option<DateTime<Utc>>,
}
