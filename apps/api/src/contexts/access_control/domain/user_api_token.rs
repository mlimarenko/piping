use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{Action, ResourceType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserApiTokenStatus {
    Active,
    Revoked,
}

impl UserApiTokenStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserApiTokenScope {
    pub action: Action,
    pub resource_type: ResourceType,
    pub resource_pattern: String,
}

impl UserApiTokenScope {
    #[must_use]
    pub fn wire_value(&self) -> String {
        super::pat_scope_wire_value(&self.action, &self.resource_type)
    }
}

#[derive(Debug, Clone)]
pub struct UserApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub token_hash: String,
    pub status: UserApiTokenStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by_user_id: Option<Uuid>,
    pub scopes: Vec<UserApiTokenScope>,
}

impl UserApiToken {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|value| value <= now)
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.status == UserApiTokenStatus::Active && !self.is_expired(now)
    }
}
