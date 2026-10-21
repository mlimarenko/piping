use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Platform domain model — NetBox `DCIM > platforms` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub manufacturer_id: Option<Uuid>,
    pub napalm_driver: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}
