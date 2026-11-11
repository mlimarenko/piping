use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Virtual machine domain model — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtualMachine {
    pub id: Uuid,
    /// NetBox integer id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub cluster_id: Option<Uuid>,
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    /// Assigned virtual machine type; supplies defaults for unset fields.
    pub virtual_machine_type_id: Option<Uuid>,
    pub status: Option<String>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip6_id: Option<Uuid>,
    pub vcpus: Option<f64>,
    pub memory: Option<i32>,
    pub disk: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub config_template_id: Option<Uuid>,
    pub local_context_data: Option<serde_json::Value>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
    /// Reverse projection of associated piping connection profile ids.
    pub piping_connection_profile_ids: Vec<Uuid>,
}

/// Hydrated list row — includes cluster/tenant/role names via LEFT JOIN.
/// `primary_ip` is resolved by the handler from the existing IPAM service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualMachineListView {
    pub id: Uuid,
    pub name: String,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub cluster_id: Option<Uuid>,
    pub cluster_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub platform_id: Option<Uuid>,
    pub platform_name: Option<String>,
    pub virtual_machine_type_id: Option<Uuid>,
    pub virtual_machine_type_name: Option<String>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip6_id: Option<Uuid>,
    pub vcpus: Option<f64>,
    pub memory: Option<i32>,
    pub disk: Option<i32>,
}
