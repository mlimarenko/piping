use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Group domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Unique identifier (UUID)
    pub id: Uuid,

    /// Group name
    pub name: String,

    /// External identifier from auth provider (optional)
    pub external_id: Option<String>,
}
