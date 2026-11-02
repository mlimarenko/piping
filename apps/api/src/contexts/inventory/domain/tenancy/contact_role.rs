use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// ContactRole domain model — NetBox `Tenancy > contact-roles` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactRole {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}
