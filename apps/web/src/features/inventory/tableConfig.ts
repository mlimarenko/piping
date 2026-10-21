/**
 * Inventory DataTable capability registry.
 *
 * Backend sort keys are owned by repository-local `*_COLUMNS` registries.
 * This file mirrors only the public table contract the frontend needs: route
 * sort keys, safe filters that are actually handled by list endpoints today,
 * and field-key normalization for rendered columns.
 */
import type { InventoryEntityKey } from "@/shared/api/types";
import type {
  ChoiceSet,
  ContentType,
  CustomFieldDefinition,
} from "@/features/inventory/custom-fields-api";
import { statusOptions } from "@/features/inventory/components/inventory/inventoryTypes";

export interface InventoryFilterEnumValue {
  value: string;
  label: string;
}

export type InventoryFilterColumn =
  | {
      colKey: string;
      param: string;
      type: "enum";
      enumValues: InventoryFilterEnumValue[];
    }
  | {
      colKey: string;
      param: string;
      type: "fk";
      fkEntity: InventoryEntityKey | InventoryEntityKey[];
    };

export interface InventoryIdentityColumn {
  key: string;
  label: string;
  fieldKey?: string;
}

export interface InventoryTableConfig {
  /** Displayed relation collections / filter-only FKs without server ordering. */
  unsortableKeys?: string[];
  identity?: InventoryIdentityColumn;
  sortKeys: string[];
  filterColumns: InventoryFilterColumn[];
  columnKeyOverrides?: Record<string, string>;
  /**
   * Non-field render columns owned by this entity (utilization/portMappings/
   * observability/schemaSummary). The render component reads these to decide
   * which bespoke column to mount, keeping the "which entity gets which
   * special column" contract beside the sort/filter metadata instead of
   * hardcoding it inline.
   */
  specialColumnKeys?: readonly string[];
}

export const CUSTOM_FIELD_FILTER_ENTITY_SCOPE = ["Sites", "Devices", "Clusters"] as const;
export type CustomFieldFilterEntityScope = (typeof CUSTOM_FIELD_FILTER_ENTITY_SCOPE)[number];

export const CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY = {
  Sites: "dcim.site",
  Devices: "dcim.device",
  Clusters: "virtualization.cluster",
  Regions: "dcim.region",
  Tenants: "tenancy.tenant",
  "Virtual Machines": "virtualization.virtualmachine",
  Interfaces: "dcim.interface",
  "Inventory Items": "dcim.inventoryitem",
  "IP Addresses": "ipam.ipaddress",
  Prefixes: "ipam.prefix",
  Services: "ipam.service",
  "VM Interfaces": "virtualization.vminterface",
  "Virtual Disks": "virtualization.virtualdisk",
} as const satisfies Partial<Record<InventoryEntityKey, string>>;

interface InventoryFieldLike {
  tableColumn?: boolean;
  key: string;
  idKey?: string;
  type?: string;
}

export const EMPTY_CONFIG: InventoryTableConfig = { sortKeys: [], filterColumns: [] };

const cfg = (
  sortKeys: string[],
  filterColumns: InventoryFilterColumn[] = [],
  columnKeyOverrides?: Record<string, string>,
  identity?: InventoryIdentityColumn,
): InventoryTableConfig => ({ identity, sortKeys, filterColumns, columnKeyOverrides });

const identity = (key: string, label: string, fieldKey?: string): InventoryIdentityColumn => ({
  key,
  label,
  fieldKey,
});

const statusFilter = (): InventoryFilterColumn => ({
  colKey: "status",
  param: "status",
  type: "enum",
  enumValues: statusOptions,
});

const boolFilter = (colKey: string, param = colKey): InventoryFilterColumn => ({
  colKey,
  param,
  type: "enum",
  enumValues: [
    { value: "true", label: "True" },
    { value: "false", label: "False" },
  ],
});

const enumFilter = (
  colKey: string,
  param: string,
  enumValues: InventoryFilterEnumValue[],
): InventoryFilterColumn => ({ colKey, param, type: "enum", enumValues });

const fkFilter = (
  colKey: string,
  param: string,
  fkEntity: InventoryEntityKey | InventoryEntityKey[],
): InventoryFilterColumn => ({ colKey, param, type: "fk", fkEntity });

const objectRefFilter = (
  colKey: string,
  param: string,
  fkEntity: InventoryEntityKey[],
): InventoryFilterColumn => fkFilter(colKey, param, fkEntity);

const customFieldColumnKey = (name: string): string => `cf_${name}`;

const contentTypeEntityById = (
  contentTypes: ContentType[],
): Map<string, InventoryEntityKey> => {
  const byToken = new Map(contentTypes.map((type) => [type.token, type.id]));
  const entityById = new Map<string, InventoryEntityKey>();
  for (const [entity, token] of Object.entries(CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY)) {
    const id = byToken.get(token);
    if (id) entityById.set(id, entity as InventoryEntityKey);
  }
  return entityById;
};

export function customFieldContentTypeTokenFor(
  entity: InventoryEntityKey | null,
): string | null {
  if (!entity || !(entity in CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY)) return null;
  return CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY[entity as keyof typeof CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY];
}

export function inventoryEntitySupportsCustomFieldFilters(
  entity: InventoryEntityKey | null,
): entity is CustomFieldFilterEntityScope {
  return entity !== null && CUSTOM_FIELD_FILTER_ENTITY_SCOPE.includes(entity as CustomFieldFilterEntityScope);
}

export function inventoryCustomFieldColumnKey(definition: Pick<CustomFieldDefinition, "name">): string {
  return customFieldColumnKey(definition.name);
}

export function inventoryCustomFieldDataColumns(
  definitions: CustomFieldDefinition[],
): CustomFieldDefinition[] {
  return definitions.filter((definition) => definition.ui_visible !== "hidden" && !definition.is_secret);
}

export function inventoryTableConfigWithCustomFields(
  base: InventoryTableConfig,
  entity: InventoryEntityKey | null,
  definitions: CustomFieldDefinition[],
  choiceSets: ChoiceSet[],
  contentTypes: ContentType[],
): InventoryTableConfig {
  if (!inventoryEntitySupportsCustomFieldFilters(entity)) return base;

  const choiceSetById = new Map(choiceSets.map((choiceSet) => [choiceSet.id, choiceSet]));
  const entityByContentTypeId = contentTypeEntityById(contentTypes);
  const dynamicFilters: InventoryFilterColumn[] = [];

  for (const definition of definitions) {
    if (definition.filter_logic === "disabled") continue;

    const colKey = customFieldColumnKey(definition.name);
    if (definition.is_secret) {
      dynamicFilters.push(
        enumFilter(colKey, `${colKey}__present`, [
          { value: "true", label: "customFields.filters.present" },
          { value: "false", label: "customFields.filters.absent" },
        ]),
      );
      continue;
    }

    switch (definition.field_type) {
      case "boolean":
        dynamicFilters.push(boolFilter(colKey, colKey));
        break;
      case "select":
      case "multiselect": {
        const choiceSet = definition.choice_set_id
          ? choiceSetById.get(definition.choice_set_id)
          : undefined;
        if (!choiceSet) break;
        dynamicFilters.push(
          enumFilter(
            colKey,
            colKey,
            choiceSet.extra_choices.map((choice) => ({
              value: choice.value,
              label: choice.label,
            })),
          ),
        );
        break;
      }
      case "object":
      case "multiobject": {
        const fkEntity = definition.related_object_type_id
          ? entityByContentTypeId.get(definition.related_object_type_id)
          : undefined;
        if (!fkEntity) break;
        dynamicFilters.push(fkFilter(colKey, colKey, fkEntity));
        break;
      }
      default:
        break;
    }
  }

  if (dynamicFilters.length === 0) return base;
  return {
    ...base,
    filterColumns: [...base.filterColumns, ...dynamicFilters],
  };
}

const interfaceTypeOptions: InventoryFilterEnumValue[] = [
  { value: "virtual", label: "Virtual" },
  { value: "other", label: "Other" },
];

const vmInterfaceModeOptions: InventoryFilterEnumValue[] = [
  { value: "access", label: "Access" },
  { value: "tagged", label: "Tagged" },
  { value: "tagged-all", label: "Tagged all" },
];

const rackTypeFormFactorOptions: InventoryFilterEnumValue[] = [
  { value: "2-post-frame", label: "2-post frame" },
  { value: "4-post-frame", label: "4-post frame" },
  { value: "4-post-cabinet", label: "4-post cabinet" },
  { value: "wall-frame", label: "Wall frame" },
  { value: "wall-frame-vertical", label: "Wall frame vertical" },
  { value: "wall-cabinet", label: "Wall cabinet" },
  { value: "wall-cabinet-vertical", label: "Wall cabinet vertical" },
];

const rackTypeOuterUnitOptions: InventoryFilterEnumValue[] = [
  { value: "mm", label: "mm" },
  { value: "in", label: "in" },
];

const weightUnitOptions: InventoryFilterEnumValue[] = [
  { value: "kg", label: "kg" },
  { value: "g", label: "g" },
  { value: "lb", label: "lb" },
  { value: "oz", label: "oz" },
];

const vpnModeOptions: InventoryFilterEnumValue[] = [
  { value: "main", label: "Main" },
  { value: "aggressive", label: "Aggressive" },
];

const tunnelEncapsulationOptions: InventoryFilterEnumValue[] = [
  { value: "ipsec", label: "IPsec" },
  { value: "gre", label: "GRE" },
  { value: "ip-in-ip", label: "IP-in-IP" },
  { value: "wireguard", label: "WireGuard" },
];

const tunnelTerminationRoleOptions: InventoryFilterEnumValue[] = [
  { value: "hub", label: "Hub" },
  { value: "spoke", label: "Spoke" },
  { value: "peer", label: "Peer" },
];

const ikeAuthenticationMethodOptions: InventoryFilterEnumValue[] = [
  { value: "pre-shared-keys", label: "Pre-shared keys" },
  { value: "rsa-signatures", label: "RSA signatures" },
];

const ikeEncryptionAlgorithmOptions: InventoryFilterEnumValue[] = [
  { value: "aes-128-cbc", label: "AES-128-CBC" },
  { value: "aes-192-cbc", label: "AES-192-CBC" },
  { value: "aes-256-cbc", label: "AES-256-CBC" },
  { value: "aes-128-gcm", label: "AES-128-GCM" },
  { value: "aes-256-gcm", label: "AES-256-GCM" },
];

const ikeAuthenticationAlgorithmOptions: InventoryFilterEnumValue[] = [
  { value: "sha1", label: "SHA1" },
  { value: "sha256", label: "SHA256" },
  { value: "sha384", label: "SHA384" },
  { value: "sha512", label: "SHA512" },
];

const ipsecEncryptionAlgorithmOptions: InventoryFilterEnumValue[] = [
  { value: "aes-128-cbc", label: "AES-128-CBC" },
  { value: "aes-192-cbc", label: "AES-192-CBC" },
  { value: "aes-256-cbc", label: "AES-256-CBC" },
  { value: "aes-128-gcm", label: "AES-128-GCM" },
  { value: "aes-256-gcm", label: "AES-256-GCM" },
];

const ipsecAuthenticationAlgorithmOptions: InventoryFilterEnumValue[] = [
  { value: "hmac-sha1", label: "HMAC-SHA1" },
  { value: "hmac-sha256", label: "HMAC-SHA256" },
  { value: "hmac-sha384", label: "HMAC-SHA384" },
  { value: "hmac-sha512", label: "HMAC-SHA512" },
];

const cableTypeOptions: InventoryFilterEnumValue[] = [
  { value: "cat3", label: "Cat 3" },
  { value: "cat5", label: "Cat 5" },
  { value: "cat5e", label: "Cat 5e" },
  { value: "cat6", label: "Cat 6" },
  { value: "cat6a", label: "Cat 6a" },
  { value: "cat7", label: "Cat 7" },
  { value: "cat7a", label: "Cat 7a" },
  { value: "cat8", label: "Cat 8" },
  { value: "dac-active", label: "DAC active" },
  { value: "dac-passive", label: "DAC passive" },
  { value: "mmf", label: "MMF" },
  { value: "smf", label: "SMF" },
  { value: "mrj21-trunk", label: "MRJ21 trunk" },
  { value: "coaxial", label: "Coaxial" },
  { value: "power", label: "Power" },
  { value: "usb", label: "USB" },
  { value: "other", label: "Other" },
];

const cableLengthUnitOptions: InventoryFilterEnumValue[] = [
  { value: "m", label: "Meters" },
  { value: "cm", label: "Centimeters" },
  { value: "ft", label: "Feet" },
  { value: "in", label: "Inches" },
];

const circuitTerminationSideOptions: InventoryFilterEnumValue[] = [
  { value: "A", label: "A" },
  { value: "Z", label: "Z" },
];

const powerFeedTypeOptions: InventoryFilterEnumValue[] = [
  { value: "primary", label: "Primary" },
  { value: "redundant", label: "Redundant" },
];

const powerFeedSupplyOptions: InventoryFilterEnumValue[] = [
  { value: "ac", label: "AC" },
  { value: "dc", label: "DC" },
];

const powerFeedPhaseOptions: InventoryFilterEnumValue[] = [
  { value: "single-phase", label: "Single phase" },
  { value: "three-phase", label: "Three phase" },
];

const contactAssignmentPriorityOptions: InventoryFilterEnumValue[] = [
  { value: "primary", label: "Primary" },
  { value: "secondary", label: "Secondary" },
  { value: "tertiary", label: "Tertiary" },
  { value: "inactive", label: "Inactive" },
];

const serviceProtocolFilter = (): InventoryFilterColumn => ({
  colKey: "protocol",
  param: "protocol",
  type: "enum",
  enumValues: [
    { value: "tcp", label: "TCP" },
    { value: "udp", label: "UDP" },
    { value: "sctp", label: "SCTP" },
  ],
});

const INVENTORY_TABLE_CONFIG: Partial<Record<InventoryEntityKey, InventoryTableConfig>> = {
  Owners: {
    ...cfg(["name", "description"], [fkFilter("group_id", "group_id", "Owner Groups")], { groupName: "group_id", userGroups: "user_groups" }),
    unsortableKeys: ["group_id", "users", "user_groups"],
  },
  "Owner Groups": cfg(["name", "description"]),
  Devices: {
    ...cfg(
      [
        "name",
        "status",
        "site",
        "role",
        "device_type",
        "platform",
        "tenant",
        "rack",
        "primary_ip",
        "serial",
        "asset_tag",
        "position",
        "face",
        "airflow",
      ],
      [
        statusFilter(),
        fkFilter("site", "site_id", "Sites"),
        fkFilter("role", "role_id", "Device Roles"),
        fkFilter("device_type", "device_type_id", "Device Types"),
        fkFilter("platform", "platform_id", "Platforms"),
        fkFilter("tenant", "tenant_id", "Tenants"),
        fkFilter("rack", "rack_id", "Racks"),
      ],
      { deviceType: "device_type", rackName: "rack", primaryIp: "primary_ip", assetTag: "asset_tag" },
    ),
    specialColumnKeys: ["observability"],
  },
  Sites: cfg(
    ["name", "status", "region", "site_group", "tenant", "facility", "time_zone", "latitude", "longitude"],
    [
      statusFilter(),
      fkFilter("region", "region_id", "Regions"),
      fkFilter("site_group", "site_group_id", "Site Groups"),
      fkFilter("tenant", "tenant_id", "Tenants"),
    ],
    { siteGroupName: "site_group", timeZone: "time_zone" },
  ),
  Clusters: cfg(
    ["name", "status", "cluster_type", "cluster_group", "site", "tenant", "description", "comments"],
    [
      statusFilter(),
      fkFilter("cluster_type", "cluster_type_id", "Cluster Types"),
      fkFilter("cluster_group", "cluster_group_id", "Cluster Groups"),
      fkFilter("site", "site_id", "Sites"),
      fkFilter("tenant", "tenant_id", "Tenants"),
    ],
    { clusterTypeName: "cluster_type", clusterGroupName: "cluster_group" },
  ),
  "Device Types": cfg(
    ["model", "manufacturer"],
    [fkFilter("manufacturer", "manufacturer_id", "Manufacturers")],
    { manufacturerName: "manufacturer" },
    identity("model", "Model"),
  ),
  Racks: cfg(
    [
      "name",
      "status",
      "site",
      "tenant",
      "rack_role",
      "rack_type",
      "facility_id",
      "serial",
      "asset_tag",
      "width",
      "u_height",
      "outer_width",
      "outer_depth",
      "desc_units",
    ],
    [
      statusFilter(),
      fkFilter("site", "site_id", "Sites"),
      fkFilter("tenant", "tenant_id", "Tenants"),
      fkFilter("rack_role", "role_id", "Rack Roles"),
      boolFilter("desc_units"),
    ],
    {
      rackRoleName: "rack_role",
      rackType: "rack_type",
      facilityId: "facility_id",
      assetTag: "asset_tag",
      uHeight: "u_height",
      outerWidth: "outer_width",
      outerDepth: "outer_depth",
      descUnits: "desc_units",
    },
  ),
  Prefixes: {
    ...cfg(
      ["prefix", "status", "site", "vrf", "tenant", "is_pool", "mark_utilized", "utilization"],
      [
        statusFilter(),
        fkFilter("site", "site_id", "Sites"),
        fkFilter("vrf", "vrf_id", "VRFs"),
        fkFilter("tenant", "tenant_id", "Tenants"),
        boolFilter("is_pool"),
        boolFilter("mark_utilized"),
      ],
      { vrfName: "vrf", isPool: "is_pool", markUtilized: "mark_utilized" },
      identity("prefix", "inventory.resources.fields.prefix", "prefix"),
    ),
    specialColumnKeys: ["utilization"],
  },
  VLANs: cfg(
    ["vid", "name", "status", "site", "tenant"],
    [statusFilter(), fkFilter("site", "site_id", "Sites"), fkFilter("tenant", "tenant_id", "Tenants")],
  ),
  "Virtual Machines": {
    ...cfg(
      ["name", "status", "site", "cluster", "role", "platform", "virtual_machine_type", "tenant", "primary_ip", "vcpus", "memory", "disk"],
      [
        statusFilter(),
        fkFilter("site", "site_id", "Sites"),
        fkFilter("cluster", "cluster_id", "Clusters"),
        fkFilter("role", "role_id", "Device Roles"),
        fkFilter("platform", "platform_id", "Platforms"),
        fkFilter("virtual_machine_type", "virtual_machine_type_id", "Virtual Machine Types"),
        fkFilter("tenant", "tenant_id", "Tenants"),
      ],
      { primaryIp: "primary_ip", virtualMachineType: "virtual_machine_type" },
    ),
    specialColumnKeys: ["observability"],
  },
  Regions: cfg(["name", "parent"], [fkFilter("parent", "parent_id", "Regions")], { parentName: "parent" }),
  Tenants: cfg(["name", "tenant_group"], [fkFilter("tenant_group", "group_id", "Tenant Groups")], { groupName: "tenant_group" }),
  "Tenant Groups": cfg(["name", "slug", "parent_id", "description"], [fkFilter("parent_id", "parent_id", "Tenant Groups")], {
    parentName: "parent_id",
  }),
  "Site Groups": cfg(["name", "slug", "parent_id", "description"], [fkFilter("parent_id", "parent_id", "Site Groups")], {
    parentName: "parent_id",
  }),
  "Device Roles": cfg(
    ["external_id", "name", "slug", "color", "vm_role", "description", "tags", "piping_synced_at"],
    [boolFilter("vm_role")],
    { vmRole: "vm_role", pipingSyncedAt: "piping_synced_at" },
  ),
  Platforms: cfg(["name", "slug", "manufacturer", "napalm_driver", "description"], [fkFilter("manufacturer", "manufacturer_id", "Manufacturers")], {
    manufacturerName: "manufacturer",
    napalmDriver: "napalm_driver",
  }),
  "Provider Networks": cfg(
    ["name", "provider", "service_id", "description", "comments"],
    [fkFilter("provider", "provider_id", "Providers")],
    { providerName: "provider", serviceId: "service_id" },
  ),
  Circuits: cfg(
    [
      "cid",
      "status",
      "provider",
      "circuit_type",
      "tenant",
      "install_date",
      "termination_date",
      "commit_rate",
      "distance",
      "distance_unit",
      "description",
      "comments",
    ],
    [
      statusFilter(),
      fkFilter("provider", "provider_id", "Providers"),
      fkFilter("circuit_type", "type_id", "Circuit Types"),
      fkFilter("tenant", "tenant_id", "Tenants"),
    ],
    {
      providerName: "provider",
      circuitTypeName: "circuit_type",
      installDate: "install_date",
      terminationDate: "termination_date",
      commitRate: "commit_rate",
      distanceUnit: "distance_unit",
    },
    identity("cid", "CID"),
  ),
  "Provider Accounts": cfg(
    ["name", "provider", "account", "description", "comments"],
    [fkFilter("provider", "provider_id", "Providers")],
    { providerName: "provider" },
  ),
  "Virtual Circuits": cfg(
    ["cid", "status", "provider_network", "virtual_circuit_type", "provider_account", "tenant", "description", "comments"],
    [
      statusFilter(),
      fkFilter("provider_network", "provider_network_id", "Provider Networks"),
      fkFilter("virtual_circuit_type", "type_id", "Virtual Circuit Types"),
      fkFilter("provider_account", "provider_account_id", "Provider Accounts"),
      fkFilter("tenant", "tenant_id", "Tenants"),
    ],
    {
      providerNetworkName: "provider_network",
      virtualCircuitTypeName: "virtual_circuit_type",
      providerName: "provider_account",
    },
    identity("cid", "CID"),
  ),

  Contacts: cfg(
    ["name", "group", "title", "phone", "email", "address", "link", "description"],
    [fkFilter("group", "group_id", "Contact Groups")],
    { groupName: "group" },
  ),
  "Contact Roles": cfg(["name", "slug", "description"]),
  "Contact Groups": cfg(["name", "slug", "parent", "description"], [fkFilter("parent", "parent_id", "Contact Groups")]),
  "Contact Assignments": cfg(
    ["contentType", "objectId", "contact", "role", "priority"],
    [
      fkFilter("contact", "contact_id", "Contacts"),
      fkFilter("role", "role_id", "Contact Roles"),
      enumFilter("priority", "priority", contactAssignmentPriorityOptions),
    ],
    { object: "objectId", contactName: "contact" },
    identity("objectId", "inventory.resources.fields.assignedTo", "object"),
  ),
  Manufacturers: cfg(["external_id", "name", "slug", "description", "tags", "piping_synced_at"]),
  "Circuit Types": cfg(["name", "slug", "color", "description", "owner"]),
  Providers: cfg(["name", "slug", "description", "comments"]),
  "Cluster Types": cfg(["name", "slug", "description"]),
  "Virtual Machine Types": cfg(
    ["name", "slug", "default_platform", "default_vcpus", "default_memory", "owner", "description", "comments"],
    [fkFilter("default_platform", "default_platform_id", "Platforms"), fkFilter("owner", "owner_id", "Owners")],
    { defaultPlatform: "default_platform", defaultVcpus: "default_vcpus", defaultMemory: "default_memory", ownerName: "owner" },
  ),
  "Cluster Groups": cfg(["name", "slug", "description"]),
  "ASN Ranges": cfg(
    ["name", "slug", "rir_id", "start", "end", "tenant_id", "description", "comments", "owner_id"],
    [fkFilter("rir_id", "rir_id", "RIRs"), fkFilter("tenant_id", "tenant_id", "Tenants")],
    {
      rirName: "rir_id",
      asnStart: "start",
      asnEnd: "end",
      tenant: "tenant_id",
      ownerName: "owner_id",
    },
  ),
  "Inventory Item Roles": cfg(["name", "slug", "color", "description", "owner_id"], [], { ownerName: "owner_id" }),
  "Inventory Items": cfg(
    ["name", "label", "part_id", "serial", "asset_tag", "discovered", "status", "device_id", "parent_id", "role_id", "manufacturer_id"],
    [
      statusFilter(),
      fkFilter("device_id", "device_id", "Devices"),
      fkFilter("parent_id", "parent_id", "Inventory Items"),
      fkFilter("role_id", "role_id", "Inventory Item Roles"),
      fkFilter("manufacturer_id", "manufacturer_id", "Manufacturers"),
      boolFilter("discovered", "discovered"),
    ],
    {
      deviceName: "device_id",
      parentName: "parent_id",
      role: "role_id",
      manufacturerName: "manufacturer_id",
      partId: "part_id",
    },
  ),
  "Tunnel Groups": cfg(["name", "slug", "description", "comments"]),
  Tunnels: cfg(
    ["name", "status", "encapsulation", "description", "comments", "group_id", "ipsec_profile_id", "tunnel_id", "tenant_id"],
    [
      statusFilter(),
      enumFilter("encapsulation", "encapsulation", tunnelEncapsulationOptions),
      fkFilter("group_id", "group_id", "Tunnel Groups"),
      fkFilter("ipsec_profile_id", "ipsec_profile_id", "IPsec Profiles"),
      fkFilter("tenant_id", "tenant_id", "Tenants"),
    ],
    { tunnelGroupName: "group_id", ipsecProfileName: "ipsec_profile_id", tunnelId: "tunnel_id", tenant: "tenant_id" },
  ),
  "Tunnel Terminations": cfg(
    ["role", "tunnel_id", "termination_type_id", "termination_id", "outside_ip_id"],
    [
      enumFilter("role", "role", tunnelTerminationRoleOptions),
      fkFilter("tunnel_id", "tunnel_id", "Tunnels"),
      objectRefFilter("termination_id", "termination_id", ["Interfaces", "VM Interfaces"]),
      fkFilter("outside_ip_id", "outside_ip_id", "IP Addresses"),
    ],
    { vpnRole: "role", tunnelFkName: "tunnel_id", outsideIpAddress: "outside_ip_id", termination: "termination_id" },
    identity("role", "inventory.resources.fields.vpnRole", "vpnRole"),
  ),
  "IKE Policies": cfg(["name", "version", "mode", "preshared_key", "description", "comments"], [
    enumFilter("mode", "mode", vpnModeOptions),
  ], {
    vpnVersion: "version",
    vpnMode: "mode",
    presharedKey: "preshared_key",
  }),
  "IKE Proposals": cfg(
    ["name", "authentication_method", "encryption_algorithm", "authentication_algorithm", "group", "sa_lifetime", "description", "comments"],
    [
      enumFilter("authentication_method", "authentication_method", ikeAuthenticationMethodOptions),
      enumFilter("encryption_algorithm", "encryption_algorithm", ikeEncryptionAlgorithmOptions),
      enumFilter("authentication_algorithm", "authentication_algorithm", ikeAuthenticationAlgorithmOptions),
    ],
    {
      authenticationMethod: "authentication_method",
      encryptionAlgorithm: "encryption_algorithm",
      authenticationAlgorithm: "authentication_algorithm",
      pfsGroup: "group",
      saLifetime: "sa_lifetime",
    },
  ),
  "IPsec Policies": cfg(["name", "pfs_group", "description", "comments"], [], { pfsGroup: "pfs_group" }),
  "IPsec Proposals": cfg(
    ["name", "encryption_algorithm", "authentication_algorithm", "sa_lifetime_seconds", "sa_lifetime_data", "description", "comments"],
    [
      enumFilter("encryption_algorithm", "encryption_algorithm", ipsecEncryptionAlgorithmOptions),
      enumFilter("authentication_algorithm", "authentication_algorithm", ipsecAuthenticationAlgorithmOptions),
    ],
    {
      encryptionAlgorithm: "encryption_algorithm",
      authenticationAlgorithm: "authentication_algorithm",
      saLifetimeSeconds: "sa_lifetime_seconds",
      saLifetimeData: "sa_lifetime_data",
    },
  ),
  "IPsec Profiles": cfg(["name", "mode", "ike_policy", "ipsec_policy", "description", "comments"], [
    enumFilter("mode", "mode", vpnModeOptions),
    fkFilter("ike_policy", "ike_policy_id", "IKE Policies"),
    fkFilter("ipsec_policy", "ipsec_policy_id", "IPsec Policies"),
  ], {
    vpnMode: "mode",
    ikePolicyName: "ike_policy",
    ipsecPolicyName: "ipsec_policy",
  }),
  "Circuit Groups": cfg(
    ["name", "slug", "tenant", "description", "comments"],
    [fkFilter("tenant", "tenant_id", "Tenants")],
  ),
  "Circuit Terminations": cfg(
    ["term_side", "circuit_id", "xconnect_id", "pp_info", "port_speed", "upstream_speed", "mark_connected", "description"],
    [
      enumFilter("term_side", "term_side", circuitTerminationSideOptions),
      fkFilter("circuit_id", "circuit_id", "Circuits"),
      boolFilter("mark_connected"),
    ],
    {
      termSide: "term_side",
      circuitName: "circuit_id",
      xconnectId: "xconnect_id",
      ppInfo: "pp_info",
      portSpeed: "port_speed",
      upstreamSpeed: "upstream_speed",
      markConnected: "mark_connected",
    },
    identity("circuit_id", "inventory.resources.fields.circuit", "circuitName"),
  ),
  "Virtual Circuit Types": cfg(["name", "slug", "color", "description", "comments"]),
  "Power Panels": cfg(
    ["name", "site_id", "location_id", "description", "comments"],
    [fkFilter("site_id", "site_id", "Sites"), fkFilter("location_id", "location_id", "Locations")],
    {
      site: "site_id",
      parentName: "location_id",
    },
  ),
  "Power Feeds": cfg(
    [
      "name",
      "status",
      "type",
      "supply",
      "phase",
      "voltage",
      "amperage",
      "max_utilization",
      "available_power",
      "power_panel_id",
      "rack_id",
      "mark_connected",
      "description",
      "comments",
      "tenant_id",
    ],
    [
      statusFilter(),
      enumFilter("type", "type", powerFeedTypeOptions),
      enumFilter("supply", "supply", powerFeedSupplyOptions),
      enumFilter("phase", "phase", powerFeedPhaseOptions),
      fkFilter("power_panel_id", "power_panel_id", "Power Panels"),
      fkFilter("rack_id", "rack_id", "Racks"),
      boolFilter("mark_connected"),
      fkFilter("tenant_id", "tenant_id", "Tenants"),
    ],
    {
      rackType: "type",
      powerPanelName: "power_panel_id",
      rackName: "rack_id",
      maxUtilization: "max_utilization",
      markConnected: "mark_connected",
      tenant: "tenant_id",
    },
  ),
  "Rack Reservations": cfg(
    ["units", "rack_id", "user_id", "status", "tenant_id", "description", "comments"],
    [statusFilter(), fkFilter("rack_id", "rack_id", "Racks"), fkFilter("tenant_id", "tenant_id", "Tenants")],
    {
      rackName: "rack_id",
      userName: "user_id",
      tenant: "tenant_id",
    },
    identity("rack_id", "inventory.resources.fields.rack", "rackName"),
  ),
  "MAC Addresses": cfg(["mac_address", "assigned_object_type_id", "assigned_object_id", "description", "comments"], [
    objectRefFilter("assigned_object_id", "assigned_object_id", ["Interfaces", "VM Interfaces"]),
  ], {
    macAddress: "mac_address",
    assignedObject: "assigned_object_id",
  }, identity("mac_address", "inventory.resources.fields.macAddress", "macAddress")),
  "Virtual Device Contexts": cfg(
    ["name", "status", "identifier", "device_id", "primary_ip4_id", "primary_ip6_id", "tenant_id", "description", "comments"],
    [
      statusFilter(),
      fkFilter("device_id", "device_id", "Devices"),
      fkFilter("primary_ip4_id", "primary_ip4_id", "IP Addresses"),
      fkFilter("primary_ip6_id", "primary_ip6_id", "IP Addresses"),
      fkFilter("tenant_id", "tenant_id", "Tenants"),
    ],
    { deviceName: "device_id", primaryIp4: "primary_ip4_id", primaryIp6: "primary_ip6_id", tenant: "tenant_id" },
  ),
  "Service Templates": cfg(["name", "protocol", "ports", "description", "comments"], [serviceProtocolFilter()]),

  "Rack Roles": cfg(["external_id", "name", "slug", "color", "description", "tags", "piping_synced_at"], [], {
    pipingSyncedAt: "piping_synced_at",
  }),
  "Rack Types": cfg(
    [
      "external_id",
      "model",
      "manufacturer",
      "slug",
      "form_factor",
      "width",
      "u_height",
      "starting_unit",
      "desc_units",
      "outer_width",
      "outer_depth",
      "outer_height",
      "outer_unit",
      "mounting_depth",
      "weight",
      "max_weight",
      "weight_unit",
      "description",
      "tags",
      "piping_synced_at",
    ],
    [
      fkFilter("manufacturer", "manufacturer_id", "Manufacturers"),
      enumFilter("form_factor", "form_factor", rackTypeFormFactorOptions),
      boolFilter("desc_units"),
      enumFilter("outer_unit", "outer_unit", rackTypeOuterUnitOptions),
      enumFilter("weight_unit", "weight_unit", weightUnitOptions),
    ],
    {
      manufacturerName: "manufacturer",
      formFactor: "form_factor",
      uHeight: "u_height",
      startingUnit: "starting_unit",
      descUnits: "desc_units",
      outerWidth: "outer_width",
      outerDepth: "outer_depth",
      outerHeight: "outer_height",
      outerUnit: "outer_unit",
      mountingDepth: "mounting_depth",
      maxWeight: "max_weight",
      weightUnit: "weight_unit",
      pipingSyncedAt: "piping_synced_at",
    },
    identity("model", "Model"),
  ),
  Locations: cfg(
    ["external_id", "name", "slug", "site", "parent", "status", "tenant", "facility", "description", "tags", "piping_synced_at"],
    [
      statusFilter(),
      fkFilter("site", "site_id", "Sites"),
      fkFilter("parent", "parent_id", "Locations"),
      fkFilter("tenant", "tenant_id", "Tenants"),
    ],
    { parentName: "parent", pipingSyncedAt: "piping_synced_at" },
  ),

  Interfaces: cfg(
    ["device_id", "name", "label", "description", "type", "enabled", "mgmt_only"],
    [
      fkFilter("device_id", "device_id", "Devices"),
      enumFilter("type", "type", interfaceTypeOptions),
      boolFilter("enabled"),
      boolFilter("mgmt_only"),
    ],
    {
      deviceName: "device_id",
      interfaceType: "type",
      mgmtOnly: "mgmt_only",
    },
  ),
  Cables: cfg(
    ["type", "status", "label", "color", "length", "length_unit"],
    [
      enumFilter("type", "type", cableTypeOptions),
      statusFilter(),
      enumFilter("length_unit", "length_unit", cableLengthUnitOptions),
    ],
    {
      interfaceType: "type",
      lengthUnit: "length_unit",
    },
    identity("label", "Label", "label"),
  ),
  "IP Addresses": cfg(
    ["address", "status", "role", "dns_name", "description", "assigned_object_id", "assigned_object_type_id", "tenant_id", "vrf_id"],
    [
      statusFilter(),
      fkFilter("assigned_object_id", "assigned_object_id", ["Interfaces", "VM Interfaces"]),
      fkFilter("tenant_id", "tenant_id", "Tenants"),
      fkFilter("vrf_id", "vrf_id", "VRFs"),
    ],
    {
      prefix: "address",
      dnsName: "dns_name",
      interfaceName: "assigned_object_id",
      tenant: "tenant_id",
      vrfName: "vrf_id",
    },
    identity("address", "inventory.resources.fields.ipAddress", "prefix"),
  ),
  "VM Interfaces": cfg(
    ["virtual_machine_id", "name", "enabled", "mtu", "mode", "description"],
    [
      fkFilter("virtual_machine_id", "virtual_machine_id", "Virtual Machines"),
      boolFilter("enabled"),
      enumFilter("mode", "mode", vmInterfaceModeOptions),
    ],
    {
      virtualMachineName: "virtual_machine_id",
    },
  ),
  RIRs: cfg(["name", "slug", "is_private", "description", "owner"], [boolFilter("is_private")], {
    isPrivate: "is_private",
  }),
  VRFs: cfg(
    ["name", "rd", "enforce_unique", "description", "comments", "tenant", "tenantName"],
    [boolFilter("enforce_unique"), fkFilter("tenant", "tenant_id", "Tenants")],
    {
      enforceUnique: "enforce_unique",
    },
  ),
  "Route Targets": cfg(
    ["name", "description", "tenant", "tenantName"],
    [fkFilter("tenant", "tenant_id", "Tenants")],
  ),
  Aggregates: cfg(
    ["prefix", "rir", "rirName", "tenant", "tenantName", "date_added", "description", "comments"],
    [fkFilter("rirName", "rir_id", "RIRs"), fkFilter("tenant", "tenant_id", "Tenants")],
    {
      prefixCidr: "prefix",
      dateAdded: "date_added",
    },
    identity("prefix", "inventory.resources.fields.prefix", "prefixCidr"),
  ),
  ASNs: cfg(
    ["asn", "rir", "rirName", "tenant", "tenantName", "description", "comments"],
    [fkFilter("rirName", "rir_id", "RIRs"), fkFilter("tenant", "tenant_id", "Tenants")],
    undefined,
    identity("asn", "inventory.resources.fields.asn", "asn"),
  ),
  "IP Ranges": cfg(
    [
      "start_address",
      "end_address",
      "size",
      "status",
      "vrf",
      "vrfName",
      "tenant",
      "tenantName",
      "description",
      "mark_utilized",
      "mark_populated",
    ],
    [
      statusFilter(),
      fkFilter("vrf", "vrf_id", "VRFs"),
      fkFilter("tenant", "tenant_id", "Tenants"),
      boolFilter("mark_utilized"),
      boolFilter("mark_populated"),
    ],
    {
      startAddress: "start_address",
      endAddress: "end_address",
      vrfName: "vrf",
      markUtilized: "mark_utilized",
      markPopulated: "mark_populated",
    },
    identity("start_address", "inventory.resources.fields.startAddress", "startAddress"),
  ),
  Services: {
    ...cfg(
      ["name", "protocol", "ports", "parentObject", "description"],
      [
        serviceProtocolFilter(),
        objectRefFilter("parentObject", "parent_object_id", ["Devices", "Virtual Machines"]),
        // owner_id targets auth users, which are not an InventoryEntityKey source.
      ],
    ),
    specialColumnKeys: ["portMappings"],
  },
  Modules: cfg(
    ["device_id", "module_bay_id", "module_type_id", "status", "serial", "asset_tag", "description"],
    [statusFilter(), fkFilter("device_id", "device_id", "Devices"), fkFilter("module_type_id", "module_type_id", "Module Types")],
    {
      deviceName: "device_id",
      moduleTypeName: "module_type_id",
      moduleBayName: "module_bay_id",
      serialNumber: "serial",
      assetTagModule: "asset_tag",
    },
    identity("module_type_id", "inventory.resources.fields.moduleType", "moduleTypeName"),
  ),
  "Module Types": cfg(["model", "manufacturer", "part_number", "weight", "weight_unit", "airflow", "description", "comments"], [
    fkFilter("manufacturer", "manufacturer_id", "Manufacturers"),
  ], {
    manufacturerName: "manufacturer",
  }, identity("model", "Model")),
  // Read-only: name/description/comments sort keys mirror the backend
  // MODULE_TYPE_PROFILE_COLUMNS registry.
  "Module Type Profiles": {
    ...cfg(["name", "description", "comments"]),
    specialColumnKeys: ["schemaSummary"],
  },
  Tags: cfg(["name", "slug", "color", "weight", "description"]),
  "Virtual Chassis": cfg(
    ["name", "domain", "master_id", "description", "comments", "member_count", "owner_id"],
    [fkFilter("master_id", "master_id", "Devices")],
    {
      masterName: "master_id",
      ownerName: "owner_id",
    },
  ),
  "Virtual Disks": cfg(
    ["virtual_machine_id", "name", "size", "description", "owner_id"],
    [fkFilter("virtual_machine_id", "virtual_machine_id", "Virtual Machines")],
    {
      virtualMachineName: "virtual_machine_id",
      ownerName: "owner_id",
    },
  ),
};

function camelToSnake(value: string): string {
  return value.replace(/[A-Z]/g, (match) => `_${match.toLowerCase()}`);
}

function stripSuffix(value: string, suffix: string): string | null {
  return value.endsWith(suffix) ? value.slice(0, -suffix.length) : null;
}

function sortedCandidate(sortKeySet: Set<string>, candidates: string[]): string | null {
  return candidates.find((candidate) => sortKeySet.has(candidate)) ?? null;
}

export function inventoryTableConfigFor(entity: InventoryEntityKey): InventoryTableConfig {
  return INVENTORY_TABLE_CONFIG[entity] ?? EMPTY_CONFIG;
}

export function inventoryNameColumnKey(entity: InventoryEntityKey): string {
  return inventoryTableConfigFor(entity).identity?.key ?? "name";
}

export function inventoryIdentityColumnLabel(entity: InventoryEntityKey): string {
  return inventoryTableConfigFor(entity).identity?.label ?? "name";
}

function isTabularInventoryField(field: InventoryFieldLike): boolean {
  // `nested-children` is a host-rendered inline editor, not a table column.
  return field.tableColumn !== false && (field.type !== "textarea" || field.tableColumn === true) && field.type !== "icon" && field.type !== "nested-children";
}

export function inventoryFieldRendersAsIdentityColumn(
  entity: InventoryEntityKey,
  field: InventoryFieldLike,
): boolean {
  const identityFieldKey = inventoryTableConfigFor(entity).identity?.fieldKey;
  if (identityFieldKey) return field.key === identityFieldKey;
  return inventoryColumnKeyForField(entity, field) === inventoryNameColumnKey(entity);
}

export function inventoryFieldRendersAsDataColumn(
  entity: InventoryEntityKey,
  field: InventoryFieldLike,
): boolean {
  return isTabularInventoryField(field) && !inventoryFieldRendersAsIdentityColumn(entity, field);
}

export function inventoryIdentityFieldForColumns<T extends InventoryFieldLike>(
  entity: InventoryEntityKey,
  fields: T[],
): T | undefined {
  const identityFieldKey = inventoryTableConfigFor(entity).identity?.fieldKey;
  if (identityFieldKey) return fields.find((field) => field.key === identityFieldKey);
  return fields.find((field) => isTabularInventoryField(field) && inventoryFieldRendersAsIdentityColumn(entity, field));
}

export function inventoryColumnKeyForField(
  entity: InventoryEntityKey,
  field: InventoryFieldLike,
): string {
  const tableConfig = inventoryTableConfigFor(entity);
  const explicit = tableConfig.columnKeyOverrides?.[field.key];
  if (explicit) return explicit;

  const sortKeySet = new Set(tableConfig.sortKeys);
  const candidates = [field.key, camelToSnake(field.key)];

  const nameStem = stripSuffix(field.key, "Name");
  if (nameStem) {
    candidates.push(nameStem, camelToSnake(nameStem));
  }

  const idStem = field.idKey ? stripSuffix(field.idKey, "Id") : null;
  if (idStem) {
    candidates.push(idStem, camelToSnake(idStem), `${camelToSnake(idStem)}_id`);
  }

  return sortedCandidate(sortKeySet, candidates) ?? field.key;
}
