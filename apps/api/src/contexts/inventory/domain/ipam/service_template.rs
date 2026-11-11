use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Service template domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceTemplate {
    pub id: Uuid,
    pub name: String,
    pub port_mappings: Vec<String>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
