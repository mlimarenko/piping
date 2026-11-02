use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// User domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Unique identifier (UUID)
    pub id: Uuid,

    /// Unique username
    pub username: String,

    /// Unique email address
    pub email: String,

    /// User status (e.g., "active", "inactive", "suspended")
    pub status: String,

    /// Authentication source (e.g., "local", "oidc", "ldap")
    pub auth_source: String,

    /// External identifier from auth provider (optional)
    pub external_id: Option<String>,
}

impl User {
    /// The single account-activity rule every authentication boundary applies,
    /// including the deferred commit boundaries of sync work that outlive the
    /// request which started it.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    /// Validates that the email is non-empty
    pub fn validate_email(email: &str) -> Result<(), String> {
        if email.trim().is_empty() {
            return Err("Email cannot be empty".to_string());
        }
        Ok(())
    }

    /// Validates the user's email
    pub fn validate(&self) -> Result<(), String> {
        Self::validate_email(&self.email)
    }
}
