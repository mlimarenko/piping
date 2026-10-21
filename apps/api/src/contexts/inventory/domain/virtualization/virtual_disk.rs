use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VirtualDisk domain model — NetBox `Virtualization > virtual disks` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VirtualDisk {
    pub id: Uuid,
    pub virtual_machine_id: Uuid,
    pub name: String,
    /// Disk size in megabytes (non-negative).
    pub size: i32,
    pub description: Option<String>,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
