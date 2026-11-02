use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Power port domain model — NetBox `DCIM > power ports` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PowerPort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// NetBox `type` field (e.g. `"iec-60320-c6"`, `"nema-5-15p"`, …).
    /// Open-ended string.
    /// `None` when the port has no connector type set.
    pub type_: Option<String>,
    /// Maximum power draw in watts; `None` when unset.
    pub maximum_draw: Option<i32>,
    /// Allocated power draw in watts; `None` when unset.
    pub allocated_draw: Option<i32>,
    /// Whether the port is considered physically connected.
    pub mark_connected: bool,
    /// Cable attached to this port, if any.
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
