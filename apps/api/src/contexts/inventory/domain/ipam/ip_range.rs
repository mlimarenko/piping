use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// IP range domain model — NetBox `IPAM > ip-ranges` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IpRange {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    /// Start of the range in inet notation (e.g. `192.168.1.1`).
    pub start_address: String,
    /// End of the range in inet notation (e.g. `192.168.1.254`).
    pub end_address: String,
    pub size: i32,
    pub status: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub vrf_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub mark_utilized: bool,
    pub mark_populated: bool,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}
