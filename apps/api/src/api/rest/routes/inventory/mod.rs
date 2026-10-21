//! Inventory REST route handlers.
//!
//! This module is intentionally kept thin: route handlers live in domain submodules
//! (`dcim/`, `ipam/`, `circuits/`, `virtualization/`, `tenancy/`, `vpn/`, `extras/`, `native/`).
//! Shared request/response DTOs live in `types` and shared parsing / bulk helpers live in `errors`.
//!
//! Handlers are re-exported from this module to keep router wiring and OpenAPI inclusion stable.

pub mod circuits;
pub mod dcim;
pub mod extras;
pub mod ipam;
pub mod native;
pub mod tenancy;
pub mod virtualization;
pub mod vpn;

use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use axum::response::IntoResponse;

pub mod errors;
pub mod types;

pub use circuits::circuit::{
    bulk_create_circuits, create_circuit, delete_circuit, get_circuit, list_circuits,
    update_circuit,
};
pub use circuits::circuit_groups::{
    bulk_create_circuit_groups, create_circuit_group, delete_circuit_group, get_circuit_group,
    list_circuit_groups, update_circuit_group,
};
pub use circuits::circuit_terminations::{
    bulk_create_circuit_terminations, create_circuit_termination, delete_circuit_termination,
    get_circuit_termination, list_circuit_terminations, update_circuit_termination,
};
pub use circuits::circuit_types::{
    bulk_create_circuit_types, create_circuit_type, delete_circuit_type, get_circuit_type,
    list_circuit_types, update_circuit_type,
};
pub use circuits::provider_accounts::{
    bulk_create_provider_accounts, create_provider_account, delete_provider_account,
    get_provider_account, list_provider_accounts, update_provider_account,
};
pub use circuits::provider_networks::{
    bulk_create_provider_networks, create_provider_network, delete_provider_network,
    get_provider_network, list_provider_networks, update_provider_network,
};
pub use circuits::providers::{
    bulk_create_providers, create_provider, delete_provider, get_provider, list_providers,
    update_provider,
};
pub use circuits::virtual_circuit_types::{
    bulk_create_virtual_circuit_types, create_virtual_circuit_type, delete_virtual_circuit_type,
    get_virtual_circuit_type, list_virtual_circuit_types, update_virtual_circuit_type,
};
pub use circuits::virtual_circuits::{
    bulk_create_virtual_circuits, create_virtual_circuit, delete_virtual_circuit,
    get_virtual_circuit, list_virtual_circuits, update_virtual_circuit,
};
pub use dcim::cables::{
    bulk_create_cables, create_cable, delete_cable, get_cable, list_cables, update_cable,
};
pub use dcim::cabling::{list_cabling_endpoints, trace_cabling_endpoint};
pub use dcim::console_ports::{
    bulk_create_console_ports, create_console_port, delete_console_port, get_console_port,
    list_console_ports, update_console_port,
};
pub use dcim::console_server_ports::{
    bulk_create_console_server_ports, create_console_server_port, delete_console_server_port,
    get_console_server_port, list_console_server_ports, update_console_server_port,
};
pub use dcim::device_bays::{
    bulk_create_device_bays, create_device_bay, delete_device_bay, get_device_bay,
    list_device_bays, update_device_bay,
};
pub use dcim::device_roles::{
    bulk_create_device_roles, create_device_role, delete_device_role, get_device_role,
    list_device_roles, update_device_role,
};
pub use dcim::device_types::{
    bulk_create_device_types, create_device_type, delete_device_type, get_device_type,
    list_device_types, update_device_type,
};
pub use dcim::devices::{
    bulk_create_devices, create_device, delete_device, get_device, get_device_config_context,
    list_devices, update_device,
};
pub use dcim::front_ports::{
    bulk_create_front_ports, create_front_port, delete_front_port, get_front_port,
    list_front_ports, update_front_port,
};
pub use dcim::inventory_item_roles::{
    bulk_create_inventory_item_roles, create_inventory_item_role, delete_inventory_item_role,
    get_inventory_item_role, list_inventory_item_roles, update_inventory_item_role,
};
pub use dcim::inventory_items::{
    bulk_create_inventory_items, create_inventory_item, delete_inventory_item, get_inventory_item,
    list_inventory_items, update_inventory_item,
};
pub use dcim::locations::{
    bulk_create_locations, create_location, delete_location, get_location, list_locations,
    update_location,
};
pub use dcim::mac_addresses::{
    bulk_create_mac_addresses, create_mac_address, delete_mac_address, get_mac_address,
    list_mac_addresses, update_mac_address,
};
pub use dcim::manufacturers::{
    bulk_create_manufacturers, create_manufacturer, delete_manufacturer, get_manufacturer,
    list_manufacturers, update_manufacturer,
};
pub use dcim::module_bays::{
    bulk_create_module_bays, create_module_bay, delete_module_bay, get_module_bay,
    list_module_bays, update_module_bay,
};
pub use dcim::module_types::{
    bulk_create_module_types, create_module_type, delete_module_type, get_module_type,
    list_module_types, update_module_type,
};
pub use dcim::modules::{
    bulk_create_modules, create_module, delete_module, get_module, list_modules, update_module,
};
pub use dcim::platforms::{
    bulk_create_platforms, create_platform, delete_platform, get_platform, list_platforms,
    update_platform,
};
pub use dcim::power_feeds::{
    bulk_create_power_feeds, create_power_feed, delete_power_feed, get_power_feed,
    list_power_feeds, update_power_feed,
};
pub use dcim::power_outlets::{
    bulk_create_power_outlets, create_power_outlet, delete_power_outlet, get_power_outlet,
    list_power_outlets, update_power_outlet,
};
pub use dcim::power_panels::{
    bulk_create_power_panels, create_power_panel, delete_power_panel, get_power_panel,
    list_power_panels, update_power_panel,
};
pub use dcim::power_ports::{
    bulk_create_power_ports, create_power_port, delete_power_port, get_power_port,
    list_power_ports, update_power_port,
};
pub use dcim::rack_reservations::{
    bulk_create_rack_reservations, create_rack_reservation, delete_rack_reservation,
    get_rack_reservation, list_rack_reservations, update_rack_reservation,
};
pub use dcim::rack_roles::{
    bulk_create_rack_roles, create_rack_role, delete_rack_role, get_rack_role, list_rack_roles,
    update_rack_role,
};
pub use dcim::rack_types::{
    bulk_create_rack_types, create_rack_type, delete_rack_type, get_rack_type, list_rack_types,
    update_rack_type,
};
pub use dcim::racks::{
    bulk_create_racks, create_rack, delete_rack, get_rack, list_racks, update_rack,
};
pub use dcim::rear_ports::{
    bulk_create_rear_ports, create_rear_port, delete_rear_port, get_rear_port, list_rear_ports,
    update_rear_port,
};
pub use dcim::regions::{
    bulk_create_regions, create_region, delete_region, get_region, list_regions, update_region,
};
pub use dcim::site_groups::{
    bulk_create_site_groups, create_site_group, delete_site_group, get_site_group,
    list_site_groups, update_site_group,
};
pub use dcim::sites::{
    bulk_create_sites, create_site, delete_site, get_site, list_sites, update_site,
};
pub use dcim::virtual_chassis::{
    bulk_create_virtual_chassis, create_virtual_chassis, delete_virtual_chassis,
    get_virtual_chassis, list_virtual_chassis, update_virtual_chassis,
};
use errors::{
    parse_optional_uuid, parse_required_string, parse_uuid, push_bulk_error,
    push_bulk_error_for_indices, push_bulk_error_with_hint, require_bulk_items,
    resolve_object_type,
};
pub use ipam::aggregates::{
    bulk_create_aggregates, create_aggregate, delete_aggregate, get_aggregate, list_aggregates,
    update_aggregate,
};
pub use ipam::asn_ranges::{
    bulk_create_asn_ranges, create_asn_range, delete_asn_range, get_asn_range, list_asn_ranges,
    update_asn_range,
};
pub use ipam::asns::{bulk_create_asns, create_asn, delete_asn, get_asn, list_asns, update_asn};
pub use ipam::ip_ranges::{
    bulk_create_ip_ranges, create_ip_range, delete_ip_range, get_ip_range, list_ip_ranges,
    update_ip_range,
};
pub use ipam::ipam_roles::{
    bulk_create_ipam_roles, create_ipam_role, delete_ipam_role, get_ipam_role, list_ipam_roles,
    update_ipam_role,
};
pub use ipam::prefixes::{
    bulk_create_prefixes, create_prefix, delete_prefix, get_prefix, list_prefix_tree,
    list_prefixes, preview_next_available_prefix, update_prefix,
};
pub use ipam::rirs::{bulk_create_rirs, create_rir, delete_rir, get_rir, list_rirs, update_rir};
pub use ipam::route_targets::{
    bulk_create_route_targets, create_route_target, delete_route_target, get_route_target,
    list_route_targets, update_route_target,
};
pub use ipam::service_templates::{
    bulk_create_service_templates, create_service_template, delete_service_template,
    get_service_template, list_service_templates, update_service_template,
};
pub use ipam::services::{
    bulk_create_services, create_service, delete_service, get_service, list_services,
    update_service,
};
pub use ipam::vlan_groups::{
    bulk_create_vlan_groups, create_vlan_group, delete_vlan_group, get_vlan_group,
    list_vlan_groups, update_vlan_group,
};
pub use ipam::vlans::{
    bulk_create_vlans, create_vlan, delete_vlan, get_vlan, list_vlans, update_vlan,
};
pub use ipam::vrfs::{bulk_create_vrfs, create_vrf, delete_vrf, get_vrf, list_vrfs, update_vrf};
pub use native::collected_facts::ingest_collected_facts;
pub use native::import_export::{export_full_backup, import_full_backup};
pub use native::ipam::{
    bulk_create_interfaces, create_interface, create_ip_address, delete_interface,
    delete_ip_address, get_interface, get_ip_address, list_interfaces, list_ip_addresses,
    update_interface, update_ip_address,
};
pub use native::layouts::{
    delete_user_layout, get_system_layout, get_user_layout, list_system_layouts,
    upsert_system_layout, upsert_user_layout,
};
pub use native::resources_bulk::execute_resource_bulk_operation;
pub use native::templates::{
    create_inventory_template, create_inventory_template_run, delete_inventory_template,
    get_inventory_template, get_inventory_template_run, list_inventory_template_run_steps,
    list_inventory_template_runs, list_inventory_template_runs_global, list_inventory_templates,
    preview_inventory_template_run, update_inventory_template,
};
pub use tenancy::contact_assignments::{
    bulk_create_contact_assignments, create_contact_assignment, delete_contact_assignment,
    get_contact_assignment, list_contact_assignments, update_contact_assignment,
};
pub use tenancy::contact_groups::{
    bulk_create_contact_groups, create_contact_group, delete_contact_group, get_contact_group,
    list_contact_groups, update_contact_group,
};
pub use tenancy::contact_roles::{
    bulk_create_contact_roles, create_contact_role, delete_contact_role, get_contact_role,
    list_contact_roles, update_contact_role,
};
pub use tenancy::contacts::{
    bulk_create_contacts, create_contact, delete_contact, get_contact, list_contacts,
    update_contact,
};
pub use tenancy::tenant_groups::{
    bulk_create_tenant_groups, create_tenant_group, delete_tenant_group, get_tenant_group,
    list_tenant_groups, update_tenant_group,
};
pub use tenancy::tenants::{
    bulk_create_tenants, create_tenant, delete_tenant, get_tenant, list_tenants, update_tenant,
};
pub use types::*;
pub use virtualization::cluster_groups::{
    bulk_create_cluster_groups, create_cluster_group, delete_cluster_group, get_cluster_group,
    list_cluster_groups, update_cluster_group,
};
pub use virtualization::cluster_types::{
    bulk_create_cluster_types, create_cluster_type, delete_cluster_type, get_cluster_type,
    list_cluster_types, update_cluster_type,
};
pub use virtualization::clusters::{
    bulk_create_clusters, create_cluster, delete_cluster, get_cluster, list_clusters,
    update_cluster,
};
pub use virtualization::virtual_device_contexts::{
    bulk_create_virtual_device_contexts, create_virtual_device_context,
    delete_virtual_device_context, get_virtual_device_context, list_virtual_device_contexts,
    update_virtual_device_context,
};
pub use virtualization::virtual_disks::{
    bulk_create_virtual_disks, create_virtual_disk, delete_virtual_disk, get_virtual_disk,
    list_virtual_disks, update_virtual_disk,
};
pub use virtualization::virtual_machine_types::{
    bulk_create_virtual_machine_types, create_virtual_machine_type, delete_virtual_machine_type,
    get_virtual_machine_type, list_virtual_machine_types, patch_virtual_machine_type,
    update_virtual_machine_type,
};
pub use virtualization::virtual_machines::{
    bulk_create_virtual_machines, create_virtual_machine, delete_virtual_machine,
    get_virtual_machine, get_virtual_machine_config_context, list_virtual_machines,
    update_virtual_machine,
};
pub use virtualization::vm_interfaces::{
    bulk_create_vm_interfaces, create_vm_interface, delete_vm_interface, get_vm_interface,
    list_vm_interfaces, update_vm_interface,
};
pub use vpn::ike_policies::{
    bulk_create_ike_policies, create_ike_policy, delete_ike_policy, get_ike_policy,
    list_ike_policies, update_ike_policy,
};
pub use vpn::ike_proposals::{
    bulk_create_ike_proposals, create_ike_proposal, delete_ike_proposal, get_ike_proposal,
    list_ike_proposals, update_ike_proposal,
};
pub use vpn::ipsec_policies::{
    bulk_create_ipsec_policies, create_ipsec_policy, delete_ipsec_policy, get_ipsec_policy,
    list_ipsec_policies, update_ipsec_policy,
};
pub use vpn::ipsec_profiles::{
    bulk_create_ipsec_profiles, create_ipsec_profile, delete_ipsec_profile, get_ipsec_profile,
    list_ipsec_profiles, update_ipsec_profile,
};
pub use vpn::ipsec_proposals::{
    bulk_create_ipsec_proposals, create_ipsec_proposal, delete_ipsec_proposal, get_ipsec_proposal,
    list_ipsec_proposals, update_ipsec_proposal,
};
pub use vpn::tunnel_groups::{
    bulk_create_tunnel_groups, create_tunnel_group, delete_tunnel_group, get_tunnel_group,
    list_tunnel_groups, update_tunnel_group,
};
pub use vpn::tunnel_terminations::{
    bulk_create_tunnel_terminations, create_tunnel_termination, delete_tunnel_termination,
    get_tunnel_termination, list_tunnel_terminations, update_tunnel_termination,
};
pub use vpn::tunnels::{
    bulk_create_tunnels, create_tunnel, delete_tunnel, get_tunnel, list_tunnels, update_tunnel,
};

use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access;
use crate::api::sync_helpers::resolve_netbox_provider_for_resource;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::application::RenderedConfigContext;
use crate::contexts::inventory::domain::{Device, Site, VirtualMachine};
use crate::contexts::inventory::infrastructure::caching::HostConfigCache;
use crate::contexts::shared::request_context::RequestContext;
use crate::contexts::sync::domain::{OutboundEvent, OutboundEventStatus};
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn slugify_site_name(name: &str) -> String {
    name.to_ascii_lowercase().replace(' ', "-")
}

fn resolve_site_slug(site: &Site) -> String {
    site.slug
        .clone()
        .unwrap_or_else(|| slugify_site_name(&site.name))
}

fn push_primary_ip_ids(
    ids: &mut Vec<Uuid>,
    primary_ip4_id: Option<Uuid>,
    primary_ip6_id: Option<Uuid>,
) {
    if let Some(id) = primary_ip4_id {
        ids.push(id);
    }
    if let Some(id) = primary_ip6_id {
        ids.push(id);
    }
}

fn resolve_primary_ip(
    primary_ip4_id: Option<Uuid>,
    primary_ip6_id: Option<Uuid>,
    ip_map: &HashMap<Uuid, String>,
) -> Option<String> {
    primary_ip4_id
        .and_then(|id| ip_map.get(&id).cloned())
        .or_else(|| primary_ip6_id.and_then(|id| ip_map.get(&id).cloned()))
}

fn map_device_response(
    device: Device,
    primary_ip: Option<String>,
    custom_fields: Option<serde_json::Value>,
) -> DeviceResponse {
    DeviceResponse {
        id: device.id,
        name: device.name,
        site_id: device.site_id,
        tenant_id: device.tenant_id,
        role_id: device.role_id,
        device_type_id: device.device_type_id,
        platform_id: device.platform_id,
        rack_id: device.rack_id,
        status: device.status,
        primary_ip,
        primary_ip4_id: device.primary_ip4_id,
        primary_ip6_id: device.primary_ip6_id,
        serial: device.serial,
        asset_tag: device.asset_tag,
        position: device.position,
        face: device.face,
        airflow: device.airflow,
        description: device.description,
        comments: device.comments,
        custom_fields,
        local_context_data: device.local_context_data,
    }
}

fn map_virtual_machine_response(
    vm: VirtualMachine,
    primary_ip: Option<String>,
    custom_fields: Option<serde_json::Value>,
) -> VirtualMachineResponse {
    VirtualMachineResponse {
        id: vm.id,
        name: vm.name,
        site_id: vm.site_id,
        cluster_id: vm.cluster_id,
        tenant_id: vm.tenant_id,
        role_id: vm.role_id,
        platform_id: vm.platform_id,
        virtual_machine_type_id: vm.virtual_machine_type_id,
        status: vm.status,
        primary_ip,
        primary_ip4_id: vm.primary_ip4_id,
        primary_ip6_id: vm.primary_ip6_id,
        vcpus: vm.vcpus,
        memory: vm.memory,
        disk: vm.disk,
        description: vm.description,
        comments: vm.comments,
        custom_fields,
        local_context_data: vm.local_context_data,
    }
}

fn map_rendered_config_context_response(
    rendered: RenderedConfigContext,
) -> RenderedConfigContextResponse {
    RenderedConfigContextResponse {
        rendered: rendered.rendered,
        local_context_data: rendered.local_context_data,
        sources: rendered
            .sources
            .into_iter()
            .map(|source| RenderedConfigContextSourceResponse {
                context_id: source.context_id,
                context_name: source.context_name,
                weight: source.weight,
            })
            .collect(),
    }
}

async fn ensure_create_inventory_access(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<(), AppError> {
    ensure_access(
        state,
        request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
}

/// Materialize an entity's custom fields for a response with secrets masked.
///
/// Secrets are never revealed on the standard read path (single GET or list);
/// cleartext is available only through the audited reveal endpoint. Returns the
/// flat `name -> value` map (defaults filled, secret values shown as `****`),
/// or `null` when the entity has no scoped custom fields.
/// Masked custom fields for a whole page in 3 queries total.
///
/// Never call [`masked_custom_fields`] from a list loop — that shape was the
/// dominant inventory N+1 (31 queries per row on the live schema).
async fn masked_custom_fields_page(
    state: &AppState,
    content_type_token: &str,
    entities: Vec<(Uuid, serde_json::Value)>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let views = state
        .services
        .custom_field_values
        .read_for_entities_token_masked(content_type_token, &entities)
        .await?;
    Ok(views.into_iter().map(|view| view.to_value_map()).collect())
}

/// Native read projection for entities whose list record has no CF column.
/// One JSONB query per page, then the same batched masked value service.
async fn masked_custom_fields_for_ids(
    state: &AppState,
    content_type_token: &str,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, serde_json::Value>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let entities = state
        .repositories
        .inventory_custom_field_data
        .read_page(content_type_token, ids)
        .await?;
    let keys: Vec<_> = entities.iter().map(|(id, _)| *id).collect();
    let values = masked_custom_fields_page(state, content_type_token, entities).await?;
    Ok(keys.into_iter().zip(values).collect())
}

trait InventoryCustomFieldProjection {
    const CONTENT_TYPE: &'static str;
    fn inventory_id(&self) -> Uuid;
    fn assign_custom_fields(&mut self, value: serde_json::Value);
}

macro_rules! impl_inventory_custom_fields {
    ($projection:ty, $content_type:literal) => {
        impl crate::api::rest::routes::inventory::InventoryCustomFieldProjection for $projection {
            const CONTENT_TYPE: &'static str = $content_type;
            fn inventory_id(&self) -> uuid::Uuid {
                self.id
            }
            fn assign_custom_fields(&mut self, value: serde_json::Value) {
                self.custom_fields = Some(value);
            }
        }
    };
}
pub(super) use impl_inventory_custom_fields;

async fn mask_inventory_projection_page<T: InventoryCustomFieldProjection>(
    state: &AppState,
    items: &mut [T],
) -> Result<(), AppError> {
    let ids: Vec<_> = items.iter().map(T::inventory_id).collect();
    let mut masked = masked_custom_fields_for_ids(state, T::CONTENT_TYPE, &ids).await?;
    for item in items {
        let value = masked.remove(&item.inventory_id()).ok_or_else(|| {
            AppError::NotFound("Inventory object disappeared during custom-field read".into())
        })?;
        item.assign_custom_fields(value);
    }
    Ok(())
}

async fn masked_inventory_response<T: InventoryCustomFieldProjection + serde::Serialize>(
    state: &AppState,
    status: axum::http::StatusCode,
    mut item: T,
) -> axum::response::Response {
    match mask_inventory_projection_page(state, std::slice::from_mut(&mut item)).await {
        Ok(()) => (status, axum::Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

async fn masked_custom_fields(
    state: &AppState,
    content_type_token: &str,
    entity_id: Uuid,
    raw_custom_field_data: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let view = state
        .services
        .custom_field_values
        .read_for_entity_token(content_type_token, entity_id, raw_custom_field_data, false)
        .await?;
    Ok(view.to_value_map())
}

async fn build_custom_field_list_filter(
    state: &AppState,
    content_type_token: &str,
    raw: &std::collections::BTreeMap<String, String>,
) -> Result<crate::contexts::shared::list_query::CustomFieldListFilter, AppError> {
    if raw.is_empty() {
        return Ok(crate::contexts::shared::list_query::CustomFieldListFilter::default());
    }
    let (app_label, model) = content_type_token.split_once('.').ok_or_else(|| {
        AppError::Validation(format!(
            "invalid content type token '{content_type_token}' (expected 'app_label.model')"
        ))
    })?;
    let content_type_id = state
        .services
        .custom_field_definitions
        .resolve_content_type_id(app_label, model)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!("unknown content type '{content_type_token}'"))
        })?;
    let definitions = state
        .services
        .custom_field_definitions
        .list_for_content_type(content_type_id)
        .await?;
    crate::contexts::shared::list_query::parse_custom_field_filters(raw, &definitions)
}

/// Validate an incoming `custom_fields` write against the definitions scoped to
/// `content_type_token` and split secret from non-secret values, WITHOUT
/// touching the secrets context yet.
///
/// `incoming` is the raw payload map. `None` (field omitted) leaves custom
/// fields untouched on update, but create-mode (`require_complete`) treats it
/// as an empty map so required custom fields are still enforced. Unknown /
/// out-of-scope keys are rejected here (fail-fast, before any entity row is
/// persisted). The returned [`WriteOutcome`] carries the sanitized non-secret
/// JSONB to persist on the entity row and the secret intents to apply AFTER the
/// row exists (see [`apply_custom_field_secret_intents`]).
async fn validate_and_split_custom_fields(
    state: &AppState,
    content_type_token: &str,
    incoming: Option<serde_json::Value>,
    require_complete: bool,
) -> Result<Option<crate::contexts::custom_fields::WriteOutcome>, AppError> {
    let incoming = match incoming {
        Some(value) => value,
        None if require_complete => serde_json::Value::Object(serde_json::Map::new()),
        None => return Ok(None),
    };
    let map = match incoming {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null => serde_json::Map::new(),
        _ => {
            return Err(AppError::Validation(
                "custom_fields must be an object".to_string(),
            ));
        }
    };
    // Create enforces required-field completeness; update is a partial merge.
    if require_complete {
        state
            .services
            .custom_field_values
            .check_required_present(content_type_token, &map)
            .await?;
    }
    let outcome = state
        .services
        .custom_field_values
        .write_for_entity_token(content_type_token, &map)
        .await?;
    Ok(Some(outcome))
}

/// Apply the secret intents produced by [`validate_and_split_custom_fields`]
/// against the secrets context, keyed to the now-persisted `entity_id`.
///
/// MUST be called only after the entity row exists: secret bindings reference
/// the entity, so persisting the row first avoids orphaning a binding if the
/// row create fails.
async fn apply_custom_field_secret_intents(
    state: &AppState,
    entity_id: Uuid,
    intents: Vec<crate::contexts::custom_fields::SecretWriteIntent>,
) -> Result<(), AppError> {
    state
        .services
        .custom_field_values
        .apply_secret_intents(entity_id, intents)
        .await
}

/// Validate, split, and persist an incoming `custom_fields` write for an entity
/// that already exists (update path), applying secret intents immediately.
///
/// For the CREATE path use [`validate_and_split_custom_fields`] +
/// [`apply_custom_field_secret_intents`] so the entity row is persisted before
/// any secret binding is written.
async fn apply_custom_fields_write(
    state: &AppState,
    content_type_token: &str,
    entity_id: Uuid,
    incoming: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, AppError> {
    let Some(outcome) =
        validate_and_split_custom_fields(state, content_type_token, incoming, false).await?
    else {
        return Ok(None);
    };
    apply_custom_field_secret_intents(state, entity_id, outcome.secret_intents).await?;
    Ok(Some(outcome.custom_field_data))
}

/// The indices align with the created entities so a page-wide masking failure
/// can be attributed to every affected input item.
struct BulkCreatedBatch<'a, E> {
    created: &'a mut Vec<E>,
    created_indices: &'a [usize],
    errors: &'a mut Vec<BulkCreateError>,
}

/// Shared tail of the inventory `bulk_create` handlers that return the created
/// entities with masked custom fields.
///
/// Masks the created page in one pass, attributes a page-wide masking failure to
/// every created row, and bumps the host-config cache generation when anything
/// survived. `entity_id` / `custom_fields` / `set_custom_fields` are per-entity
/// accessors that keep this generic without a trait impl per entity.
async fn finalize_bulk_created<E>(
    state: &AppState,
    content_type_token: &str,
    batch: BulkCreatedBatch<'_, E>,
    entity_id: impl Fn(&E) -> Uuid,
    custom_fields: impl Fn(&E) -> Option<&serde_json::Value>,
    set_custom_fields: impl Fn(&mut E, serde_json::Value),
) {
    let BulkCreatedBatch {
        created,
        created_indices,
        errors,
    } = batch;
    // One masking pass for the whole batch (3 queries), never per created row.
    if !created.is_empty() {
        let cf_entities: Vec<(Uuid, serde_json::Value)> = created
            .iter()
            .map(|entity| {
                (
                    entity_id(entity),
                    custom_fields(entity)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
            })
            .collect();
        match masked_custom_fields_page(state, content_type_token, cf_entities).await {
            Ok(masked) => {
                for (entity, value) in created.iter_mut().zip(masked) {
                    set_custom_fields(entity, value);
                }
            }
            Err(error) => {
                // The pass is page-wide: its failure invalidates every created
                // row's view, so report it against each of them.
                push_bulk_error_for_indices(errors, created_indices, error);
                created.clear();
            }
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(state).await;
    }
}

async fn bump_host_config_cache_generation(state: &AppState) {
    let mut redis = state.redis.clone();
    let cache = HostConfigCache::new();
    if let Err(error) = cache.bump_generation(&mut redis).await {
        log_error_event(
            "inventory.host_config_cache.generation_bump.failed",
            "inventory_host_config_cache_error",
            error.to_string(),
            EventFields::default()
                .with_operation("rest.inventory.host_config_cache.bump_generation")
                .with_resource("inventory_cache", "host_config_generation")
                .with_message("Failed to bump host config cache generation"),
        );
    }
}

async fn enqueue_site_outbound_event(
    state: &AppState,
    request_id: &RequestId,
    operation: &str,
    site: &Site,
    previous_slug: Option<String>,
) -> Result<(), AppError> {
    let Some(provider_id) = resolve_netbox_provider_for_resource(state, "dcim.sites").await? else {
        return Ok(());
    };
    let Some(provider) = state
        .services
        .source_service
        .get_source(provider_id)
        .await?
    else {
        return Ok(());
    };
    if !provider.is_enabled {
        return Ok(());
    }
    let sync_mode = provider
        .config
        .as_object()
        .and_then(|map| map.get("sync_mode"))
        .and_then(|value| value.as_str())
        .unwrap_or("pull_only");
    if sync_mode != "bidirectional" {
        return Ok(());
    }

    let slug = resolve_site_slug(site);
    let mut payload = json!({
        "local_id": site.id.to_string(),
        "name": site.name,
        "slug": slug,
    });
    if let Some(status) = site.status.as_deref() {
        payload["status"] = json!(status);
    }
    if let Some(previous_slug) = previous_slug
        && !previous_slug.trim().is_empty()
    {
        payload["previous_slug"] = json!(previous_slug);
    }

    let event = OutboundEvent {
        id: Uuid::now_v7(),
        source_id: provider_id,
        resource_type: "dcim.sites".to_string(),
        operation: operation.to_string(),
        payload,
        status: OutboundEventStatus::Pending,
        attempts: 0,
        next_attempt_at: None,
        processed_at: None,
        correlation_id: Some(format!("local:{}", request_id.0)),
        last_error: None,
        created_at: Utc::now(),
    };
    state.services.outbound_service.enqueue(&event).await
}
