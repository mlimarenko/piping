use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Module domain model — NetBox `DCIM > modules` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Module {
    pub id: Uuid,
    /// Device this module is installed in.
    pub device_id: Uuid,
    /// Module bay that houses this module.
    pub module_bay_id: Uuid,
    /// Module type (manufacturer model) for this module.
    pub module_type_id: Uuid,
    /// NetBox status string (e.g. `"active"`, `"planned"`, `"failed"`).
    pub status: String,
    pub serial: String,
    /// Globally-unique asset tag; `None` when not assigned.
    pub asset_tag: Option<String>,
    pub description: String,
    pub custom_fields: Option<serde_json::Value>,
}
