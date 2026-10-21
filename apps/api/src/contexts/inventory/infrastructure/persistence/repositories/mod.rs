//! Inventory SQLx repository modules and their public re-exports.
pub mod circuits;
pub mod dcim;
pub mod extras;
pub mod ipam;
pub mod native;
pub mod tenancy;
pub mod virtualization;
pub mod vpn;

pub mod bulk_insert;
pub mod bulk_resource_repository;
pub mod content_type_lookup;
pub mod custom_field_data_repository;
pub mod delete_guard;
pub mod object_ref_resolver;
pub use native::collected_facts_repository::{
    CollectedDiskFacts, CollectedFactsIngestRequest, CollectedFactsIngestResponse,
    CollectedFactsInventoryKind, CollectedFactsWritePolicy, CollectedHardwareFacts,
    CollectedInterfaceFacts, CollectedPlatformFacts, SqlxCollectedFactsRepository,
};
pub use native::connection_target_repository::{
    CONNECTION_TARGET_COLUMNS, ConnectionTarget, ConnectionTargetFilters, ConnectionTargetKind,
    ConnectionTargetKindFilter, ConnectionTargetRepository, SqlxConnectionTargetRepository,
};
pub use native::owner_group_repository::{
    OWNER_GROUP_COLUMNS, OwnerGroupRepository, SqlxOwnerGroupRepository,
};
pub use native::owner_repository::{
    OWNER_COLUMNS, OwnerChanges, OwnerFilter, OwnerRepository, SqlxOwnerRepository,
};
pub mod helpers;
pub use dcim::module_type_profile_repository::{
    MODULE_TYPE_PROFILE_COLUMNS, SqlxModuleTypeProfileRepository,
};
pub mod repository;
pub use extras::tag_repository::{SqlxTagRepository, TAG_COLUMNS};
pub mod visibility_sql;

pub use bulk_insert::BulkCreateResult;
pub use bulk_resource_repository::{
    BulkColumn, BulkResourceRepository, SqlxBulkResourceRepository,
};
pub use circuits::circuit_group_repository::{
    CIRCUIT_GROUP_COLUMNS, CircuitGroupRepository, SqlxCircuitGroupRepository,
};
pub use circuits::circuit_repository::{
    BulkCircuitResult, CIRCUIT_COLUMNS, CircuitFilter, CircuitListItem, CircuitRecord,
    CircuitRepository, NewCircuit, SqlxCircuitRepository, UpdateCircuit,
};
pub use circuits::circuit_termination_repository::{
    CIRCUIT_TERMINATION_COLUMNS, CircuitTerminationFilter, CircuitTerminationRepository,
    SqlxCircuitTerminationRepository,
};
pub use circuits::circuit_type_repository::{
    BulkCircuitTypeResult, CIRCUIT_TYPE_COLUMNS, CircuitTypeRecord, CircuitTypeRepository,
    NewCircuitType, SqlxCircuitTypeRepository, UpdateCircuitType,
};
pub use circuits::provider_account_repository::{
    PROVIDER_ACCOUNT_COLUMNS, ProviderAccountListItem, ProviderAccountRepository,
    SqlxProviderAccountRepository,
};
pub use circuits::provider_network_repository::{
    BulkProviderNetworkResult, NewProviderNetwork, PROVIDER_NETWORK_COLUMNS,
    ProviderNetworkListItem, ProviderNetworkRecord, ProviderNetworkRepository,
    SqlxProviderNetworkRepository, UpdateProviderNetwork,
};
pub use circuits::provider_repository::{
    PROVIDER_COLUMNS, ProviderRepository, SqlxProviderRepository,
};
pub use circuits::virtual_circuit_repository::{
    SqlxVirtualCircuitRepository, VIRTUAL_CIRCUIT_COLUMNS, VirtualCircuitFilter,
    VirtualCircuitListItem, VirtualCircuitRepository,
};
pub use circuits::virtual_circuit_type_repository::{
    SqlxVirtualCircuitTypeRepository, VIRTUAL_CIRCUIT_TYPE_COLUMNS, VirtualCircuitTypeRepository,
};
pub use content_type_lookup::{
    ContentTypeRow, list_content_types, lookup_content_type_id, lookup_content_type_id_tx,
};
pub use dcim::cable_repository::{
    BulkCableResult, CABLE_COLUMNS, CableFilter, CableRecord, CableRepository, NewCable,
    SqlxCableRepository, UpdateCable,
};
pub use dcim::cabling_repository::{
    CableEndpointKind, CablingCableSummary, CablingEndpointFilter, CablingEndpointItem,
    CablingEndpointRef, CablingIssueCode, CablingLinkState, CablingLocation, CablingRepository,
    CablingResourceLink, CablingTrace, CablingTraceIssue, CablingTraceSegment, CablingTraceState,
    CablingTraceSummary, CablingTraversal, SqlxCablingRepository,
};
pub use dcim::console_port_repository::{
    CONSOLE_PORT_COLUMNS, ConsolePortFilter, ConsolePortRecord, ConsolePortRepository,
    NewConsolePort, SqlxConsolePortRepository, UpdateConsolePort,
};
pub use dcim::console_server_port_repository::{
    BulkConsoleServerPortResult, CONSOLE_SERVER_PORT_COLUMNS, ConsoleServerPortFilter,
    ConsoleServerPortRecord, ConsoleServerPortRepository, NewConsoleServerPort,
    SqlxConsoleServerPortRepository, UpdateConsoleServerPort,
};
pub use dcim::device_bay_repository::{
    BulkDeviceBayResult, DEVICE_BAY_COLUMNS, DeviceBayFilter, DeviceBayRecord, DeviceBayRepository,
    NewDeviceBay, SqlxDeviceBayRepository, UpdateDeviceBay,
};
pub use dcim::device_repository::{DEVICE_COLUMNS, SqlxDeviceRepository};
pub use dcim::device_role_repository::SqlxDeviceRoleRepository;
pub use dcim::device_type_repository::{DEVICE_TYPE_COLUMNS, SqlxDeviceTypeRepository};
pub use dcim::front_port_repository::{
    BulkFrontPortResult, FRONT_PORT_COLUMNS, FrontPortFilter, FrontPortRecord, FrontPortRepository,
    NewFrontPort, SqlxFrontPortRepository, UpdateFrontPort,
};
pub use dcim::interface_repository::{
    BulkInterfaceResult, INTERFACE_COLUMNS, InterfaceRecord, InterfaceRepository, NewInterface,
    SqlxInterfaceRepository, UpdateInterface,
};
pub use dcim::inventory_item_repository::{
    BulkInventoryItemResult, INVENTORY_ITEM_COLUMNS, InventoryItemFilter, InventoryItemRecord,
    InventoryItemRepository, NewInventoryItem, SqlxInventoryItemRepository, UpdateInventoryItem,
};
pub use dcim::inventory_item_role_repository::{
    BulkInventoryItemRoleResult, INVENTORY_ITEM_ROLE_COLUMNS, InventoryItemRoleRecord,
    InventoryItemRoleRepository, NewInventoryItemRole, SqlxInventoryItemRoleRepository,
    UpdateInventoryItemRole,
};
pub use dcim::location_repository::{
    BulkLocationResult, LocationFilter, LocationListItem, LocationRecord, LocationRepository,
    NewLocation, SqlxLocationRepository, UpdateLocation,
};
pub use dcim::mac_address_repository::{
    MAC_ADDRESS_COLUMNS, MacAddressFilter, MacAddressRepository, SqlxMacAddressRepository,
};
pub use dcim::manufacturer_repository::{MANUFACTURER_COLUMNS, SqlxManufacturerRepository};
pub use dcim::module_bay_repository::{
    BulkModuleBayResult, MODULE_BAY_COLUMNS, ModuleBayFilter, ModuleBayRecord, ModuleBayRepository,
    NewModuleBay, SqlxModuleBayRepository, UpdateModuleBay,
};
pub use dcim::module_repository::{
    BulkModuleResult, MODULE_COLUMNS, ModuleRecord, ModuleRepository, NewModule,
    SqlxModuleRepository, UpdateModule,
};
pub use dcim::module_type_repository::{
    MODULE_TYPE_COLUMNS, ModuleTypeRepository, SqlxModuleTypeRepository,
};
pub use dcim::platform_repository::{
    NewPlatform, PLATFORM_COLUMNS, PlatformListItem, PlatformRecord, PlatformRepository,
    SqlxPlatformRepository, UpdatePlatform,
};
pub use dcim::power_feed_repository::{
    POWER_FEED_COLUMNS, PowerFeedRepository, SqlxPowerFeedRepository,
};
pub use dcim::power_outlet_repository::{
    BulkPowerOutletResult, NewPowerOutlet, POWER_OUTLET_COLUMNS, PowerOutletFilter,
    PowerOutletRecord, PowerOutletRepository, SqlxPowerOutletRepository, UpdatePowerOutlet,
};
pub use dcim::power_panel_repository::{
    POWER_PANEL_COLUMNS, PowerPanelRepository, SqlxPowerPanelRepository,
};
pub use dcim::power_port_repository::{
    BulkPowerPortResult, NewPowerPort, POWER_PORT_COLUMNS, PowerPortFilter, PowerPortRecord,
    PowerPortRepository, SqlxPowerPortRepository, UpdatePowerPort,
};
pub use dcim::rack_repository::{
    BulkRackResult, NewRack, RACK_COLUMNS, RackFilter, RackListItem, RackRecord, RackRepository,
    SqlxRackRepository, UpdateRack,
};
pub use dcim::rack_reservation_repository::{
    RACK_RESERVATION_COLUMNS, RackReservationRepository, SqlxRackReservationRepository,
};
pub use dcim::rack_role_repository::{
    NewRackRole, RackRoleRecord, RackRoleRepository, SqlxRackRoleRepository, UpdateRackRole,
};
pub use dcim::rack_type_repository::{
    BulkRackTypeResult, NewRackType, RackTypeFilter, RackTypeListItem, RackTypeRecord,
    RackTypeRepository, SqlxRackTypeRepository, UpdateRackType,
};
pub use dcim::rear_port_repository::{
    BulkRearPortResult, NewRearPort, REAR_PORT_COLUMNS, RearPortFilter, RearPortRecord,
    RearPortRepository, SqlxRearPortRepository, UpdateRearPort,
};
pub use dcim::region_repository::{REGION_COLUMNS, SqlxRegionRepository};
pub use dcim::site_group_repository::SqlxSiteGroupRepository;
pub use dcim::site_repository::{SITE_COLUMNS, SqlxSiteRepository};
pub use dcim::virtual_chassis_repository::{
    BulkVirtualChassisResult, NewVirtualChassis, SqlxVirtualChassisRepository,
    UpdateVirtualChassis, VIRTUAL_CHASSIS_COLUMNS, VirtualChassisRecord, VirtualChassisRepository,
};
pub use extras::config_context_repository::{
    CONFIG_CONTEXT_COLUMNS, ConfigContextBindingListFilters, ConfigContextRepository,
    SqlxConfigContextRepository,
};
pub use helpers::derive_slug;
pub use ipam::aggregate_repository::{
    AGGREGATE_COLUMNS, AggregateListItem, AggregateRecord, AggregateRepository,
    BulkAggregateResult, NewAggregate, SqlxAggregateRepository, UpdateAggregate,
};
pub use ipam::asn_range_repository::{
    ASN_RANGE_COLUMNS, AsnRangeRecord, AsnRangeRepository, BulkAsnRangeResult, NewAsnRange,
    SqlxAsnRangeRepository, UpdateAsnRange,
};
pub use ipam::asn_repository::{
    ASN_COLUMNS, AsnListItem, AsnRecord, AsnRepository, NewAsn, SqlxAsnRepository, UpdateAsn,
};
pub use ipam::ip_range_repository::{
    BulkIpRangeResult, IP_RANGE_COLUMNS, IpRangeFilter, IpRangeListItem, IpRangeRecord,
    IpRangeRepository, NewIpRange, SqlxIpRangeRepository, UpdateIpRange,
};
pub use ipam::ipam_role_repository::{
    IPAM_ROLE_COLUMNS, IpamRoleRecord, IpamRoleRepository, NewIpamRole, SqlxIpamRoleRepository,
    UpdateIpamRole,
};
pub use ipam::prefix_repository::{
    BulkPrefixResult, NewPrefix, PREFIX_COLUMNS, PrefixFilter, PrefixListItem,
    PrefixPlanningPreview, PrefixRecord, PrefixRepository, PrefixTreeFilter, PrefixTreeNode,
    PrefixTreeReadModel, SqlxPrefixRepository, UpdatePrefix,
};
pub use ipam::rir_repository::{
    NewRir, RIR_COLUMNS, RirRecord, RirRepository, SqlxRirRepository, UpdateRir,
};
pub use ipam::route_target_repository::{
    BulkRouteTargetResult, NewRouteTarget, ROUTE_TARGET_COLUMNS, RouteTargetListItem,
    RouteTargetRecord, RouteTargetRepository, SqlxRouteTargetRepository, UpdateRouteTarget,
};
pub use ipam::service_repository::{
    NewService, SERVICE_COLUMNS, ServiceRecord, ServiceRepository, SqlxServiceRepository,
    UpdateService,
};
pub use ipam::service_template_repository::{
    SERVICE_TEMPLATE_COLUMNS, ServiceTemplateFilter, ServiceTemplateRepository,
    SqlxServiceTemplateRepository,
};
pub use ipam::vlan_group_repository::{
    BulkVlanGroupResult, NewVlanGroup, SqlxVlanGroupRepository, UpdateVlanGroup,
    VLAN_GROUP_COLUMNS, VlanGroupFilter, VlanGroupRecord, VlanGroupRepository,
};
pub use ipam::vlan_repository::{
    NewVlan, SqlxVlanRepository, UpdateVlan, VLAN_COLUMNS, VlanFilter, VlanListItem, VlanRecord,
    VlanRepository,
};
pub use ipam::vrf_repository::{
    NewVrf, SqlxVrfRepository, UpdateVrf, VRF_COLUMNS, VrfListItem, VrfRecord, VrfRepository,
};
pub use native::discovery_repository::{DiscoveryRepository, SqlxDiscoveryRepository};
pub use native::ipam_repository::{
    IP_ADDRESS_COLUMNS, IpAddressListFilter, IpAddressRecord, IpAddressRepository, NewIpAddress,
    SqlxIpAddressRepository, UpdateIpAddress, normalize_cidr,
};
pub use native::layout_repository::{LayoutRepository, SqlxLayoutRepository, SystemLayoutRow};
pub use native::template_repository::{
    INVENTORY_TEMPLATE_COLUMNS, PreparedTemplateStep, PreparedTemplateVariable,
    SqlxTemplateRepository, StepOutcome, StepOutcomeProducer, StepResourceExecutor,
    TemplateHeaderRow, TemplateIdRow, TemplateInsertRow, TemplateRepository, TemplateRunInput,
    TemplateRunListFilter, TemplateRunRow, TemplateRunStepRow, TemplateSnapshot, TemplateStepRow,
    TemplateUpdateRow, TemplateVariableRow, extract_step_key, parse_step_dependencies,
    resolve_step_payload, resolve_step_preview, resolve_step_previews, resolve_template_value,
    run_status_for, summarize_run,
};
pub use native::template_step_resource_repository::{
    SUPPORTED_RESOURCE_TYPES, SqlxTemplateStepResourceRepository, is_supported_resource_type,
    supported_resource_types_csv,
};
pub use object_ref_resolver::SqlxObjectRefResolver;
pub use repository::{
    ClusterFilter, ClusterGroupRepository, ClusterRepository, ClusterTypeRepository, DeviceFilter,
    DeviceListProjection, DeviceRepository, DeviceRoleRepository, DeviceTypeRepository,
    ManufacturerRepository, RegionRepository, SiteFilter, SiteGroupRepository, SiteRepository,
    TenantGroupRepository, TenantRepository, VirtualMachineFilter, VirtualMachineListProjection,
    VirtualMachineRepository,
};
pub use tenancy::contact_assignment_repository::{
    BulkContactAssignmentResult, CONTACT_ASSIGNMENT_COLUMNS, ContactAssignmentFilter,
    ContactAssignmentRecord, ContactAssignmentRepository, NewContactAssignment,
    SqlxContactAssignmentRepository, UpdateContactAssignment,
};
pub use tenancy::contact_group_repository::{
    BulkContactGroupResult, CONTACT_GROUP_COLUMNS, ContactGroupRecord, ContactGroupRepository,
    NewContactGroup, SqlxContactGroupRepository, UpdateContactGroup,
};
pub use tenancy::contact_repository::{
    CONTACT_COLUMNS, ContactRecord, ContactRepository, NewContact, SqlxContactRepository,
    UpdateContact,
};
pub use tenancy::contact_role_repository::{
    CONTACT_ROLE_COLUMNS, ContactRoleRecord, ContactRoleRepository, NewContactRole,
    SqlxContactRoleRepository, UpdateContactRole,
};
pub use tenancy::tenant_group_repository::SqlxTenantGroupRepository;
pub use tenancy::tenant_repository::{SqlxTenantRepository, TENANT_COLUMNS};
pub use virtualization::cluster_group_repository::SqlxClusterGroupRepository;
pub use virtualization::cluster_repository::{CLUSTER_COLUMNS, SqlxClusterRepository};
pub use virtualization::cluster_type_repository::SqlxClusterTypeRepository;
pub use virtualization::virtual_device_context_repository::{
    SqlxVirtualDeviceContextRepository, VIRTUAL_DEVICE_CONTEXT_COLUMNS, VirtualDeviceContextFilter,
    VirtualDeviceContextRepository,
};
pub use virtualization::virtual_disk_repository::{
    BulkVirtualDiskResult, NewVirtualDisk, SqlxVirtualDiskRepository, UpdateVirtualDisk,
    VIRTUAL_DISK_COLUMNS, VirtualDiskFilter, VirtualDiskRecord, VirtualDiskRepository,
};
pub use virtualization::virtual_machine_repository::{
    SqlxVirtualMachineRepository, VIRTUAL_MACHINE_COLUMNS,
};
pub use virtualization::virtual_machine_type_repository::{
    SqlxVirtualMachineTypeRepository, VIRTUAL_MACHINE_TYPE_COLUMNS, VirtualMachineTypeChanges,
    VirtualMachineTypeFilter, VirtualMachineTypeRepository,
};
pub use virtualization::vm_interface_repository::{
    BulkVmInterfaceResult, NewVmInterface, SqlxVmInterfaceRepository, UpdateVmInterface,
    VM_INTERFACE_COLUMNS, VmInterfaceRecord, VmInterfaceRepository,
};
pub use vpn::vpn_ike_policy_repository::{
    SqlxVpnIkePolicyRepository, VPN_IKE_POLICY_COLUMNS, VpnIkePolicyFilter, VpnIkePolicyRepository,
};
pub use vpn::vpn_ike_proposal_repository::{
    SqlxVpnIkeProposalRepository, VPN_IKE_PROPOSAL_COLUMNS, VpnIkeProposalFilter,
    VpnIkeProposalRepository,
};
pub use vpn::vpn_ipsec_policy_repository::{
    SqlxVpnIpsecPolicyRepository, VPN_IPSEC_POLICY_COLUMNS, VpnIpsecPolicyFilter,
    VpnIpsecPolicyRepository,
};
pub use vpn::vpn_ipsec_profile_repository::{
    SqlxVpnIpsecProfileRepository, VPN_IPSEC_PROFILE_COLUMNS, VpnIpsecProfileFilter,
    VpnIpsecProfileRepository,
};
pub use vpn::vpn_ipsec_proposal_repository::{
    SqlxVpnIpsecProposalRepository, VPN_IPSEC_PROPOSAL_COLUMNS, VpnIpsecProposalFilter,
    VpnIpsecProposalRepository,
};
pub use vpn::vpn_tunnel_group_repository::{
    SqlxVpnTunnelGroupRepository, VPN_TUNNEL_GROUP_COLUMNS, VpnTunnelGroupRepository,
};
pub use vpn::vpn_tunnel_repository::{
    SqlxVpnTunnelRepository, VPN_TUNNEL_COLUMNS, VpnTunnelFilter, VpnTunnelRepository,
};
pub use vpn::vpn_tunnel_termination_repository::{
    SqlxVpnTunnelTerminationRepository, VPN_TUNNEL_TERMINATION_COLUMNS, VpnTunnelTerminationFilter,
    VpnTunnelTerminationRepository,
};
