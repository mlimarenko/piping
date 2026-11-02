//! Canonical discovery host row.
//!
//! Data-carrying shape returned by the inventory discovery repository. It unifies
//! rows from `dcim_device` and `virtualization_virtualmachine` with joined
//! scope/tenant/role attributes, leaving monitoring-specific decoration
//! (monitoring overrides, tag materialization) to the service layer.

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DiscoveryInterfaceAddress {
    pub name: String,
    pub address: String,
    pub channel_address: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveryHostRow {
    pub host_id: Uuid,
    pub host_name: String,
    pub host_address: String,
    pub kind: String,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub site_slug: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub tenant_slug: Option<String>,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub role_slug: Option<String>,
    pub status: Option<String>,
    // Hardware metadata.
    pub platform_name: Option<String>,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    // Network metadata.
    pub primary_ip4: Option<String>,
    pub primary_ip6: Option<String>,
    pub interfaces: Option<String>,
    pub interface_addresses: Vec<DiscoveryInterfaceAddress>,
    pub tunnel_addresses: Vec<DiscoveryInterfaceAddress>,
    // Contacts (comma-joined "Role:Name <email>" entries).
    pub contacts: Option<String>,
    // Description / comments.
    pub description: Option<String>,
    pub comments: Option<String>,
    // Site geo.
    pub site_facility: Option<String>,
    pub site_address: Option<String>,
    pub site_region: Option<String>,
    pub site_group: Option<String>,
    pub tenant_group: Option<String>,
    pub rack: Option<String>,
    pub cluster: Option<String>,
    pub local_context_data: Option<Value>,
    pub custom_field_data: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryHostFilters {
    pub host_id: Option<Uuid>,
    pub kind: Option<String>,
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub status: Option<String>,
    pub site_status: Option<String>,
    pub allowed_statuses: Option<Vec<String>>,
    pub allowed_site_statuses: Option<Vec<String>>,
    pub search: Option<String>,
    pub fetch_limit: u32,
    pub fetch_offset: u32,
}
