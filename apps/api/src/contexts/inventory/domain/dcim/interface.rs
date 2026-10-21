use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Interface (DCIM) domain model — NetBox `DCIM > interfaces` 1:1 mirror.
///
/// NOTE: a thinner `InterfaceRecord` exists in
/// `infrastructure/persistence/repositories/dcim/interface_repository.rs` for a
/// minimal REST surface. Both are retained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interface {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub device_id: Uuid,
    pub name: String,
    /// `type` in NetBox; uses trailing underscore to avoid Rust keyword clash.
    pub type_: InterfaceType,
    pub enabled: bool,
    pub mac_address: Option<String>,
    pub mtu: Option<i32>,
    pub mode: Option<String>,
    pub mgmt_only: bool,
    pub speed: Option<i64>,
    pub description: Option<String>,
    pub cable_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}

/// Subset of NetBox `dcim.interface` `type` choices. Stored as a string in DB
/// to match NetBox's open-ended enum and to avoid migrations on each new
/// upstream value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterfaceType(pub String);

impl InterfaceType {
    pub fn new<S: Into<String>>(value: S) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
