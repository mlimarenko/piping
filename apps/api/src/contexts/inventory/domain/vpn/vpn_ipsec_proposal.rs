use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN IPSec proposal domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnIpsecProposal {
    pub id: Uuid,
    pub name: String,
    pub encryption_algorithm: Option<String>,
    pub authentication_algorithm: Option<String>,
    pub sa_lifetime_seconds: Option<i32>,
    pub sa_lifetime_data: Option<i32>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
