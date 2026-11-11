use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// ConfigContext binding to a scope
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigContextBinding {
    pub id: Uuid,
    pub context_id: Uuid,
    pub scope_type: String,
    pub scope_id: Uuid,
}
