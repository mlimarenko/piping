use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Service domain model — NetBox `IPAM > Services` 1:1 mirror.
///
/// The parent link is polymorphic: `parent_object_type_id` identifies the
/// content-type (e.g. device, virtual machine) and `parent_object_id` is the UUID
/// of the parent record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Service {
    pub id: Uuid,
    pub name: String,
    pub protocol: String,
    /// TCP/UDP port numbers associated with the service.
    pub ports: Vec<i32>,
    /// UUID of the content-type entry identifying the parent object type.
    pub parent_object_type_id: Uuid,
    /// UUID of the parent object (device or virtual machine).
    pub parent_object_id: Uuid,
    pub description: Option<String>,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}
