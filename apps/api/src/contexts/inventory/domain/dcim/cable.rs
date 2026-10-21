use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Cable domain model — NetBox `DCIM > cables` 1:1 mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cable {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub type_: Option<CableType>,
    pub status: CableStatus,
    pub label: Option<String>,
    pub color: Option<String>,
    pub length: Option<f64>,
    pub length_unit: Option<String>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CableType(pub String);

impl CableType {
    pub fn new<S: Into<String>>(value: S) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CableStatus {
    Connected,
    Planned,
    Decommissioned,
}

impl CableStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Planned => "planned",
            Self::Decommissioned => "decommissioned",
        }
    }
}
