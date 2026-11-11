use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN IPSec profile domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnIpsecProfile {
    pub id: Uuid,
    pub name: String,
    pub mode: String,
    pub ike_policy_id: Uuid,
    pub ipsec_policy_id: Uuid,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
