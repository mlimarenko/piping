use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical request shape for a polymorphic target across the
/// inventory entities that own one (MAC, service, contact assignment, tunnel
/// termination, VLAN group, inventory item).
///
/// Carries only `{ object_type: "app_label.model", object_id }` —
/// response-only `label` is rejected, so a client that posts back a response
/// `ObjectRef` fails loudly. The `object_type` token is validated against the
/// entity's allowed content types and resolved to a content-type id
/// server-side.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectRefInput {
    /// NetBox-style `app_label.model` content-type token (e.g. `"dcim.interface"`).
    pub object_type: String,
    /// Target row id.
    pub object_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct PaginationQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Optional free-text search. Applied as a case-insensitive substring match
    /// on the entity's primary text fields. An empty or whitespace-only value
    /// is treated as no filter.
    pub search: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TenantGroupPayload {
    pub name: String,
    pub slug: Option<String>,
    pub parent_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TenantPayload {
    pub name: String,
    pub slug: Option<String>,
    pub group_id: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SitePayload {
    pub name: String,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<String>,
    pub region_id: Option<String>,
    pub site_group_id: Option<String>,
    pub facility: Option<String>,
    pub time_zone: Option<String>,
    pub physical_address: Option<String>,
    pub shipping_address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ClusterPayload {
    pub name: String,
    pub site_id: Option<String>,
    pub tenant_id: Option<String>,
    pub cluster_type_id: Option<String>,
    pub cluster_group_id: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RegionPayload {
    pub name: String,
    pub slug: String,
    pub parent_id: Option<String>,
    pub description: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SiteGroupPayload {
    pub name: String,
    pub slug: String,
    pub parent_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ClusterTypePayload {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ClusterGroupPayload {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ManufacturerPayload {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeviceTypePayload {
    pub manufacturer_id: String,
    pub model: String,
    pub slug: String,
    pub description: Option<String>,
    pub comments: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeviceRolePayload {
    pub name: String,
    pub slug: Option<String>,
    pub color: Option<String>,
    pub vm_role: Option<bool>,
    pub description: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DevicePayload {
    pub name: String,
    pub site_id: Option<String>,
    pub tenant_id: Option<String>,
    pub role_id: Option<String>,
    pub device_type_id: Option<String>,
    pub platform_id: Option<String>,
    pub rack_id: Option<String>,
    pub status: Option<String>,
    pub primary_ip: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub position: Option<f64>,
    pub face: Option<String>,
    pub airflow: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
    // dynamic: arbitrary NetBox-style local config-context data merged over inherited contexts
    pub local_context_data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualMachinePayload {
    pub name: String,
    pub site_id: Option<String>,
    pub cluster_id: Option<String>,
    pub tenant_id: Option<String>,
    pub role_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub platform_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub virtual_machine_type_id: Option<Option<Uuid>>,
    pub status: Option<String>,
    pub primary_ip: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    #[schema(minimum = 0.01, maximum = 9999.99, multiple_of = 0.01)]
    pub vcpus: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    #[schema(minimum = 0)]
    pub memory: Option<Option<i32>>,
    pub disk: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
    // dynamic: arbitrary NetBox-style local config-context data merged over inherited contexts
    pub local_context_data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkCreateRequest<T> {
    pub items: Vec<T>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkCreateError {
    /// 1-based row position in the original `items` array. Errors for the
    /// 0th and 2nd rows surface as `index = 1` and `index = 3` respectively.
    pub index: u32,
    /// Human-readable hint identifying the row by its natural key (e.g. the
    /// `name`, `slug`, or `vid` of the item that failed). Combined with
    /// `index` so duplicate-key errors don't collide on a shared natural key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    pub error: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkCreateResponse<T> {
    pub created: Vec<T>,
    pub errors: Vec<BulkCreateError>,
}

/// Canonical envelope for inventory list endpoints. `limit` and `offset` are
/// the resolved (clamped) values the server actually applied, so a client knows
/// exactly how it was paged.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InventoryListEnvelope<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Hydrated device list row returned by `GET /api/v1/inventory/devices`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceListRowResponse {
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
    pub primary_ip: Option<String>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub position: Option<f64>,
    pub face: Option<String>,
    pub airflow: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VirtualMachineListRowResponse {
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
    pub primary_ip: Option<String>,
    pub vcpus: Option<f64>,
    pub memory: Option<i32>,
    pub disk: Option<i32>,
}

/// Hydrated device type list row returned by `GET /api/v1/inventory/device_types`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceTypeListRowResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub manufacturer_id: Uuid,
    pub manufacturer_name: Option<String>,
    pub model: String,
    pub slug: String,
    pub default_platform_id: Option<Uuid>,
    pub part_number: Option<String>,
    pub u_height: f64,
    pub is_full_depth: bool,
    pub subdevice_role: Option<String>,
    pub airflow: Option<String>,
    pub weight: Option<f64>,
    pub weight_unit: Option<String>,
    pub front_image: Option<String>,
    pub rear_image: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Hydrated cluster list row returned by `GET /api/v1/inventory/clusters`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ClusterListRowResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub cluster_type_id: Option<Uuid>,
    pub cluster_type_name: Option<String>,
    pub cluster_group_id: Option<Uuid>,
    pub cluster_group_name: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SiteListRowResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub region_id: Option<Uuid>,
    pub region_name: Option<String>,
    pub site_group_id: Option<Uuid>,
    pub site_group_name: Option<String>,
    pub facility: Option<String>,
    pub time_zone: Option<String>,
    pub physical_address: Option<String>,
    pub shipping_address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TenantListRowResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub group_id: Option<Uuid>,
    pub group_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegionListRowResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub parent_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceResponse {
    pub id: Uuid,
    pub name: String,
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub device_type_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    pub rack_id: Option<Uuid>,
    pub status: Option<String>,
    pub primary_ip: Option<String>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip6_id: Option<Uuid>,
    pub serial: Option<String>,
    pub asset_tag: Option<String>,
    pub position: Option<f64>,
    pub face: Option<String>,
    pub airflow: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
    // dynamic: arbitrary NetBox-style local config-context data merged over inherited contexts
    pub local_context_data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VirtualMachineResponse {
    pub id: Uuid,
    pub name: String,
    pub site_id: Option<Uuid>,
    pub cluster_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    pub virtual_machine_type_id: Option<Uuid>,
    pub status: Option<String>,
    pub primary_ip: Option<String>,
    pub primary_ip4_id: Option<Uuid>,
    pub primary_ip6_id: Option<Uuid>,
    pub vcpus: Option<f64>,
    pub memory: Option<i32>,
    pub disk: Option<i32>,
    pub description: Option<String>,
    pub comments: Option<String>,
    // dynamic: per-object custom field name->value map (definition+value, secrets masked)
    pub custom_fields: Option<serde_json::Value>,
    // dynamic: arbitrary NetBox-style local config-context data merged over inherited contexts
    pub local_context_data: Option<serde_json::Value>,
}

/// One config context that contributed to a rendered config context, in
/// resolution order (lowest precedence first, highest precedence last).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RenderedConfigContextSourceResponse {
    pub context_id: Uuid,
    pub context_name: String,
    pub weight: i32,
}

/// Rendered config context for a device or virtual machine. `rendered` is the
/// merged inherited configuration with the entity's `local_context_data`
/// applied as the highest-priority layer (NetBox semantics); connection
/// credentials are redacted from it. `local_context_data` echoes the stored
/// local override, and `sources` lists the contributing contexts in resolution
/// order.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RenderedConfigContextResponse {
    // dynamic: merged config context (inherited contexts + local_context_data on top, secrets redacted)
    pub rendered: serde_json::Value,
    // dynamic: the entity's stored local config-context override (arbitrary JSON)
    pub local_context_data: Option<serde_json::Value>,
    pub sources: Vec<RenderedConfigContextSourceResponse>,
}
