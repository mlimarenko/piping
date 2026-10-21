use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Role domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    /// Unique identifier (UUID)
    pub id: Uuid,

    /// Role name
    pub name: String,

    /// Role description (optional)
    pub description: Option<String>,
}
