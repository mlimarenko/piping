use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN tunnel termination domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnTunnelTermination {
    pub id: Uuid,
    pub role: String,
    pub tunnel_id: Uuid,
    pub tunnel_name: Option<String>,
    pub termination_type_id: Uuid,
    pub termination_id: Option<Uuid>,
    pub outside_ip_id: Option<Uuid>,
    pub outside_ip: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}
