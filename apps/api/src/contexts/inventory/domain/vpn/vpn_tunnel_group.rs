use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN tunnel group domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnTunnelGroup {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
