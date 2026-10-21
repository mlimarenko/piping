use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Tenant domain model — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Tenant {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: Option<String>,
    pub group_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}

/// Hydrated list row returned by the canonical list endpoint — includes
/// the parent tenant-group name via LEFT JOIN so the UI does not need
/// a secondary lookup call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantListView {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub group_id: Option<Uuid>,
    pub group_name: Option<String>,
    pub description: Option<String>,
}
