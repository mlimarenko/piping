//! Inventory persistence modules, list-query helpers, and their public re-exports.
pub mod list_query;
pub mod repositories;
pub use repositories::{
    OWNER_COLUMNS, OWNER_GROUP_COLUMNS, OwnerChanges, OwnerFilter, OwnerGroupRepository,
    OwnerRepository, SqlxOwnerGroupRepository, SqlxOwnerRepository,
};

pub use list_query::{
    CustomFieldListFilter, ListColumn, MAX_FILTER_VALUES, MAX_ID_FILTER_VALUES, SortSpec,
    parse_csv_strings, parse_csv_uuids, parse_custom_field_filters, parse_id_filter,
    push_custom_field_filters, push_id_filter, push_order_by, push_string_in, push_uuid_in,
};
pub use repositories::{
    AGGREGATE_COLUMNS, ASN_COLUMNS, ASN_RANGE_COLUMNS, AggregateListItem, AggregateRecord,
    AggregateRepository, AsnListItem, AsnRecord, AsnRepository, BulkAggregateResult,
    BulkCableResult, BulkColumn, BulkContactAssignmentResult, BulkContactGroupResult,
    BulkCreateResult, BulkInterfaceResult, BulkIpRangeResult, BulkLocationResult, BulkPrefixResult,
    BulkRackResult, BulkRackTypeResult, BulkResourceRepository, BulkRouteTargetResult,
    BulkVirtualDiskResult, BulkVlanGroupResult, BulkVmInterfaceResult, CABLE_COLUMNS,
    CIRCUIT_TERMINATION_COLUMNS, CIRCUIT_TYPE_COLUMNS, CLUSTER_COLUMNS, CONTACT_ASSIGNMENT_COLUMNS,
    CONTACT_COLUMNS, CONTACT_GROUP_COLUMNS, CONTACT_ROLE_COLUMNS, CableEndpointKind, CableFilter,
    CableRecord, CableRepository, CablingCableSummary, CablingEndpointFilter, CablingEndpointItem,
    CablingEndpointRef, CablingIssueCode, CablingLinkState, CablingLocation, CablingRepository,
    CablingResourceLink, CablingTrace, CablingTraceIssue, CablingTraceSegment, CablingTraceState,
    CablingTraceSummary, CablingTraversal, ClusterFilter, ClusterGroupRepository,
    ClusterRepository, ClusterTypeRepository, CollectedDiskFacts, CollectedFactsIngestRequest,
    CollectedFactsIngestResponse, CollectedFactsInventoryKind, CollectedFactsWritePolicy,
    CollectedHardwareFacts, CollectedInterfaceFacts, CollectedPlatformFacts,
    ConfigContextBindingListFilters, ConfigContextRepository, ConnectionTargetRepository,
    ConsolePortRecord, ConsolePortRepository, ContactAssignmentRecord, ContactAssignmentRepository,
    ContactGroupRecord, ContactGroupRepository, ContactRecord, ContactRepository,
    ContactRoleRecord, ContactRoleRepository, DeviceFilter, DeviceListProjection, DeviceRepository,
    DeviceRoleRepository, DeviceTypeRepository, DiscoveryRepository, INTERFACE_COLUMNS,
    IP_ADDRESS_COLUMNS, IP_RANGE_COLUMNS, IPAM_ROLE_COLUMNS, InterfaceRecord, InterfaceRepository,
    IpAddressListFilter, IpAddressRecord, IpAddressRepository, IpRangeFilter, IpRangeListItem,
    IpRangeRecord, IpRangeRepository, IpamRoleRecord, IpamRoleRepository, LayoutRepository,
    LocationListItem, LocationRecord, LocationRepository, MAC_ADDRESS_COLUMNS,
    MANUFACTURER_COLUMNS, MacAddressFilter, ManufacturerRepository, NewAggregate, NewAsn, NewCable,
    NewConsolePort, NewContact, NewContactAssignment, NewContactGroup, NewContactRole,
    NewInterface, NewIpAddress, NewIpRange, NewIpamRole, NewLocation, NewPlatform, NewPrefix,
    NewRack, NewRackRole, NewRackType, NewRir, NewRouteTarget, NewService, NewVirtualDisk, NewVlan,
    NewVlanGroup, NewVmInterface, NewVrf, PLATFORM_COLUMNS, PlatformListItem, PlatformRecord,
    PlatformRepository, PrefixFilter, PrefixListItem, PrefixPlanningPreview, PrefixRecord,
    PrefixRepository, PrefixTreeFilter, PrefixTreeNode, PrefixTreeReadModel, RIR_COLUMNS,
    ROUTE_TARGET_COLUMNS, RackFilter, RackListItem, RackRecord, RackRepository, RackRoleRecord,
    RackRoleRepository, RackTypeFilter, RackTypeListItem, RackTypeRecord, RackTypeRepository,
    RegionRepository, RirRecord, RirRepository, RouteTargetListItem, RouteTargetRecord,
    RouteTargetRepository, SERVICE_COLUMNS, ServiceRecord, ServiceRepository,
    ServiceTemplateFilter, SiteFilter, SiteGroupRepository, SiteRepository,
    SqlxAggregateRepository, SqlxAsnRepository, SqlxBulkResourceRepository, SqlxCableRepository,
    SqlxCablingRepository, SqlxClusterGroupRepository, SqlxClusterRepository,
    SqlxClusterTypeRepository, SqlxCollectedFactsRepository, SqlxConfigContextRepository,
    SqlxConnectionTargetRepository, SqlxConsolePortRepository, SqlxContactAssignmentRepository,
    SqlxContactGroupRepository, SqlxContactRepository, SqlxContactRoleRepository,
    SqlxDeviceRepository, SqlxDeviceRoleRepository, SqlxDeviceTypeRepository,
    SqlxDiscoveryRepository, SqlxInterfaceRepository, SqlxIpAddressRepository,
    SqlxIpRangeRepository, SqlxIpamRoleRepository, SqlxLayoutRepository, SqlxLocationRepository,
    SqlxManufacturerRepository, SqlxPlatformRepository, SqlxPrefixRepository, SqlxRackRepository,
    SqlxRackRoleRepository, SqlxRackTypeRepository, SqlxRegionRepository, SqlxRirRepository,
    SqlxRouteTargetRepository, SqlxServiceRepository, SqlxSiteGroupRepository, SqlxSiteRepository,
    SqlxTemplateRepository, SqlxTemplateStepResourceRepository, SqlxTenantGroupRepository,
    SqlxTenantRepository, SqlxVirtualDiskRepository, SqlxVirtualMachineRepository,
    SqlxVirtualMachineTypeRepository, SqlxVlanGroupRepository, SqlxVlanRepository,
    SqlxVmInterfaceRepository, SqlxVrfRepository, TemplateRepository, TenantGroupRepository,
    TenantRepository, UpdateAggregate, UpdateAsn, UpdateCable, UpdateConsolePort, UpdateContact,
    UpdateContactAssignment, UpdateContactGroup, UpdateContactRole, UpdateInterface,
    UpdateIpAddress, UpdateIpRange, UpdateIpamRole, UpdateLocation, UpdatePlatform, UpdatePrefix,
    UpdateRack, UpdateRackRole, UpdateRackType, UpdateRir, UpdateRouteTarget, UpdateService,
    UpdateVirtualDisk, UpdateVlan, UpdateVlanGroup, UpdateVmInterface, UpdateVrf,
    VIRTUAL_DISK_COLUMNS, VIRTUAL_MACHINE_TYPE_COLUMNS, VM_INTERFACE_COLUMNS, VRF_COLUMNS,
    VirtualDeviceContextFilter, VirtualDiskFilter, VirtualDiskRecord, VirtualDiskRepository,
    VirtualMachineFilter, VirtualMachineListProjection, VirtualMachineRepository,
    VirtualMachineTypeChanges, VirtualMachineTypeFilter, VirtualMachineTypeRepository, VlanFilter,
    VlanGroupRecord, VlanGroupRepository, VlanListItem, VlanRecord, VlanRepository,
    VmInterfaceRecord, VmInterfaceRepository, VrfListItem, VrfRecord, VrfRepository,
    normalize_cidr,
};
pub use repositories::{
    AsnRangeRecord, AsnRangeRepository, BulkAsnRangeResult, NewAsnRange, SqlxAsnRangeRepository,
    UpdateAsnRange,
};
pub use repositories::{
    BulkCircuitResult, CircuitListItem, CircuitRecord, CircuitRepository, NewCircuit,
    SqlxCircuitRepository, UpdateCircuit,
};
pub use repositories::{
    BulkCircuitTypeResult, CircuitTypeRecord, CircuitTypeRepository, NewCircuitType,
    SqlxCircuitTypeRepository, UpdateCircuitType,
};
pub use repositories::{
    BulkConsoleServerPortResult, CONSOLE_SERVER_PORT_COLUMNS, ConsoleServerPortFilter,
    ConsoleServerPortRecord, ConsoleServerPortRepository, NewConsoleServerPort,
    SqlxConsoleServerPortRepository, UpdateConsoleServerPort,
};
pub use repositories::{
    BulkDeviceBayResult, DEVICE_BAY_COLUMNS, DeviceBayFilter, DeviceBayRecord, DeviceBayRepository,
    NewDeviceBay, SqlxDeviceBayRepository, UpdateDeviceBay,
};
pub use repositories::{
    BulkFrontPortResult, FRONT_PORT_COLUMNS, FrontPortFilter, FrontPortRecord, FrontPortRepository,
    NewFrontPort, SqlxFrontPortRepository, UpdateFrontPort,
};
pub use repositories::{
    BulkInventoryItemResult, INVENTORY_ITEM_COLUMNS, InventoryItemFilter, InventoryItemRecord,
    InventoryItemRepository, NewInventoryItem, SqlxInventoryItemRepository, UpdateInventoryItem,
};
pub use repositories::{
    BulkInventoryItemRoleResult, INVENTORY_ITEM_ROLE_COLUMNS, InventoryItemRoleRecord,
    InventoryItemRoleRepository, NewInventoryItemRole, SqlxInventoryItemRoleRepository,
    UpdateInventoryItemRole,
};
pub use repositories::{
    BulkModuleBayResult, MODULE_BAY_COLUMNS, ModuleBayFilter, ModuleBayRecord, ModuleBayRepository,
    NewModuleBay, SqlxModuleBayRepository, UpdateModuleBay,
};
pub use repositories::{
    BulkModuleResult, MODULE_COLUMNS, ModuleRecord, ModuleRepository, NewModule,
    SqlxModuleRepository, UpdateModule,
};
pub use repositories::{
    BulkPowerOutletResult, NewPowerOutlet, POWER_OUTLET_COLUMNS, PowerOutletFilter,
    PowerOutletRecord, PowerOutletRepository, SqlxPowerOutletRepository, UpdatePowerOutlet,
};
pub use repositories::{
    BulkPowerPortResult, NewPowerPort, POWER_PORT_COLUMNS, PowerPortFilter, PowerPortRecord,
    PowerPortRepository, SqlxPowerPortRepository, UpdatePowerPort,
};
pub use repositories::{
    BulkProviderNetworkResult, NewProviderNetwork, ProviderNetworkListItem, ProviderNetworkRecord,
    ProviderNetworkRepository, SqlxProviderNetworkRepository, UpdateProviderNetwork,
};
pub use repositories::{
    BulkRearPortResult, NewRearPort, REAR_PORT_COLUMNS, RearPortFilter, RearPortRecord,
    RearPortRepository, SqlxRearPortRepository, UpdateRearPort,
};
pub use repositories::{
    BulkVirtualChassisResult, NewVirtualChassis, SqlxVirtualChassisRepository,
    UpdateVirtualChassis, VIRTUAL_CHASSIS_COLUMNS, VirtualChassisRecord, VirtualChassisRepository,
};
pub use repositories::{
    CIRCUIT_COLUMNS, DEVICE_COLUMNS, DEVICE_TYPE_COLUMNS, PREFIX_COLUMNS, PROVIDER_ACCOUNT_COLUMNS,
    PROVIDER_NETWORK_COLUMNS, RACK_COLUMNS, REGION_COLUMNS, SITE_COLUMNS, TENANT_COLUMNS,
    VIRTUAL_MACHINE_COLUMNS, VLAN_COLUMNS,
};
pub use repositories::{
    CIRCUIT_GROUP_COLUMNS, CircuitGroupRepository, CircuitTerminationFilter,
    CircuitTerminationRepository, MacAddressRepository, POWER_FEED_COLUMNS, POWER_PANEL_COLUMNS,
    PowerFeedRepository, PowerPanelRepository, ProviderAccountListItem, ProviderAccountRepository,
    RACK_RESERVATION_COLUMNS, RackReservationRepository, SERVICE_TEMPLATE_COLUMNS,
    ServiceTemplateRepository, SqlxCircuitGroupRepository, SqlxCircuitTerminationRepository,
    SqlxMacAddressRepository, SqlxPowerFeedRepository, SqlxPowerPanelRepository,
    SqlxProviderAccountRepository, SqlxRackReservationRepository, SqlxServiceTemplateRepository,
    SqlxVirtualCircuitRepository, SqlxVirtualCircuitTypeRepository,
    SqlxVirtualDeviceContextRepository, VIRTUAL_CIRCUIT_COLUMNS, VIRTUAL_CIRCUIT_TYPE_COLUMNS,
    VIRTUAL_DEVICE_CONTEXT_COLUMNS, VirtualCircuitFilter, VirtualCircuitListItem,
    VirtualCircuitRepository, VirtualCircuitTypeRepository, VirtualDeviceContextRepository,
};
pub use repositories::{CircuitFilter, ContactAssignmentFilter, LocationFilter};
pub use repositories::{
    ContentTypeRow, SqlxObjectRefResolver, list_content_types, lookup_content_type_id,
    lookup_content_type_id_tx,
};
pub use repositories::{MODULE_TYPE_COLUMNS, ModuleTypeRepository, SqlxModuleTypeRepository};
pub use repositories::{MODULE_TYPE_PROFILE_COLUMNS, SqlxModuleTypeProfileRepository};
pub use repositories::{PROVIDER_COLUMNS, ProviderRepository, SqlxProviderRepository};
pub use repositories::{
    SqlxVpnIkePolicyRepository, SqlxVpnIkeProposalRepository, SqlxVpnIpsecPolicyRepository,
    SqlxVpnIpsecProfileRepository, SqlxVpnIpsecProposalRepository, SqlxVpnTunnelGroupRepository,
    SqlxVpnTunnelRepository, SqlxVpnTunnelTerminationRepository, VPN_IKE_POLICY_COLUMNS,
    VPN_IKE_PROPOSAL_COLUMNS, VPN_IPSEC_POLICY_COLUMNS, VPN_IPSEC_PROFILE_COLUMNS,
    VPN_IPSEC_PROPOSAL_COLUMNS, VPN_TUNNEL_COLUMNS, VPN_TUNNEL_GROUP_COLUMNS,
    VPN_TUNNEL_TERMINATION_COLUMNS, VpnIkePolicyFilter, VpnIkePolicyRepository,
    VpnIkeProposalFilter, VpnIkeProposalRepository, VpnIpsecPolicyFilter, VpnIpsecPolicyRepository,
    VpnIpsecProfileFilter, VpnIpsecProfileRepository, VpnIpsecProposalFilter,
    VpnIpsecProposalRepository, VpnTunnelFilter, VpnTunnelGroupRepository, VpnTunnelRepository,
    VpnTunnelTerminationFilter, VpnTunnelTerminationRepository,
};
