use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Secret binding to a scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretBinding {
    pub id: Uuid,
    pub secret_id: Uuid,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub weight: i32,
    /// Internal authority to detach a scope binding with its last field usage.
    #[serde(skip)]
    pub managed_by_field_usage: bool,
}
