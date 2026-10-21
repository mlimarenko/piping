use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical inventory tag definition.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Tag {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: String,
    pub description: String,
    pub weight: i32,
}
