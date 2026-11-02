use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Aggregate domain model — NetBox `IPAM > Aggregates` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Aggregate {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    /// CIDR aggregate prefix.
    pub prefix: String,
    /// Foreign key to the RIR.
    pub rir_id: Uuid,
    /// Optional foreign key to the tenant.
    pub tenant_id: Option<Uuid>,
    /// Date the aggregate was added (NetBox `date_added`).
    pub date_added: Option<NaiveDate>,
    pub description: Option<String>,
    pub comments: Option<String>,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}
