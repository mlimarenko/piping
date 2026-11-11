use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VPN IKE proposal domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VpnIkeProposal {
    pub id: Uuid,
    pub name: String,
    pub authentication_method: String,
    pub encryption_algorithm: String,
    pub authentication_algorithm: Option<String>,
    pub group: i16,
    pub sa_lifetime: Option<i32>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
