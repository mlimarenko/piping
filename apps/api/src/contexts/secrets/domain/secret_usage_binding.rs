use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical binding between a consumer field and a stored secret value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretUsageBinding {
    pub id: Uuid,
    pub consumer_type: String,
    pub consumer_id: Uuid,
    pub field_key: String,
    pub owner_scope_type: String,
    pub owner_scope_id: Uuid,
    pub secret_id: Uuid,
}
