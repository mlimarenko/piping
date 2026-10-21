use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN IPSec policy domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnIpsecPolicy {
    pub id: Uuid,
    pub name: String,
    pub pfs_group: Option<i16>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
