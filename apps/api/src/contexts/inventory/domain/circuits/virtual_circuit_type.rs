use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Virtual circuit type domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VirtualCircuitType {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: String,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
