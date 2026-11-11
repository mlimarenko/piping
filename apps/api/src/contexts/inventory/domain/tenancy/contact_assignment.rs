use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// ContactAssignment domain model — NetBox `Tenancy > contact-assignments` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactAssignment {
    pub id: Uuid,
    pub external_id: Option<i64>,
    /// ContentType string, e.g. `dcim.site`.
    pub content_type: String,
    /// NetBox int id of the assigned object.
    pub object_id: i64,
    pub contact_id: Uuid,
    pub role_id: Option<Uuid>,
    pub priority: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}
