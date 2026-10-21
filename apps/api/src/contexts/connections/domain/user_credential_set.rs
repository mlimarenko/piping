use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::ConnectionProtocol;

#[derive(Debug, Clone)]
pub struct UserCredentialSet {
    pub id: Uuid,
    pub user_id: Uuid,
    pub protocol: ConnectionProtocol,
    pub name: String,
    pub params: Value,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
