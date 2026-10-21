use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Circuit termination domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CircuitTermination {
    pub id: Uuid,
    pub term_side: String,
    pub circuit_id: Uuid,
    pub xconnect_id: String,
    pub pp_info: String,
    pub port_speed: Option<i32>,
    pub upstream_speed: Option<i32>,
    pub mark_connected: bool,
    pub description: String,
    pub custom_fields: Option<serde_json::Value>,
}
