use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Service template domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceTemplate {
    pub id: Uuid,
    pub name: String,
    pub protocol: String,
    pub ports: Vec<i32>,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
