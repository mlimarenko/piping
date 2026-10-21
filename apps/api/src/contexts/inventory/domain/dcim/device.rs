use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Device domain model — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Device {
    pub id: Uuid,
    /// NetBox integer id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub name: String,
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub device_type_id: Option<Uuid>,
    pub status: Option<String>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip6_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    pub rack_id: Option<Uuid>,
    pub cluster_id: Option<Uuid>,
    pub virtual_chassis_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub position: Option<f64>,
    pub face: Option<String>,
    pub vc_position: Option<i32>,
    pub vc_priority: Option<i16>,
    pub airflow: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub config_template_id: Option<Uuid>,
    pub local_context_data: Option<serde_json::Value>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
    /// Reverse projection of associated piping connection profile ids.
    /// Populated by the connection-context join service when applicable.
    pub piping_connection_profile_ids: Vec<Uuid>,
}

/// Hydrated list row — includes site/tenant/role/device-type names via
/// LEFT JOIN. `primary_ip` is resolved by the handler using the existing
/// `IpamService::map_ip_addresses` path to keep IP-resolution canonical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceListView {
    pub id: Uuid,
    pub name: String,
    pub status: Option<String>,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub device_type_id: Option<Uuid>,
    pub device_type_name: Option<String>,
    pub platform_id: Option<Uuid>,
    pub platform_name: Option<String>,
    pub rack_id: Option<Uuid>,
    pub rack_name: Option<String>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip6_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub position: Option<f64>,
    pub face: Option<String>,
    pub airflow: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}
