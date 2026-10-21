use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// ContactGroup domain model — NetBox `Tenancy > contact-groups` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactGroup {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    /// Self-referential parent group.
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}
