use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Front port domain model — NetBox `DCIM > front ports` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FrontPort {
    pub id: Uuid,
    pub device_id: Uuid,
    pub module_id: Option<Uuid>,
    pub name: String,
    pub label: String,
    pub description: String,
    /// NetBox `type` field (e.g. `"8p8c"`, `"fc"`, `"lc"`, …).
    /// Open-ended string.
    pub type_: String,
    /// Six-character hex colour code (no leading `#`); empty string when unset.
    pub color: String,
    /// Number of rear-port positions this front port maps to.
    pub positions: i16,
    /// Whether the port is considered physically connected.
    pub mark_connected: bool,
    /// Cable attached to this port, if any.
    pub cable_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
