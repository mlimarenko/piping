use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    connection_status::ConnectionStatus, connection_template::ConnectionProtocol,
    routing_source::RoutingSource,
};

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
