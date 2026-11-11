use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Site domain model — full NetBox 1:1 surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Site {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub region_id: Option<Uuid>,
    pub site_group_id: Option<Uuid>,
    pub facility: Option<String>,
    pub time_zone: Option<String>,
    pub description: Option<String>,
    pub physical_address: Option<String>,
    pub shipping_address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub comments: Option<String>,
    /// NetBox `asns` field — flat list of ASN integer ids.
    pub asns: Vec<i64>,
    /// Flat tag list mirroring NetBox's `tags` polymorphic projection.
    pub tags: Vec<String>,
    /// NetBox `custom_fields`.
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}

/// Hydrated list row — includes tenant/region/site-group names via LEFT JOIN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteListView {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub region_id: Option<Uuid>,
    pub region_name: Option<String>,
    pub site_group_id: Option<Uuid>,
    pub site_group_name: Option<String>,
    pub facility: Option<String>,
    pub time_zone: Option<String>,
    pub description: Option<String>,
    pub physical_address: Option<String>,
    pub shipping_address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub custom_fields: Option<serde_json::Value>,
}
