// Source-entity to target-entity link inside a
// MappingConfiguration version.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMapping {
    pub id: Uuid,
    pub mapping_config_logical_id: Uuid,
    pub mapping_config_version_id: i32,
    /// E.g. `netbox.dcim.device`.
    pub source_entity: String,
    /// E.g. `piping.inventory.device`.
    pub target_entity: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}
