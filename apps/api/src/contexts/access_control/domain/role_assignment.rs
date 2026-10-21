use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Role assignment domain model (user or group to role).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleAssignment {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub role_id: Uuid,
}
