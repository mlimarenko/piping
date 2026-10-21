use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Provider account domain model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderAccount {
    pub id: Uuid,
    pub account: String,
    pub name: String,
    pub provider_id: Uuid,
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}
