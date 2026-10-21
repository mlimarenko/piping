use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Rack domain model — NetBox `DCIM > racks` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rack {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub site_id: Uuid,
    pub status: RackStatus,
    pub facility_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub rack_role_id: Option<i64>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub type_: Option<RackType>,
    pub width: Option<i32>,
    pub u_height: Option<i32>,
    pub desc_units: bool,
    pub outer_width: Option<i32>,
    pub outer_depth: Option<i32>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RackStatus {
    Active,
    Planned,
    Reserved,
    Available,
    Deprecated,
}

impl RackStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Planned => "planned",
            Self::Reserved => "reserved",
            Self::Available => "available",
            Self::Deprecated => "deprecated",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RackType(pub String);

impl RackType {
    pub fn new<S: Into<String>>(value: S) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
