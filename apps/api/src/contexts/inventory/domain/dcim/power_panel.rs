use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Power panel domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PowerPanel {
    pub id: Uuid,
    pub name: String,
    pub site_id: Uuid,
    pub location_id: Option<Uuid>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
