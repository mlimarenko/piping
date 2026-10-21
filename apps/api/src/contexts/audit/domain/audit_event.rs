use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Audit event domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier (UUID)
    pub id: Uuid,
    /// Event type (e.g., "connection.created")
    pub event_type: String,
    /// Actor identifier (user/service)
    pub actor_id: Option<Uuid>,
    /// Actor category (`user`, `service_principal`, `playbook_job_token`, etc.)
    pub actor_kind: String,
    /// Target type (e.g., "connection_session")
    pub target_type: Option<String>,
    /// Target identifier
    pub target_id: Option<Uuid>,
    /// Event metadata
    pub metadata: Value,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}
