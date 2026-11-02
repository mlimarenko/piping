use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Rear port domain model — NetBox `DCIM > rear ports` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RearPort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// NetBox `type` field (e.g. `"8p8c"`, `"8p6c"`, `"110-punch"`, …).
    /// Open-ended string.
    pub type_: String,
    /// HTML hex color (6 characters, no `#`); empty string when unset.
    pub color: String,
    /// Number of front ports that can be mapped to this rear port.
    pub positions: i16,
    /// Whether the port is considered physically connected.
    pub mark_connected: bool,
    /// Cable attached to this port, if any.
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
