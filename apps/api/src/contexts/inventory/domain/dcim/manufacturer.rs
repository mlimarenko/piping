use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Manufacturer domain model (device type manufacturer) — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Manufacturer {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}
