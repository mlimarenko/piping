use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Provider domain model (circuit provider).
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Provider {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
