use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// NetBox owner group: flat, not a tree and not tagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct OwnerGroup {
    pub id: Uuid,
    pub name: String,
    pub description: String,
}
