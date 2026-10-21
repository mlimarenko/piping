use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Schema describing the attributes of a module type.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ModuleTypeProfile {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub comments: String,
    pub schema: Option<serde_json::Value>,
    pub owner_id: Option<Uuid>,
}
