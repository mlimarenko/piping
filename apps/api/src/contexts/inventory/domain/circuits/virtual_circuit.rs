use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Virtual circuit domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VirtualCircuit {
    pub id: Uuid,
    pub cid: String,
    pub status: String,
    pub provider_network_id: Uuid,
    pub type_id: Uuid,
    pub provider_account_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
