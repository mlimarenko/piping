use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Location domain model — NetBox `DCIM > locations` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Location {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: String,
    pub site_id: Uuid,
    pub parent_id: Option<Uuid>,
    /// NetBox `LocationStatusChoices` (e.g. `active`, `planned`, `staging`, `decommissioning`, `retired`).
    pub status: String,
    pub tenant_id: Option<Uuid>,
    pub facility: Option<String>,
    pub description: Option<String>,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}
