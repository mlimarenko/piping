use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::access_control::domain::{Action, ResourceType};

/// Policy domain model.
/// Timestamps: creation from UUIDv7 (id), changes from audit events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Unique identifier (UUIDv7)
    pub id: Uuid,

    /// Policy name
    pub name: String,

    /// Human-readable description (optional)
    pub description: Option<String>,

    /// Policy effect (allow or deny)
    pub effect: PolicyEffect,

    /// List of actions this policy applies to
    pub actions: Vec<Action>,

    /// List of resource types this policy applies to
    pub resource_types: Vec<ResourceType>,

    /// Policy constraints as JSON (optional)
    pub constraints: Option<Value>,

    /// Subjects this policy applies to
    pub subjects: Vec<String>,

    /// Resource identifiers this policy applies to
    pub resources: Vec<String>,

    /// Whether this policy is enabled
    pub enabled: bool,
}

/// Policy effect enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEffect {
    /// Allow access
    Allow,
    /// Deny access
    Deny,
}
