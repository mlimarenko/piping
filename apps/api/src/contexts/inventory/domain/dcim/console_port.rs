use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Console port domain model — NetBox `DCIM > console ports` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConsolePort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// NetBox `type` field (e.g. `"de-9"`, `"db-25"`, `"rj-11"`, `"rj-45"`, …).
    /// Open-ended string.
    /// `None` when the port has no connector type set.
    pub type_: Option<String>,
    /// Port speed in kbps; `None` when unset.
    pub speed: Option<i32>,
    /// Whether the port is considered physically connected.
    pub mark_connected: bool,
    /// Cable attached to this port, if any.
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
