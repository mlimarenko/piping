use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Power outlet domain model — NetBox `DCIM > power outlets` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PowerOutlet {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// NetBox `type` field (e.g. `"iec-60320-c5"`, `"iec-60320-c13"`, …).
    /// Open-ended string.
    /// `None` when the outlet has no connector type set.
    pub type_: Option<String>,
    /// Feed leg (e.g. `"A"`, `"B"`, `"C"`); `None` when unset.
    pub feed_leg: Option<String>,
    /// Power port supplying this outlet, if any.
    pub power_port_id: Option<Uuid>,
    /// Whether the outlet is considered physically connected.
    pub mark_connected: bool,
    /// Cable attached to this outlet, if any.
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
