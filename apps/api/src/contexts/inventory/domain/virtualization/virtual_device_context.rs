use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Virtual device context domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VirtualDeviceContext {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub identifier: Option<i16>,
    pub device_id: Option<Uuid>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip4: Option<String>,
    pub primary_ip6_id: Option<Uuid>,
    pub primary_ip6: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
