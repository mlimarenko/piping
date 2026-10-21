use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN tunnel domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnTunnel {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub encapsulation: String,
    pub description: String,
    pub comments: String,
    pub group_id: Option<Uuid>,
    pub ipsec_profile_id: Option<Uuid>,
    pub tunnel_id: Option<i64>,
    pub tenant_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
