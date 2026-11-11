use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Contact domain model — NetBox `Tenancy > contacts` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub group_id: Option<Uuid>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}
