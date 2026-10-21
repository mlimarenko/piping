mod registry_collection_operations;

use std::collections::HashSet;

use piping_backend::contexts::inventory::infrastructure::persistence::repositories as repos;
use piping_backend::contexts::shared::list_query::{FilterKind, ListColumn, SortSpec};

use self::registry_collection_operations::{
    NonHttpFilter, RegistryRoute, assert_registry_collection_operations,
};

struct Registry {
    name: &'static str,
    const_name: &'static str,
    columns: &'static [ListColumn],
    path: &'static str,
}

const REGISTRIES: &[Registry] = &[
    Registry {
        name: "owners",
        const_name: "OWNER_COLUMNS",
        columns: repos::native::owner_repository::OWNER_COLUMNS,
        path: "/api/v1/inventory/owners",
    },
    Registry {
        name: "owner_groups",
        const_name: "OWNER_GROUP_COLUMNS",
        columns: repos::native::owner_group_repository::OWNER_GROUP_COLUMNS,
        path: "/api/v1/inventory/owner_groups",
    },
    Registry {
        name: "aggregates",
        const_name: "AGGREGATE_COLUMNS",
        columns: repos::ipam::aggregate_repository::AGGREGATE_COLUMNS,
        path: "/api/v1/inventory/aggregates",
    },
    Registry {
        name: "asn_ranges",
        const_name: "ASN_RANGE_COLUMNS",
        columns: repos::ipam::asn_range_repository::ASN_RANGE_COLUMNS,
        path: "/api/v1/inventory/asn-ranges",
    },
    Registry {
        name: "asns",
        const_name: "ASN_COLUMNS",
        columns: repos::ipam::asn_repository::ASN_COLUMNS,
        path: "/api/v1/inventory/asns",
    },
    Registry {
        name: "cables",
        const_name: "CABLE_COLUMNS",
        columns: repos::dcim::cable_repository::CABLE_COLUMNS,
        path: "/api/v1/inventory/cables",
    },
    Registry {
        name: "circuit_groups",
        const_name: "CIRCUIT_GROUP_COLUMNS",
        columns: repos::circuits::circuit_group_repository::CIRCUIT_GROUP_COLUMNS,
        path: "/api/v1/inventory/circuit-groups",
    },
    Registry {
        name: "circuits",
        const_name: "CIRCUIT_COLUMNS",
        columns: repos::circuits::circuit_repository::CIRCUIT_COLUMNS,
        path: "/api/v1/inventory/circuits",
    },
    Registry {
        name: "circuit_terminations",
        const_name: "CIRCUIT_TERMINATION_COLUMNS",
        columns: repos::circuits::circuit_termination_repository::CIRCUIT_TERMINATION_COLUMNS,
        path: "/api/v1/inventory/circuit-terminations",
    },
    Registry {
        name: "circuit_types",
        const_name: "CIRCUIT_TYPE_COLUMNS",
        columns: repos::circuits::circuit_type_repository::CIRCUIT_TYPE_COLUMNS,
        path: "/api/v1/inventory/circuit-types",
    },
    Registry {
        name: "cluster_groups",
        const_name: "CLUSTER_GROUP_COLUMNS",
        columns: repos::virtualization::cluster_group_repository::CLUSTER_GROUP_COLUMNS,
        path: "/api/v1/inventory/cluster_groups",
    },
    Registry {
        name: "clusters",
        const_name: "CLUSTER_COLUMNS",
        columns: repos::virtualization::cluster_repository::CLUSTER_COLUMNS,
        path: "/api/v1/inventory/clusters",
    },
    Registry {
        name: "cluster_types",
        const_name: "CLUSTER_TYPE_COLUMNS",
        columns: repos::virtualization::cluster_type_repository::CLUSTER_TYPE_COLUMNS,
        path: "/api/v1/inventory/cluster_types",
    },
    Registry {
        name: "config_contexts",
        const_name: "CONFIG_CONTEXT_COLUMNS",
        columns: repos::extras::config_context_repository::CONFIG_CONTEXT_COLUMNS,
        path: "/api/v1/config-contexts",
    },
    Registry {
        name: "config_context_bindings",
        const_name: "CONFIG_CONTEXT_BINDING_COLUMNS",
        columns: repos::extras::config_context_repository::CONFIG_CONTEXT_BINDING_COLUMNS,
        path: "/api/v1/config-context-bindings",
    },
    Registry {
        name: "connection_targets",
        const_name: "CONNECTION_TARGET_COLUMNS",
        columns: repos::native::connection_target_repository::CONNECTION_TARGET_COLUMNS,
        path: "/api/v1/connections/targets",
    },
    Registry {
        name: "console_ports",
        const_name: "CONSOLE_PORT_COLUMNS",
        columns: repos::dcim::console_port_repository::CONSOLE_PORT_COLUMNS,
        path: "/api/v1/inventory/console-ports",
    },
    Registry {
        name: "console_server_ports",
        const_name: "CONSOLE_SERVER_PORT_COLUMNS",
        columns: repos::dcim::console_server_port_repository::CONSOLE_SERVER_PORT_COLUMNS,
        path: "/api/v1/inventory/console-server-ports",
    },
    Registry {
        name: "contact_assignments",
        const_name: "CONTACT_ASSIGNMENT_COLUMNS",
        columns: repos::tenancy::contact_assignment_repository::CONTACT_ASSIGNMENT_COLUMNS,
        path: "/api/v1/inventory/contact-assignments",
    },
    Registry {
        name: "contact_groups",
        const_name: "CONTACT_GROUP_COLUMNS",
        columns: repos::tenancy::contact_group_repository::CONTACT_GROUP_COLUMNS,
        path: "/api/v1/inventory/contact-groups",
    },
    Registry {
        name: "contacts",
        const_name: "CONTACT_COLUMNS",
        columns: repos::tenancy::contact_repository::CONTACT_COLUMNS,
        path: "/api/v1/inventory/contacts",
    },
    Registry {
        name: "contact_roles",
        const_name: "CONTACT_ROLE_COLUMNS",
        columns: repos::tenancy::contact_role_repository::CONTACT_ROLE_COLUMNS,
        path: "/api/v1/inventory/contact-roles",
    },
    Registry {
        name: "content_types",
        const_name: "CONTENT_TYPE_COLUMNS",
        columns: repos::content_type_lookup::CONTENT_TYPE_COLUMNS,
        path: "/api/v1/inventory/content-types",
    },
    Registry {
        name: "device_bays",
        const_name: "DEVICE_BAY_COLUMNS",
        columns: repos::dcim::device_bay_repository::DEVICE_BAY_COLUMNS,
        path: "/api/v1/inventory/device-bays",
    },
    Registry {
        name: "devices",
        const_name: "DEVICE_COLUMNS",
        columns: repos::dcim::device_repository::DEVICE_COLUMNS,
        path: "/api/v1/inventory/devices",
    },
    Registry {
        name: "device_roles",
        const_name: "DEVICE_ROLE_COLUMNS",
        columns: repos::dcim::device_role_repository::DEVICE_ROLE_COLUMNS,
        path: "/api/v1/inventory/device_roles",
    },
    Registry {
        name: "device_types",
        const_name: "DEVICE_TYPE_COLUMNS",
        columns: repos::dcim::device_type_repository::DEVICE_TYPE_COLUMNS,
        path: "/api/v1/inventory/device_types",
    },
    Registry {
        name: "front_ports",
        const_name: "FRONT_PORT_COLUMNS",
        columns: repos::dcim::front_port_repository::FRONT_PORT_COLUMNS,
        path: "/api/v1/inventory/front-ports",
    },
    Registry {
        name: "interfaces",
        const_name: "INTERFACE_COLUMNS",
        columns: repos::dcim::interface_repository::INTERFACE_COLUMNS,
        path: "/api/v1/inventory/interfaces",
    },
    Registry {
        name: "inventory_items",
        const_name: "INVENTORY_ITEM_COLUMNS",
        columns: repos::dcim::inventory_item_repository::INVENTORY_ITEM_COLUMNS,
        path: "/api/v1/inventory/inventory-items",
    },
    Registry {
        name: "inventory_item_roles",
        const_name: "INVENTORY_ITEM_ROLE_COLUMNS",
        columns: repos::dcim::inventory_item_role_repository::INVENTORY_ITEM_ROLE_COLUMNS,
        path: "/api/v1/inventory/inventory-item-roles",
    },
    Registry {
        name: "ip_ranges",
        const_name: "IP_RANGE_COLUMNS",
        columns: repos::ipam::ip_range_repository::IP_RANGE_COLUMNS,
        path: "/api/v1/inventory/ip_ranges",
    },
    Registry {
        name: "ip_addresses",
        const_name: "IP_ADDRESS_COLUMNS",
        columns: repos::native::ipam_repository::IP_ADDRESS_COLUMNS,
        path: "/api/v1/inventory/ip-addresses",
    },
    Registry {
        name: "ipam_roles",
        const_name: "IPAM_ROLE_COLUMNS",
        columns: repos::ipam::ipam_role_repository::IPAM_ROLE_COLUMNS,
        path: "/api/v1/inventory/ipam_roles",
    },
    Registry {
        name: "locations",
        const_name: "LOCATION_LIST_COLUMNS",
        columns: repos::dcim::location_repository::LOCATION_LIST_COLUMNS,
        path: "/api/v1/inventory/locations",
    },
    Registry {
        name: "mac_addresses",
        const_name: "MAC_ADDRESS_COLUMNS",
        columns: repos::dcim::mac_address_repository::MAC_ADDRESS_COLUMNS,
        path: "/api/v1/inventory/mac-addresses",
    },
    Registry {
        name: "manufacturers",
        const_name: "MANUFACTURER_COLUMNS",
        columns: repos::dcim::manufacturer_repository::MANUFACTURER_COLUMNS,
        path: "/api/v1/inventory/manufacturers",
    },
    Registry {
        name: "module_bays",
        const_name: "MODULE_BAY_COLUMNS",
        columns: repos::dcim::module_bay_repository::MODULE_BAY_COLUMNS,
        path: "/api/v1/inventory/module-bays",
    },
    Registry {
        name: "modules",
        const_name: "MODULE_COLUMNS",
        columns: repos::dcim::module_repository::MODULE_COLUMNS,
        path: "/api/v1/inventory/modules",
    },
    Registry {
        name: "module_types",
        const_name: "MODULE_TYPE_COLUMNS",
        columns: repos::dcim::module_type_repository::MODULE_TYPE_COLUMNS,
        path: "/api/v1/inventory/module-types",
    },
    Registry {
        name: "module_type_profiles",
        const_name: "MODULE_TYPE_PROFILE_COLUMNS",
        columns: repos::dcim::module_type_profile_repository::MODULE_TYPE_PROFILE_COLUMNS,
        path: "/api/v1/inventory/module-type-profiles",
    },
    Registry {
        name: "tags",
        const_name: "TAG_COLUMNS",
        columns: repos::extras::tag_repository::TAG_COLUMNS,
        path: "/api/v1/inventory/tags",
    },
    Registry {
        name: "platforms",
        const_name: "PLATFORM_COLUMNS",
        columns: repos::dcim::platform_repository::PLATFORM_COLUMNS,
        path: "/api/v1/inventory/platforms",
    },
    Registry {
        name: "power_feeds",
        const_name: "POWER_FEED_COLUMNS",
        columns: repos::dcim::power_feed_repository::POWER_FEED_COLUMNS,
        path: "/api/v1/inventory/power-feeds",
    },
    Registry {
        name: "power_outlets",
        const_name: "POWER_OUTLET_COLUMNS",
        columns: repos::dcim::power_outlet_repository::POWER_OUTLET_COLUMNS,
        path: "/api/v1/inventory/power-outlets",
    },
    Registry {
        name: "power_panels",
        const_name: "POWER_PANEL_COLUMNS",
        columns: repos::dcim::power_panel_repository::POWER_PANEL_COLUMNS,
        path: "/api/v1/inventory/power-panels",
    },
    Registry {
        name: "power_ports",
        const_name: "POWER_PORT_COLUMNS",
        columns: repos::dcim::power_port_repository::POWER_PORT_COLUMNS,
        path: "/api/v1/inventory/power-ports",
    },
    Registry {
        name: "prefixes",
        const_name: "PREFIX_COLUMNS",
        columns: repos::ipam::prefix_repository::PREFIX_COLUMNS,
        path: "/api/v1/inventory/prefixes",
    },
    Registry {
        name: "provider_accounts",
        const_name: "PROVIDER_ACCOUNT_COLUMNS",
        columns: repos::circuits::provider_account_repository::PROVIDER_ACCOUNT_COLUMNS,
        path: "/api/v1/inventory/provider-accounts",
    },
    Registry {
        name: "provider_networks",
        const_name: "PROVIDER_NETWORK_COLUMNS",
        columns: repos::circuits::provider_network_repository::PROVIDER_NETWORK_COLUMNS,
        path: "/api/v1/inventory/provider-networks",
    },
    Registry {
        name: "providers",
        const_name: "PROVIDER_COLUMNS",
        columns: repos::circuits::provider_repository::PROVIDER_COLUMNS,
        path: "/api/v1/inventory/providers",
    },
    Registry {
        name: "racks",
        const_name: "RACK_COLUMNS",
        columns: repos::dcim::rack_repository::RACK_COLUMNS,
        path: "/api/v1/inventory/racks",
    },
    Registry {
        name: "rack_reservations",
        const_name: "RACK_RESERVATION_COLUMNS",
        columns: repos::dcim::rack_reservation_repository::RACK_RESERVATION_COLUMNS,
        path: "/api/v1/inventory/rack-reservations",
    },
    Registry {
        name: "rack_roles",
        const_name: "RACK_ROLE_COLUMNS",
        columns: repos::dcim::rack_role_repository::RACK_ROLE_COLUMNS,
        path: "/api/v1/inventory/rack_roles",
    },
    Registry {
        name: "rack_types",
        const_name: "RACK_TYPE_LIST_COLUMNS",
        columns: repos::dcim::rack_type_repository::RACK_TYPE_LIST_COLUMNS,
        path: "/api/v1/inventory/rack_types",
    },
    Registry {
        name: "rear_ports",
        const_name: "REAR_PORT_COLUMNS",
        columns: repos::dcim::rear_port_repository::REAR_PORT_COLUMNS,
        path: "/api/v1/inventory/rear-ports",
    },
    Registry {
        name: "regions",
        const_name: "REGION_COLUMNS",
        columns: repos::dcim::region_repository::REGION_COLUMNS,
        path: "/api/v1/inventory/regions",
    },
    Registry {
        name: "rirs",
        const_name: "RIR_COLUMNS",
        columns: repos::ipam::rir_repository::RIR_COLUMNS,
        path: "/api/v1/inventory/rirs",
    },
    Registry {
        name: "route_targets",
        const_name: "ROUTE_TARGET_COLUMNS",
        columns: repos::ipam::route_target_repository::ROUTE_TARGET_COLUMNS,
        path: "/api/v1/inventory/route_targets",
    },
    Registry {
        name: "services",
        const_name: "SERVICE_COLUMNS",
        columns: repos::ipam::service_repository::SERVICE_COLUMNS,
        path: "/api/v1/inventory/services",
    },
    Registry {
        name: "service_templates",
        const_name: "SERVICE_TEMPLATE_COLUMNS",
        columns: repos::ipam::service_template_repository::SERVICE_TEMPLATE_COLUMNS,
        path: "/api/v1/inventory/service-templates",
    },
    Registry {
        name: "site_groups",
        const_name: "SITE_GROUP_COLUMNS",
        columns: repos::dcim::site_group_repository::SITE_GROUP_COLUMNS,
        path: "/api/v1/inventory/site_groups",
    },
    Registry {
        name: "sites",
        const_name: "SITE_COLUMNS",
        columns: repos::dcim::site_repository::SITE_COLUMNS,
        path: "/api/v1/inventory/sites",
    },
    Registry {
        name: "inventory_templates",
        const_name: "INVENTORY_TEMPLATE_COLUMNS",
        columns: repos::native::template_repository::INVENTORY_TEMPLATE_COLUMNS,
        path: "/api/v1/inventory/templates",
    },
    Registry {
        name: "tenant_groups",
        const_name: "TENANT_GROUP_COLUMNS",
        columns: repos::tenancy::tenant_group_repository::TENANT_GROUP_COLUMNS,
        path: "/api/v1/inventory/tenant_groups",
    },
    Registry {
        name: "tenants",
        const_name: "TENANT_COLUMNS",
        columns: repos::tenancy::tenant_repository::TENANT_COLUMNS,
        path: "/api/v1/inventory/tenants",
    },
    Registry {
        name: "virtual_chassis",
        const_name: "VIRTUAL_CHASSIS_COLUMNS",
        columns: repos::dcim::virtual_chassis_repository::VIRTUAL_CHASSIS_COLUMNS,
        path: "/api/v1/inventory/virtual-chassis",
    },
    Registry {
        name: "virtual_circuits",
        const_name: "VIRTUAL_CIRCUIT_COLUMNS",
        columns: repos::circuits::virtual_circuit_repository::VIRTUAL_CIRCUIT_COLUMNS,
        path: "/api/v1/inventory/virtual-circuits",
    },
    Registry {
        name: "virtual_circuit_types",
        const_name: "VIRTUAL_CIRCUIT_TYPE_COLUMNS",
        columns: repos::circuits::virtual_circuit_type_repository::VIRTUAL_CIRCUIT_TYPE_COLUMNS,
        path: "/api/v1/inventory/virtual-circuit-types",
    },
    Registry {
        name: "virtual_device_contexts",
        const_name: "VIRTUAL_DEVICE_CONTEXT_COLUMNS",
        columns:
            repos::virtualization::virtual_device_context_repository::VIRTUAL_DEVICE_CONTEXT_COLUMNS,
        path: "/api/v1/inventory/virtual-device-contexts",
    },
    Registry {
        name: "virtual_disks",
        const_name: "VIRTUAL_DISK_COLUMNS",
        columns: repos::virtualization::virtual_disk_repository::VIRTUAL_DISK_COLUMNS,
        path: "/api/v1/inventory/virtual-disks",
    },
    Registry {
        name: "virtual_machine_types",
        const_name: "VIRTUAL_MACHINE_TYPE_COLUMNS",
        columns:
            repos::virtualization::virtual_machine_type_repository::VIRTUAL_MACHINE_TYPE_COLUMNS,
        path: "/api/v1/inventory/virtual-machine-types",
    },
    Registry {
        name: "virtual_machines",
        const_name: "VIRTUAL_MACHINE_COLUMNS",
        columns: repos::virtualization::virtual_machine_repository::VIRTUAL_MACHINE_COLUMNS,
        path: "/api/v1/inventory/virtual_machines",
    },
    Registry {
        name: "vlan_groups",
        const_name: "VLAN_GROUP_COLUMNS",
        columns: repos::ipam::vlan_group_repository::VLAN_GROUP_COLUMNS,
        path: "/api/v1/inventory/vlan_groups",
    },
    Registry {
        name: "vlans",
        const_name: "VLAN_COLUMNS",
        columns: repos::ipam::vlan_repository::VLAN_COLUMNS,
        path: "/api/v1/inventory/vlans",
    },
    Registry {
        name: "vm_interfaces",
        const_name: "VM_INTERFACE_COLUMNS",
        columns: repos::virtualization::vm_interface_repository::VM_INTERFACE_COLUMNS,
        path: "/api/v1/inventory/vm-interfaces",
    },
    Registry {
        name: "ike_policies",
        const_name: "VPN_IKE_POLICY_COLUMNS",
        columns: repos::vpn::vpn_ike_policy_repository::VPN_IKE_POLICY_COLUMNS,
        path: "/api/v1/inventory/ike-policies",
    },
    Registry {
        name: "ike_proposals",
        const_name: "VPN_IKE_PROPOSAL_COLUMNS",
        columns: repos::vpn::vpn_ike_proposal_repository::VPN_IKE_PROPOSAL_COLUMNS,
        path: "/api/v1/inventory/ike-proposals",
    },
    Registry {
        name: "ipsec_policies",
        const_name: "VPN_IPSEC_POLICY_COLUMNS",
        columns: repos::vpn::vpn_ipsec_policy_repository::VPN_IPSEC_POLICY_COLUMNS,
        path: "/api/v1/inventory/ipsec-policies",
    },
    Registry {
        name: "ipsec_profiles",
        const_name: "VPN_IPSEC_PROFILE_COLUMNS",
        columns: repos::vpn::vpn_ipsec_profile_repository::VPN_IPSEC_PROFILE_COLUMNS,
        path: "/api/v1/inventory/ipsec-profiles",
    },
    Registry {
        name: "ipsec_proposals",
        const_name: "VPN_IPSEC_PROPOSAL_COLUMNS",
        columns: repos::vpn::vpn_ipsec_proposal_repository::VPN_IPSEC_PROPOSAL_COLUMNS,
        path: "/api/v1/inventory/ipsec-proposals",
    },
    Registry {
        name: "tunnel_groups",
        const_name: "VPN_TUNNEL_GROUP_COLUMNS",
        columns: repos::vpn::vpn_tunnel_group_repository::VPN_TUNNEL_GROUP_COLUMNS,
        path: "/api/v1/inventory/tunnel-groups",
    },
    Registry {
        name: "tunnels",
        const_name: "VPN_TUNNEL_COLUMNS",
        columns: repos::vpn::vpn_tunnel_repository::VPN_TUNNEL_COLUMNS,
        path: "/api/v1/inventory/tunnels",
    },
    Registry {
        name: "tunnel_terminations",
        const_name: "VPN_TUNNEL_TERMINATION_COLUMNS",
        columns: repos::vpn::vpn_tunnel_termination_repository::VPN_TUNNEL_TERMINATION_COLUMNS,
        path: "/api/v1/inventory/tunnel-terminations",
    },
    Registry {
        name: "vrfs",
        const_name: "VRF_COLUMNS",
        columns: repos::ipam::vrf_repository::VRF_COLUMNS,
        path: "/api/v1/inventory/vrfs",
    },
];

/// Inventory registries whose declared filter is not a settable query
/// parameter, each with the reason that makes it so.
const NON_HTTP_FILTERS: &[NonHttpFilter] = &[];

fn assert_inventory_column_identity(
    registry: &Registry,
    column: &ListColumn,
    keys: &mut HashSet<&'static str>,
) {
    assert!(
        !column.key.trim().is_empty(),
        "{} has an empty column key",
        registry.name
    );
    assert!(
        !column.key.chars().any(char::is_whitespace),
        "{} has a whitespace-bearing column key: {:?}",
        registry.name,
        column.key
    );
    assert!(
        keys.insert(column.key),
        "{} has duplicate column key {:?}",
        registry.name,
        column.key
    );
    assert!(
        !column.label_key.trim().is_empty(),
        "{}.{} has an empty label_key",
        registry.name,
        column.key
    );
}

fn assert_inventory_sortable_column(
    registry: &Registry,
    column: &ListColumn,
    sortable_count: &mut usize,
) {
    if let Some(sort_sql) = column.sort_sql {
        *sortable_count += 1;
        assert!(
            !sort_sql.trim().is_empty(),
            "{}.{} has empty sort SQL",
            registry.name,
            column.key
        );
        assert!(
            SortSpec::parse_columns(Some(column.key), Some("asc"), registry.columns).is_ok(),
            "{} rejected its own asc sort key {:?}",
            registry.name,
            column.key
        );
        assert!(
            SortSpec::parse_columns(Some(column.key), Some("desc"), registry.columns).is_ok(),
            "{} rejected its own desc sort key {:?}",
            registry.name,
            column.key
        );
    }
}

fn assert_inventory_filter_column(
    registry: &Registry,
    column: &ListColumn,
    filter_params: &mut HashSet<&'static str>,
) {
    if let Some(filter) = column.filter {
        assert!(
            !filter.param.trim().is_empty(),
            "{}.{} has an empty filter param",
            registry.name,
            column.key
        );
        assert!(
            !filter.sql.trim().is_empty(),
            "{}.{} has empty filter SQL",
            registry.name,
            column.key
        );
        assert!(
            filter_params.insert(filter.param),
            "{} has duplicate filter param {:?}",
            registry.name,
            filter.param
        );
        assert_inventory_uuid_filter(registry, column, filter.param, filter.kind);
    }
}

fn assert_inventory_uuid_filter(
    registry: &Registry,
    column: &ListColumn,
    filter_param: &'static str,
    filter_kind: FilterKind,
) {
    if filter_kind == FilterKind::Uuid {
        assert!(
            column.fk_entity.is_some(),
            "{}.{} UUID filter {:?} must declare fk_entity metadata",
            registry.name,
            column.key,
            filter_param
        );
    }
}

fn validate_inventory_registry(registry: &Registry) {
    let mut keys = HashSet::new();
    let mut filter_params = HashSet::new();
    let mut sortable_count = 0;

    for column in registry.columns {
        assert_inventory_column_identity(registry, column, &mut keys);
        assert_inventory_sortable_column(registry, column, &mut sortable_count);
        assert_inventory_filter_column(registry, column, &mut filter_params);
    }

    assert!(
        sortable_count > 0,
        "{} has no sortable columns",
        registry.name
    );
}

#[test]
fn inventory_table_column_registries_are_valid() {
    for registry in REGISTRIES {
        assert!(
            !registry.columns.is_empty(),
            "{} has no declared columns",
            registry.name
        );

        validate_inventory_registry(registry);

        assert!(
            SortSpec::parse_columns(Some("__unsupported__"), None, registry.columns).is_err(),
            "{} accepted an unsupported sort key",
            registry.name
        );
    }
}

/// The frontend table checks (`apps/web/src/app/tableContracts.test.ts`,
/// `apps/web/src/features/inventory/inventoryFilterBackendParity.test.ts`) parse
/// this table by `name` and `const_name` into maps keyed by those identifiers, so
/// a duplicate would silently drop a registry there.
#[test]
fn inventory_registry_identifier_keys_are_unique() {
    let mut names = HashSet::new();
    let mut const_names = HashSet::new();

    for registry in REGISTRIES {
        assert!(
            names.insert(registry.name),
            "duplicate registry name {:?}",
            registry.name
        );
        assert!(
            const_names.insert(registry.const_name),
            "duplicate registry const name {:?}",
            registry.const_name
        );
    }
}

/// Every inventory registry must be served by a documented `GET` collection
/// operation that declares the query parameters its columns filter on.
#[test]
fn inventory_registries_resolve_to_documented_collection_operations()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let routes: Vec<RegistryRoute> = REGISTRIES
        .iter()
        .map(|registry| RegistryRoute {
            name: registry.name,
            path: registry.path,
            columns: registry.columns,
        })
        .collect();

    assert_registry_collection_operations(&routes, NON_HTTP_FILTERS)
}
