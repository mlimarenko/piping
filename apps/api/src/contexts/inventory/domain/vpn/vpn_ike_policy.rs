use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN IKE policy domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnIkePolicy {
    pub id: Uuid,
    pub name: String,
    pub version: i16,
    pub mode: Option<String>,
    pub preshared_key: String,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
