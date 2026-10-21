use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Region domain model (geographic hierarchy for sites) — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Region {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}

/// Hydrated list row — includes the parent region name via a self-JOIN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionListView {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub parent_name: Option<String>,
    pub description: Option<String>,
}
