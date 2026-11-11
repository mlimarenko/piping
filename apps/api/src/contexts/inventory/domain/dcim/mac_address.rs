use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// MAC address domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MacAddress {
    pub id: Uuid,
    pub mac_address: String,
    pub assigned_object_type_id: Option<Uuid>,
    pub assigned_object_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
