use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A named bundle of cables; individual cables retain their own endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CableBundle {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub description: String,
    pub comments: String,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}
