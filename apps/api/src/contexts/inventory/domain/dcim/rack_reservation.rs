use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Rack reservation domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RackReservation {
    pub id: Uuid,
    pub units: Vec<i16>,
    pub rack_id: Uuid,
    pub user_id: Uuid,
    pub user_name: Option<String>,
    pub status: String,
    pub tenant_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
