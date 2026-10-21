use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// NetBox owner: no tags, custom fields or change-date fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Owner {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub group_id: Option<Uuid>,
    pub users: Vec<Uuid>,
    pub user_groups: Vec<Uuid>,
}
