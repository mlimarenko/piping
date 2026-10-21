use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Power feed domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PowerFeed {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub r#type: String,
    pub supply: String,
    pub phase: String,
    pub voltage: i16,
    pub amperage: i16,
    pub max_utilization: i16,
    pub available_power: i32,
    pub power_panel_id: Uuid,
    pub rack_id: Option<Uuid>,
    pub mark_connected: bool,
    pub description: String,
    pub comments: String,
    pub tenant_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
