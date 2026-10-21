use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Prefix domain model — NetBox `IPAM > prefixes` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefix {
    pub id: Uuid,
    pub external_id: Option<i64>,
    /// CIDR notation (e.g. `10.0.0.0/24`).
    pub prefix: String,
    pub status: PrefixStatus,
    pub site_id: Option<Uuid>,
    pub vrf_id: Option<i64>,
    pub tenant_id: Option<Uuid>,
    pub vlan_id: Option<Uuid>,
    pub role_id: Option<i64>,
    pub is_pool: bool,
    pub mark_utilized: bool,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrefixStatus {
    Container,
    Active,
    Reserved,
    Deprecated,
}

impl PrefixStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Active => "active",
            Self::Reserved => "reserved",
            Self::Deprecated => "deprecated",
        }
    }
}
