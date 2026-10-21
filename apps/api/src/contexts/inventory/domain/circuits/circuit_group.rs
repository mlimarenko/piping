use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Circuit group domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CircuitGroup {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub tenant_id: Option<Uuid>,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
