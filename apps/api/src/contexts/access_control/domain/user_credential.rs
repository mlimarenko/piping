use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// User credential for local authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCredential {
    pub user_id: Uuid,
    pub password_hash: String,
    pub password_changed_at: DateTime<Utc>,
}
