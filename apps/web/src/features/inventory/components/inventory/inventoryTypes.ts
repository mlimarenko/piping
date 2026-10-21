import {
  MapPin,
  Map,
  Server,
  Cpu,
  Building2,
  Globe2,
  CircleDot,
  Users,
  FolderTree,
  Contact as ContactIcon,
  UserCog,
  UsersRound,
  Link2,
  Factory,
  Boxes,
  Layers,
  Tag,
  HardDrive,
  Network,
  Cable as CableIcon,
  LayoutGrid,
  Hash,
  Wifi,
  Landmark,
  Route,
  Target,
  Combine,
  Binary,
  ArrowLeftRight,
  Plug,
  Component,
  Package,
  Disc,
  Waypoints,
  GitBranch,
  Share2,
  Spline,
  SlidersHorizontal,
  Shield,
  ShieldCheck,
  Lock,
  KeyRound,
} from "lucide-react";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import type { RelationSpec } from "@/shared/nested-editor/descriptorFromSpec";

export type { Resource };

export const tabConfig = [
  { key: "Owners", icon: Users, singular: "Owner" },
  { key: "Owner Groups", icon: UsersRound, singular: "Owner Group" },
  { key: "Sites", icon: Building2, singular: "Site" },
  { key: "Devices", icon: Server, singular: "Device" },
  { key: "Virtual Machines", icon: Cpu, singular: "Virtual Machine" },
  { key: "Virtual Machine Types", icon: Boxes, singular: "Virtual Machine Type" },
  { key: "Tenants", icon: Users, singular: "Tenant" },
  { key: "Tenant Groups", icon: FolderTree, singular: "Tenant Group" },
  { key: "Regions", icon: Map, singular: "Region" },
  { key: "Device Roles", icon: CircleDot, singular: "Device Role" },
  { key: "Contacts", icon: ContactIcon, singular: "Contact" },
  { key: "Contact Roles", icon: UserCog, singular: "Contact Role" },
  { key: "Contact Groups", icon: UsersRound, singular: "Contact Group" },
  { key: "Contact Assignments", icon: Link2, singular: "Contact Assignment" },
  { key: "Manufacturers", icon: Factory, singular: "Manufacturer" },
  { key: "Site Groups", icon: LayoutGrid, singular: "Site Group" },
  { key: "Platforms", icon: Layers, singular: "Platform" },
  { key: "Device Types", icon: Cpu, singular: "Device Type" },
  { key: "Racks", icon: HardDrive, singular: "Rack" },
  { key: "Interfaces", icon: Network, singular: "Interface" },
  { key: "Cables", icon: CableIcon, singular: "Cable" },
  { key: "Cluster Types", icon: Boxes, singular: "Cluster Type" },
  { key: "Cluster Groups", icon: LayoutGrid, singular: "Cluster Group" },
  { key: "Clusters", icon: Boxes, singular: "Cluster" },
  { key: "VLANs", icon: Share2, singular: "VLAN" },
  { key: "Prefixes", icon: Hash, singular: "Prefix" },
  { key: "IP Addresses", icon: Globe2, singular: "IP Address" },
  { key: "VM Interfaces", icon: Wifi, singular: "VM Interface" },
  { key: "Rack Roles", icon: HardDrive, singular: "Rack Role" },
  { key: "Rack Types", icon: Layers, singular: "Rack Type" },
  { key: "Locations", icon: MapPin, singular: "Location" },
  { key: "RIRs", icon: Landmark, singular: "RIR" },
  { key: "VRFs", icon: Route, singular: "VRF" },
  { key: "Route Targets", icon: Target, singular: "Route Target" },
  { key: "Aggregates", icon: Combine, singular: "Aggregate" },
  { key: "ASNs", icon: Binary, singular: "ASN" },
  { key: "IP Ranges", icon: ArrowLeftRight, singular: "IP Range" },
  { key: "Services", icon: Plug, singular: "Service" },
  { key: "Modules", icon: Component, singular: "Module" },
  { key: "Module Types", icon: Package, singular: "Module Type" },
  { key: "Module Type Profiles", icon: Package, singular: "Module Type Profile" },
  { key: "Tags", icon: Tag, singular: "Tag" },
  { key: "Virtual Chassis", icon: Boxes, singular: "Virtual Chassis" },
  { key: "Virtual Disks", icon: Disc, singular: "Virtual Disk" },
  // Providers / Circuit Types / Provider Networks / Circuits / ASN Ranges /
  // Inventory Item Roles / Inventory Items.
  { key: "Providers", icon: Waypoints, singular: "Provider" },
  { key: "Circuit Types", icon: GitBranch, singular: "Circuit Type" },
  { key: "Provider Networks", icon: Share2, singular: "Provider Network" },
  { key: "Circuits", icon: Spline, singular: "Circuit" },
  { key: "ASN Ranges", icon: ArrowLeftRight, singular: "ASN Range" },
  { key: "Inventory Item Roles", icon: SlidersHorizontal, singular: "Inventory Item Role" },
  { key: "Inventory Items", icon: Component, singular: "Inventory Item" },
  // VPN.
  { key: "Tunnel Groups", icon: Network, singular: "Tunnel Group" },
  { key: "Tunnels", icon: Spline, singular: "Tunnel" },
  { key: "Tunnel Terminations", icon: Waypoints, singular: "Tunnel Termination" },
  { key: "IKE Policies", icon: Shield, singular: "IKE Policy" },
  { key: "IKE Proposals", icon: KeyRound, singular: "IKE Proposal" },
  { key: "IPsec Policies", icon: Lock, singular: "IPsec Policy" },
  { key: "IPsec Proposals", icon: ShieldCheck, singular: "IPsec Proposal" },
  { key: "IPsec Profiles", icon: ShieldCheck, singular: "IPsec Profile" },
  // Circuits remainder + DCIM gaps + IPAM gaps.
  { key: "Circuit Groups", icon: Combine, singular: "Circuit Group" },
  { key: "Provider Accounts", icon: Waypoints, singular: "Provider Account" },
  { key: "Circuit Terminations", icon: Spline, singular: "Circuit Termination" },
  { key: "Virtual Circuit Types", icon: GitBranch, singular: "Virtual Circuit Type" },
  { key: "Virtual Circuits", icon: Spline, singular: "Virtual Circuit" },
  { key: "Power Panels", icon: Landmark, singular: "Power Panel" },
  { key: "Power Feeds", icon: Plug, singular: "Power Feed" },
  { key: "Rack Reservations", icon: HardDrive, singular: "Rack Reservation" },
  { key: "MAC Addresses", icon: Binary, singular: "MAC Address" },
  { key: "Virtual Device Contexts", icon: Layers, singular: "Virtual Device Context" },
  { key: "Service Templates", icon: Component, singular: "Service Template" },
] as const;

/**
 * Inventory entities the backend exposes read-only (native list/detail routes,
 * no create/update/delete). The resource page suppresses every write affordance
 * for these; it never fakes a write endpoint.
 */
export const READ_ONLY_INVENTORY_ENTITIES: ReadonlySet<InventoryEntityKey> = new Set([
  "Module Type Profiles",
  "Tags",
]);

export function inventoryEntityIsReadOnly(entity: InventoryEntityKey): boolean {
  return READ_ONLY_INVENTORY_ENTITIES.has(entity);
}

export const statusOptions = [
  { value: "active", label: "inventory.resources.status.active" },
  { value: "planned", label: "inventory.resources.status.planned" },
  { value: "staged", label: "inventory.resources.status.staged" },
  { value: "decommissioning", label: "inventory.resources.status.decommissioning" },
  { value: "offline", label: "inventory.resources.status.offline" },
];

/** Pinned native AdminModel contract; other models retain their own metadata. */
const MODEL_CAPABILITIES: Partial<Record<InventoryEntityKey, { tags: boolean; custom_fields: boolean; change_dates: boolean }>> = {
  Owners: { tags: false, custom_fields: false, change_dates: false },
  "Owner Groups": { tags: false, custom_fields: false, change_dates: false },
};

export const inventoryModelCapabilities = (entity: InventoryEntityKey) => MODEL_CAPABILITIES[entity];

/** One allowed polymorphic target for an `objectRef` field: the canonical
 *  `app_label.model` token, the inventory tab its picker selects from, and the
 *  i18n key for the token's human name in the type dropdown. */
export interface ObjectRefTargetOption {
  token: string;
  relatedTab: InventoryEntityKey;
  label: string;
}

export interface FieldConfig {
  /** Human-display column key used for table rendering and form state. */
  key: keyof Resource;
  label: string;
  type:
    | "text"
    | "select"
    | "related"
    | "identity-multiple"
    | "related-multiple"
    | "textarea"
    | "color"
    | "number"
    | "icon"
    | "objectRef"
    | "nested-children";
  required?: boolean;
  placeholder?: string;
  maxLength?: number;
  options?: string[];
  /**
   * For `objectRef` fields: the allowed polymorphic target set. The form renders
   * a type `<Select>` over these tokens plus a cascading entity picker bound to
   * the chosen token's `relatedTab`. `key` points at the `Resource`'s nested
   * `ObjectRefProjection` (e.g. `assignedObject`).
   */
  objectTypes?: ObjectRefTargetOption[];
  /**
   * For `number` fields: which virtual keyboard / parsing affordance to surface.
   * `decimal` allows a fractional/negative value (coordinates, dimensions);
   * `numeric` is integer-only (counts, IDs, speeds). Validation stays lenient —
   * the backend accepts these as strings — this only drives the input UX.
   */
  numberMode?: "decimal" | "numeric";
  relatedTab?: InventoryEntityKey;
  identityKind?: "user" | "group";
  /** Short notes worth showing in the list as well as the full textarea form. */
  tableColumn?: boolean;
  /** Explicit wire value for a cleared scalar/FK instead of omission. */
  emptyValue?: "" | null;
  /**
   * For `related` fields: the `*Id` FK key on `Resource` that backend actually
   * consumes. ResourceFormDialog stores the selected UUID under this key so the
   * payload mapper (`toBackendPayload` in api/inventory.ts) emits `site_id`/etc.
   */
  idKey?: keyof Resource;
  /** For `icon` fields: companion custom image data URL key. */
  iconDataKey?: keyof Resource;
  /**
   * For `nested-children` fields: the {@link relationshipsByTab} key whose
   * `RelationSpec[]` declares which collection FKs become inline nested editors.
   * Scalar `related` fields stay EntitySelect pickers and are not affected.
   */
  relationshipsKey?: InventoryEntityKey;
}

/** Fields to show in CRUD form per tab. Status is entity-specific. */
export const fieldsByTab: Record<string, FieldConfig[]> = {
  Owners: [
    { key: "groupName", idKey: "groupId", label: "inventory.resources.fields.ownerGroup", type: "related", relatedTab: "Owner Groups", emptyValue: null },
    { key: "users", label: "accessControl.tabs.users", type: "identity-multiple", identityKind: "user" },
    { key: "userGroups", label: "accessControl.tabs.groups", type: "identity-multiple", identityKind: "group" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", maxLength: 200, tableColumn: true, emptyValue: "" },
  ],
  "Owner Groups": [
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", maxLength: 200, tableColumn: true, emptyValue: "" },
  ],
  Sites: [
    { key: "region", idKey: "regionId", label: "inventory.resources.fields.region", type: "related", relatedTab: "Regions", placeholder: "inventory.resources.fields.regionPlaceholder" },
    { key: "siteGroupName", idKey: "siteGroupId", label: "inventory.resources.fields.siteGroup", type: "related", relatedTab: "Site Groups", placeholder: "inventory.resources.fields.siteGroupPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "facility", label: "inventory.resources.fields.facility", type: "text", placeholder: "inventory.resources.fields.facilityPlaceholder" },
    { key: "timeZone", label: "inventory.resources.fields.timeZone", type: "text", placeholder: "inventory.resources.fields.timeZonePlaceholder" },
    { key: "physicalAddress", label: "inventory.resources.fields.physicalAddress", type: "textarea", placeholder: "inventory.resources.fields.physicalAddressPlaceholder", maxLength: 500 },
    { key: "shippingAddress", label: "inventory.resources.fields.shippingAddress", type: "textarea", placeholder: "inventory.resources.fields.shippingAddressPlaceholder", maxLength: 500 },
    { key: "latitude", label: "inventory.resources.fields.latitude", type: "number", numberMode: "decimal", placeholder: "inventory.resources.fields.latitudePlaceholder" },
    { key: "longitude", label: "inventory.resources.fields.longitude", type: "number", numberMode: "decimal", placeholder: "inventory.resources.fields.longitudePlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  Devices: [
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder", required: true },
    { key: "role", idKey: "roleId", label: "inventory.resources.fields.role", type: "related", relatedTab: "Device Roles", placeholder: "inventory.resources.fields.rolePlaceholder" },
    { key: "deviceType", idKey: "deviceTypeId", label: "inventory.resources.fields.deviceType", type: "related", relatedTab: "Device Types", placeholder: "inventory.resources.fields.deviceTypePlaceholder" },
    { key: "platform", idKey: "platformId", label: "inventory.resources.fields.platform", type: "related", relatedTab: "Platforms", placeholder: "inventory.resources.fields.platformPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "rackName", idKey: "rackId", label: "inventory.resources.fields.rack", type: "related", relatedTab: "Racks", placeholder: "inventory.resources.fields.rackPlaceholder" },
    { key: "primaryIp", label: "inventory.resources.fields.primaryIp", type: "text", placeholder: "inventory.resources.fields.primaryIpPlaceholder" },
    // Inline nested editors (Interfaces → IP addresses, Modules) render right
    // under the network fields — the relevant place, not the form's tail. The
    // scalar `related` fields above stay EntitySelect pickers; the IP rows carry
    // the "primary" toggle that sets this device's primary IP. The secondary
    // detail fields below follow the nested block.
    { key: "id", label: "nestedEditor.title", type: "nested-children", relationshipsKey: "Devices" },
    { key: "serial", label: "inventory.resources.fields.serial", type: "text", placeholder: "inventory.resources.fields.serialPlaceholder" },
    { key: "assetTag", label: "inventory.resources.fields.assetTag", type: "text", placeholder: "inventory.resources.fields.assetTagPlaceholder" },
    { key: "position", label: "inventory.resources.fields.position", type: "number", numberMode: "decimal", placeholder: "inventory.resources.fields.positionPlaceholder" },
    { key: "face", label: "inventory.resources.fields.face", type: "text", placeholder: "inventory.resources.fields.facePlaceholder" },
    { key: "airflow", label: "inventory.resources.fields.airflow", type: "text", placeholder: "inventory.resources.fields.airflowPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  "Virtual Machines": [
    { key: "virtualMachineType", idKey: "virtualMachineTypeId", label: "inventory.resources.fields.virtualMachineType", type: "related", relatedTab: "Virtual Machine Types", emptyValue: "" },
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder" },
    { key: "cluster", idKey: "clusterId", label: "inventory.resources.fields.cluster", type: "related", relatedTab: "Clusters", placeholder: "inventory.resources.fields.clusterPlaceholder" },
    { key: "role", idKey: "roleId", label: "inventory.resources.fields.role", type: "related", relatedTab: "Device Roles", placeholder: "inventory.resources.fields.rolePlaceholder" },
    { key: "platform", idKey: "platformId", label: "inventory.resources.fields.platform", type: "related", relatedTab: "Platforms", placeholder: "inventory.resources.fields.platformPlaceholder", emptyValue: "" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "primaryIp", label: "inventory.resources.fields.primaryIp", type: "text", placeholder: "inventory.resources.fields.primaryIpPlaceholder" },
    // Inline nested editor (VM Interfaces → IP addresses) under the network
    // fields; IP rows carry the "primary" toggle for this VM's primary IP.
    { key: "id", label: "nestedEditor.title", type: "nested-children", relationshipsKey: "Virtual Machines" },
    { key: "vcpus", label: "inventory.resources.fields.vcpus", type: "number", numberMode: "decimal", placeholder: "inventory.resources.fields.vcpusPlaceholder", emptyValue: "" },
    { key: "memory", label: "inventory.resources.fields.memory", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.memoryPlaceholder", emptyValue: "" },
    { key: "disk", label: "inventory.resources.fields.disk", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.diskPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  // Tenants: group FK + description + comments. TenantPayload accepts both
  // description and comments (verified against schema.gen.ts).
  Tenants: [
    { key: "groupName", idKey: "groupId", label: "inventory.resources.fields.tenantGroup", type: "related", relatedTab: "Tenant Groups", placeholder: "inventory.resources.fields.tenantGroupPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  "Tenant Groups": [
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.parentGroup", type: "related", relatedTab: "Tenant Groups", placeholder: "inventory.resources.fields.parentGroupPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Regions: parent FK + description. RegionPayload accepts description but NOT
  // comments (verified against schema.gen.ts) — comments is intentionally omitted.
  Regions: [
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.parentRegion", type: "related", relatedTab: "Regions", placeholder: "inventory.resources.fields.parentRegionPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  "Device Roles": [
    { key: "iconName", iconDataKey: "iconDataUrl", label: "inventory.resources.fields.icon", type: "icon" },
    { key: "color", label: "inventory.resources.fields.color", type: "color", placeholder: "inventory.resources.fields.colorPlaceholder" },
    { key: "vmRole", label: "inventory.resources.fields.vmRole", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  "Contact Roles": [
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  "Contact Groups": [
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.parentGroup", type: "related", relatedTab: "Contact Groups", placeholder: "inventory.resources.fields.parentGroupPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  Contacts: [
    { key: "groupName", idKey: "groupId", label: "inventory.resources.fields.contactGroup", type: "related", relatedTab: "Contact Groups", placeholder: "inventory.resources.fields.contactGroupPlaceholder" },
    { key: "title", label: "inventory.resources.fields.title", type: "text", placeholder: "inventory.resources.fields.titlePlaceholder" },
    { key: "phone", label: "inventory.resources.fields.phone", type: "text", placeholder: "inventory.resources.fields.phonePlaceholder" },
    { key: "email", label: "inventory.resources.fields.email", type: "text", placeholder: "inventory.resources.fields.emailPlaceholder" },
    { key: "address", label: "inventory.resources.fields.address", type: "textarea", placeholder: "inventory.resources.fields.addressPlaceholder", maxLength: 500 },
    { key: "link", label: "inventory.resources.fields.link", type: "text", placeholder: "inventory.resources.fields.linkPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  "Contact Assignments": [
    { key: "contactName", idKey: "contactId", label: "inventory.resources.fields.contact", type: "related", relatedTab: "Contacts", placeholder: "inventory.resources.fields.contactPlaceholder", required: true },
    { key: "role", idKey: "roleId", label: "inventory.resources.fields.contactRole", type: "related", relatedTab: "Contact Roles", placeholder: "inventory.resources.fields.contactRolePlaceholder" },
    {
      key: "object",
      label: "inventory.resources.fields.assignedTo",
      type: "objectRef",
      required: true,
      objectTypes: [
        { token: "dcim.site", relatedTab: "Sites", label: "inventory.resources.objectTypes.dcim.site" },
        { token: "dcim.region", relatedTab: "Regions", label: "inventory.resources.objectTypes.dcim.region" },
        { token: "dcim.device", relatedTab: "Devices", label: "inventory.resources.objectTypes.dcim.device" },
        { token: "tenancy.tenant", relatedTab: "Tenants", label: "inventory.resources.objectTypes.tenancy.tenant" },
        { token: "virtualization.cluster", relatedTab: "Clusters", label: "inventory.resources.objectTypes.virtualization.cluster" },
        { token: "virtualization.virtualmachine", relatedTab: "Virtual Machines", label: "inventory.resources.objectTypes.virtualization.virtualmachine" },
      ],
    },
    { key: "priority", label: "inventory.resources.fields.priority", type: "text", placeholder: "inventory.resources.fields.priorityPlaceholder" },
  ],
  // ── DCIM ──
  // Manufacturers: name (generic) + description. slug derived from name.
  Manufacturers: [
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Site Groups: name (generic) + optional parent group + description.
  "Site Groups": [
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.parentGroup", type: "related", relatedTab: "Site Groups", placeholder: "inventory.resources.fields.parentGroupPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Platforms: name (generic) + manufacturer FK + NAPALM driver + description.
  Platforms: [
    { key: "manufacturerName", idKey: "manufacturerId", label: "inventory.resources.fields.manufacturer", type: "related", relatedTab: "Manufacturers", placeholder: "inventory.resources.fields.manufacturerPlaceholder" },
    { key: "napalmDriver", label: "inventory.resources.fields.napalmDriver", type: "text", placeholder: "inventory.resources.fields.napalmDriverPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Device Types: the model is captured via the generic name field (mapped to
  // `model` in api.ts). DeviceTypePayload accepts manufacturer FK + description +
  // comments (verified against schema.gen.ts). No separate `model` inspector row
  // is added because the title/header already displays model/name.
  "Device Types": [
    { key: "manufacturerName", idKey: "manufacturerId", label: "inventory.resources.fields.manufacturer", type: "related", relatedTab: "Manufacturers", placeholder: "inventory.resources.fields.manufacturerPlaceholder", required: true },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  // Racks: name (generic) + site FK + tenant FK + status + type + identifiers +
  // dimensions. location/role are not in the create payload (see report).
  Racks: [
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder", required: true },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "rackRoleName", idKey: "rackRoleId", label: "inventory.resources.fields.rackRole", type: "related", relatedTab: "Rack Roles", placeholder: "inventory.resources.fields.rackRolePlaceholder" },
    { key: "rackType", label: "inventory.resources.fields.rackType", type: "text", placeholder: "inventory.resources.fields.rackTypePlaceholder" },
    { key: "facilityId", label: "inventory.resources.fields.facilityId", type: "text", placeholder: "inventory.resources.fields.facilityIdPlaceholder" },
    { key: "serial", label: "inventory.resources.fields.serial", type: "text", placeholder: "inventory.resources.fields.serialPlaceholder" },
    { key: "assetTag", label: "inventory.resources.fields.assetTag", type: "text", placeholder: "inventory.resources.fields.assetTagPlaceholder" },
    { key: "width", label: "inventory.resources.fields.width", type: "text", placeholder: "inventory.resources.fields.widthPlaceholder" },
    { key: "uHeight", label: "inventory.resources.fields.uHeight", type: "text", placeholder: "inventory.resources.fields.uHeightPlaceholder" },
    { key: "outerWidth", label: "inventory.resources.fields.outerWidth", type: "text", placeholder: "inventory.resources.fields.outerWidthPlaceholder" },
    { key: "outerDepth", label: "inventory.resources.fields.outerDepth", type: "text", placeholder: "inventory.resources.fields.outerDepthPlaceholder" },
    { key: "descUnits", label: "inventory.resources.fields.descUnits", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
  ],
  // Interfaces: device FK + name (generic) + type + enabled + description.
  // mac_address/mtu/mode are not in the create payload (see report).
  Interfaces: [
    { key: "deviceName", idKey: "deviceId", label: "inventory.resources.fields.device", type: "related", relatedTab: "Devices", placeholder: "inventory.resources.fields.devicePlaceholder", required: true },
    { key: "interfaceType", label: "inventory.resources.fields.interfaceType", type: "text", placeholder: "inventory.resources.fields.interfaceTypePlaceholder" },
    { key: "label", label: "inventory.resources.fields.label", type: "text", placeholder: "inventory.resources.fields.labelPlaceholder" },
    { key: "enabled", label: "inventory.resources.fields.enabled", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "mgmtOnly", label: "inventory.resources.fields.mgmtOnly", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Cables: nameless on the wire — the generic name field maps to `label`.
  // Status is entity-level; the editable fields are cable type + physical
  // attributes stored directly on `dcim_cable`.
  Cables: [
    { key: "interfaceType", label: "inventory.resources.fields.cableType", type: "text", placeholder: "inventory.resources.fields.cableTypePlaceholder" },
    { key: "color", label: "inventory.resources.fields.color", type: "color", placeholder: "inventory.resources.fields.colorPlaceholder" },
    { key: "length", label: "inventory.resources.fields.length", type: "text", placeholder: "inventory.resources.fields.lengthPlaceholder" },
    { key: "lengthUnit", label: "inventory.resources.fields.lengthUnit", type: "text", placeholder: "inventory.resources.fields.lengthUnitPlaceholder" },
  ],
  // ── Virtualization + IPAM ──
  // Cluster Types: name (generic) only. ClusterTypePayload is { name, slug } —
  // description is not accepted by the create/update payload (response-only).
  "Cluster Types": [],
  "Virtual Machine Types": [
    { key: "slug", label: "inventory.resources.fields.slug", type: "text", required: true, maxLength: 100 },
    { key: "defaultPlatform", idKey: "defaultPlatformId", label: "inventory.resources.fields.defaultPlatform", type: "related", relatedTab: "Platforms", emptyValue: "" },
    { key: "defaultVcpus", label: "inventory.resources.fields.defaultVcpus", type: "number", numberMode: "decimal", emptyValue: "" },
    { key: "defaultMemory", label: "inventory.resources.fields.defaultMemory", type: "number", numberMode: "numeric", emptyValue: "" },
    { key: "ownerName", idKey: "ownerId", label: "inventory.resources.fields.owner", type: "related", relatedTab: "Owners", emptyValue: "" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", maxLength: 200, emptyValue: "" },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", emptyValue: "" },
  ],
  // Cluster Groups: name (generic) only. ClusterGroupPayload is { name, slug } —
  // description is not accepted by the create/update payload (response-only).
  "Cluster Groups": [],
  // Clusters: name + status + type FK + group FK + site FK + tenant FK +
  // optional notes.
  Clusters: [
    { key: "clusterTypeName", idKey: "clusterTypeId", label: "inventory.resources.fields.clusterType", type: "related", relatedTab: "Cluster Types", placeholder: "inventory.resources.fields.clusterTypePlaceholder" },
    { key: "clusterGroupName", idKey: "clusterGroupId", label: "inventory.resources.fields.clusterGroup", type: "related", relatedTab: "Cluster Groups", placeholder: "inventory.resources.fields.clusterGroupPlaceholder" },
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  // VLANs: vid (required int) + name (generic) + status + group FK + site FK +
  // tenant FK + description. role_id is int64 (NetBox external id) — omitted.
  VLANs: [
    { key: "vid", label: "inventory.resources.fields.vid", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.vidPlaceholder", required: true },
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Prefixes: prefix (required CIDR) + status + site FK + tenant FK + is_pool +
  // description. vrf_id and role_id are int64 — omitted (no resolver tabs).
  Prefixes: [
    { key: "prefix", label: "inventory.resources.fields.prefix", type: "text", placeholder: "inventory.resources.fields.prefixPlaceholder", required: true },
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder" },
    { key: "vrfName", idKey: "vrfId", label: "inventory.resources.fields.vrf", type: "related", relatedTab: "VRFs", placeholder: "inventory.resources.fields.vrfPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "isPool", label: "inventory.resources.fields.isPool", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "markUtilized", label: "inventory.resources.fields.markUtilized", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // IP Addresses: address (required CIDR/host) + interface FK (required) +
  // status + tenant FK + dns_name + description. role is response-only.
  "IP Addresses": [
    { key: "prefix", label: "inventory.resources.fields.ipAddress", type: "text", placeholder: "inventory.resources.fields.ipAddressPlaceholder", required: true },
    { key: "interfaceName", idKey: "interfaceId", label: "inventory.resources.fields.interface", type: "related", relatedTab: "Interfaces", placeholder: "inventory.resources.fields.interfacePlaceholder", required: true },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "vrfName", idKey: "vrfId", label: "inventory.resources.fields.vrf", type: "related", relatedTab: "VRFs", placeholder: "inventory.resources.fields.vrfPlaceholder" },
    { key: "dnsName", label: "inventory.resources.fields.dnsName", type: "text", placeholder: "inventory.resources.fields.dnsNamePlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // VM Interfaces: vm FK (required) + name (generic) + enabled + description.
  // mac_address/mtu/mode not in the task surface — omitted.
  "VM Interfaces": [
    { key: "virtualMachineName", idKey: "virtualMachineId", label: "inventory.resources.fields.virtualMachine", type: "related", relatedTab: "Virtual Machines", placeholder: "inventory.resources.fields.virtualMachinePlaceholder", required: true },
    { key: "enabled", label: "inventory.resources.fields.enabled", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "mtu", label: "inventory.resources.fields.mtu", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.mtuPlaceholder" },
    { key: "mode", label: "inventory.resources.fields.mode", type: "text", placeholder: "inventory.resources.fields.modePlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // ── DCIM ──
  // Rack Roles: name (generic) + color + description. slug derived from name.
  "Rack Roles": [
    { key: "iconName", iconDataKey: "iconDataUrl", label: "inventory.resources.fields.icon", type: "icon" },
    { key: "color", label: "inventory.resources.fields.color", type: "color", placeholder: "inventory.resources.fields.colorPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Rack Types: model captured via generic name field (maps to `model` in api.ts).
  // manufacturer FK (required) + form_factor + width + u_height + weight + description.
  "Rack Types": [
    { key: "manufacturerName", idKey: "manufacturerId", label: "inventory.resources.fields.manufacturer", type: "related", relatedTab: "Manufacturers", placeholder: "inventory.resources.fields.manufacturerPlaceholder", required: true },
    { key: "formFactor", label: "inventory.resources.fields.formFactor", type: "text", placeholder: "inventory.resources.fields.formFactorPlaceholder" },
    { key: "width", label: "inventory.resources.fields.width", type: "text", placeholder: "inventory.resources.fields.widthPlaceholder" },
    { key: "uHeight", label: "inventory.resources.fields.uHeight", type: "text", placeholder: "inventory.resources.fields.uHeightPlaceholder" },
    { key: "startingUnit", label: "inventory.resources.fields.startingUnit", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.startingUnitPlaceholder" },
    { key: "descUnits", label: "inventory.resources.fields.descUnits", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "outerWidth", label: "inventory.resources.fields.outerWidth", type: "text", placeholder: "inventory.resources.fields.outerWidthPlaceholder" },
    { key: "outerDepth", label: "inventory.resources.fields.outerDepth", type: "text", placeholder: "inventory.resources.fields.outerDepthPlaceholder" },
    { key: "outerHeight", label: "inventory.resources.fields.outerHeight", type: "text", placeholder: "inventory.resources.fields.outerHeightPlaceholder" },
    { key: "outerUnit", label: "inventory.resources.fields.outerUnit", type: "text", placeholder: "inventory.resources.fields.outerUnitPlaceholder" },
    { key: "mountingDepth", label: "inventory.resources.fields.mountingDepth", type: "text", placeholder: "inventory.resources.fields.mountingDepthPlaceholder" },
    { key: "weight", label: "inventory.resources.fields.weight", type: "text", placeholder: "inventory.resources.fields.weightPlaceholder" },
    { key: "maxWeight", label: "inventory.resources.fields.maxWeight", type: "text", placeholder: "inventory.resources.fields.maxWeightPlaceholder" },
    { key: "weightUnit", label: "inventory.resources.fields.weightUnit", type: "text", placeholder: "inventory.resources.fields.weightUnitPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Locations: name (generic) + site FK (required) + parent FK + tenant FK +
  // facility + description. Status is entity-level. slug derived from name.
  Locations: [
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder", required: true },
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.location", type: "related", relatedTab: "Locations", placeholder: "inventory.resources.fields.locationPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "facility", label: "inventory.resources.fields.facility", type: "text", placeholder: "inventory.resources.fields.facilityPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // RIRs: name (generic) + is_private + description. slug derived from name.
  RIRs: [
    { key: "isPrivate", label: "inventory.resources.fields.isPrivate", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // VRFs: name (generic) + rd + tenant FK + enforce_unique + description + comments.
  VRFs: [
    { key: "rd", label: "inventory.resources.fields.rd", type: "text", placeholder: "inventory.resources.fields.rdPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "enforceUnique", label: "inventory.resources.fields.enforceUnique", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Route Targets: name (generic) + tenant FK + description.
  "Route Targets": [
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Aggregates: prefix (required CIDR — generic name maps to prefix) + RIR FK
  // (required) + tenant FK + description + comments.
  Aggregates: [
    { key: "prefixCidr", label: "inventory.resources.fields.prefix", type: "text", placeholder: "inventory.resources.fields.prefixPlaceholder", required: true },
    { key: "rirName", idKey: "rirId", label: "inventory.resources.fields.rir", type: "related", relatedTab: "RIRs", placeholder: "inventory.resources.fields.rirPlaceholder", required: true },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "dateAdded", label: "inventory.resources.fields.dateAdded", type: "text", placeholder: "inventory.resources.fields.dateAddedPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // ASNs: asn (required int — generic name maps to asn) + RIR FK (required) +
  // tenant FK + description + comments.
  ASNs: [
    { key: "asn", label: "inventory.resources.fields.asn", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.asnPlaceholder", required: true },
    { key: "rirName", idKey: "rirId", label: "inventory.resources.fields.rir", type: "related", relatedTab: "RIRs", placeholder: "inventory.resources.fields.rirPlaceholder", required: true },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // IP Ranges: start_address + end_address (both required) + status + tenant FK +
  // VRF FK + description. Nameless — no generic name field is shown.
  "IP Ranges": [
    { key: "startAddress", label: "inventory.resources.fields.startAddress", type: "text", placeholder: "inventory.resources.fields.startAddressPlaceholder", required: true },
    { key: "endAddress", label: "inventory.resources.fields.endAddress", type: "text", placeholder: "inventory.resources.fields.endAddressPlaceholder", required: true },
    { key: "vrfName", idKey: "vrfId", label: "inventory.resources.fields.vrf", type: "related", relatedTab: "VRFs", placeholder: "inventory.resources.fields.vrfPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "markUtilized", label: "inventory.resources.fields.markUtilized", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "markPopulated", label: "inventory.resources.fields.markPopulated", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Services: name (generic) + protocol + ports (comma-separated) + parent GFK
  // (type id + object id) + description.
  Services: [
    { key: "serviceIpAddressIds", label: "dataflows.serviceAddresses", type: "related-multiple", relatedTab: "IP Addresses", placeholder: "dataflows.serviceAddressesHint", tableColumn: false },
    { key: "protocol", label: "inventory.resources.fields.protocol", type: "text", placeholder: "inventory.resources.fields.protocolPlaceholder" },
    { key: "ports", label: "inventory.resources.fields.ports", type: "text", placeholder: "inventory.resources.fields.portsPlaceholder" },
    {
      key: "parentObject",
      label: "inventory.resources.fields.parentObject",
      type: "objectRef",
      required: true,
      objectTypes: [
        { token: "dcim.device", relatedTab: "Devices", label: "inventory.resources.objectTypes.dcim.device" },
        { token: "virtualization.virtualmachine", relatedTab: "Virtual Machines", label: "inventory.resources.objectTypes.virtualization.virtualmachine" },
      ],
    },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Modules: device FK (required) + module type FK (required) + module bay FK
  // (required) + status + serial + asset tag + description. Nameless — module
  // type stands in for the row label.
  Modules: [
    { key: "deviceName", idKey: "deviceId", label: "inventory.resources.fields.device", type: "related", relatedTab: "Devices", placeholder: "inventory.resources.fields.devicePlaceholder", required: true },
    { key: "moduleTypeName", idKey: "moduleTypeId", label: "inventory.resources.fields.moduleType", type: "related", relatedTab: "Module Types", placeholder: "inventory.resources.fields.moduleTypePlaceholder", required: true },
    { key: "moduleBayName", idKey: "moduleBayId", label: "inventory.resources.fields.moduleBayId", type: "text", placeholder: "inventory.resources.fields.moduleBayIdPlaceholder", required: true },
    { key: "serialNumber", label: "inventory.resources.fields.serial", type: "text", placeholder: "inventory.resources.fields.serialPlaceholder" },
    { key: "assetTagModule", label: "inventory.resources.fields.assetTag", type: "text", placeholder: "inventory.resources.fields.assetTagPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Module Types: model captured via generic name field (maps to `model`).
  // manufacturer FK (required) only — other attributes are response-only.
  "Module Types": [
    { key: "manufacturerName", idKey: "manufacturerId", label: "inventory.resources.fields.manufacturer", type: "related", relatedTab: "Manufacturers", placeholder: "inventory.resources.fields.manufacturerPlaceholder", required: true },
  ],
  // Module Type Profiles are read-only: name + description + comments. The
  // attribute `schema` is rendered as a dedicated table column, not a form field
  // (no write routes exist).
  "Module Type Profiles": [
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 1024 },
  ],
  Tags: [
    { key: "slug", label: "inventory.resources.fields.slug", type: "text" },
    { key: "color", label: "inventory.resources.fields.color", type: "text" },
    { key: "weight", label: "inventory.resources.fields.tagWeight", type: "number" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea" },
  ],
  // Virtual Chassis: name (generic) + domain + master device FK + description + comments.
  "Virtual Chassis": [
    { key: "domain", label: "inventory.resources.fields.domain", type: "text", placeholder: "inventory.resources.fields.domainPlaceholder" },
    { key: "masterName", idKey: "masterId", label: "inventory.resources.fields.master", type: "related", relatedTab: "Devices", placeholder: "inventory.resources.fields.masterPlaceholder" },
    { key: "ownerName", idKey: "ownerId", label: "inventory.resources.fields.owner", type: "text" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Virtual Disks: VM FK (required) + name (generic) + size (required int) + description.
  "Virtual Disks": [
    { key: "virtualMachineName", idKey: "virtualMachineId", label: "inventory.resources.fields.virtualMachine", type: "related", relatedTab: "Virtual Machines", placeholder: "inventory.resources.fields.virtualMachinePlaceholder", required: true },
    { key: "size", label: "inventory.resources.fields.size", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.sizePlaceholder", required: true },
    { key: "ownerName", idKey: "ownerId", label: "inventory.resources.fields.owner", type: "text" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // ── Circuits / Providers / Provider Networks / … ──
  // Providers: name (generic) + slug + description + comments.
  Providers: [
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Circuit Types: name (generic) + slug + color + description.
  "Circuit Types": [
    { key: "iconName", iconDataKey: "iconDataUrl", label: "inventory.resources.fields.icon", type: "icon" },
    { key: "color", label: "inventory.resources.fields.color", type: "color", placeholder: "inventory.resources.fields.colorPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Provider Networks: provider FK (required) + name (generic) + service_id +
  // description + comments.
  "Provider Networks": [
    { key: "providerName", idKey: "providerId", label: "inventory.resources.fields.provider", type: "related", relatedTab: "Providers", placeholder: "inventory.resources.fields.providerPlaceholder", required: true },
    { key: "serviceId", label: "inventory.resources.fields.serviceId", type: "text", placeholder: "inventory.resources.fields.serviceIdPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Circuits: cid (required — captured via generic name field) + provider FK
  // (required) + circuit type FK (required) + status + tenant FK + description +
  // comments. Other scalars (dates, rates, distances) are optional.
  Circuits: [
    { key: "providerName", idKey: "providerId", label: "inventory.resources.fields.provider", type: "related", relatedTab: "Providers", placeholder: "inventory.resources.fields.providerPlaceholder", required: true },
    { key: "circuitTypeName", idKey: "circuitTypeId", label: "inventory.resources.fields.circuitType", type: "related", relatedTab: "Circuit Types", placeholder: "inventory.resources.fields.circuitTypePlaceholder", required: true },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "installDate", label: "inventory.resources.fields.installDate", type: "text", placeholder: "inventory.resources.fields.installDatePlaceholder" },
    { key: "terminationDate", label: "inventory.resources.fields.terminationDate", type: "text", placeholder: "inventory.resources.fields.terminationDatePlaceholder" },
    { key: "commitRate", label: "inventory.resources.fields.commitRate", type: "text", placeholder: "inventory.resources.fields.commitRatePlaceholder" },
    { key: "distance", label: "inventory.resources.fields.distance", type: "text", placeholder: "inventory.resources.fields.distancePlaceholder" },
    { key: "distanceUnit", label: "inventory.resources.fields.distanceUnit", type: "text", placeholder: "inventory.resources.fields.distanceUnitPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // ASN Ranges: name (generic) + RIR FK (required) + start (required int) +
  // end (required int) + tenant FK + description + comments.
  "ASN Ranges": [
    { key: "rirName", idKey: "rirId", label: "inventory.resources.fields.rir", type: "related", relatedTab: "RIRs", placeholder: "inventory.resources.fields.rirPlaceholder", required: true },
    { key: "asnStart", label: "inventory.resources.fields.asnStart", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.asnStartPlaceholder", required: true },
    { key: "asnEnd", label: "inventory.resources.fields.asnEnd", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.asnEndPlaceholder", required: true },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "ownerName", idKey: "ownerId", label: "inventory.resources.fields.owner", type: "text" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Inventory Item Roles: name (generic) + slug + color + description.
  "Inventory Item Roles": [
    { key: "iconName", iconDataKey: "iconDataUrl", label: "inventory.resources.fields.icon", type: "icon" },
    { key: "color", label: "inventory.resources.fields.color", type: "color", placeholder: "inventory.resources.fields.colorPlaceholder" },
    { key: "ownerName", idKey: "ownerId", label: "inventory.resources.fields.owner", type: "text" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // Inventory Items: device FK (required) + optional parent/role/manufacturer +
  // label + part_id + serial + asset_tag + discovered + description. Name is
  // generic; status is entity-level; `component` is an ObjectRef not a form field.
  "Inventory Items": [
    { key: "deviceName", idKey: "deviceId", label: "inventory.resources.fields.device", type: "related", relatedTab: "Devices", placeholder: "inventory.resources.fields.devicePlaceholder", required: true },
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.parent", type: "related", relatedTab: "Inventory Items", placeholder: "inventory.resources.fields.parentPlaceholder" },
    { key: "role", idKey: "roleId", label: "inventory.resources.fields.role", type: "related", relatedTab: "Inventory Item Roles", placeholder: "inventory.resources.fields.rolePlaceholder" },
    { key: "manufacturerName", idKey: "manufacturerId", label: "inventory.resources.fields.manufacturer", type: "related", relatedTab: "Manufacturers", placeholder: "inventory.resources.fields.manufacturerPlaceholder" },
    { key: "label", label: "inventory.resources.fields.label", type: "text", placeholder: "inventory.resources.fields.labelPlaceholder" },
    { key: "partId", label: "inventory.resources.fields.partId", type: "text", placeholder: "inventory.resources.fields.partIdPlaceholder" },
    { key: "serial", label: "inventory.resources.fields.serial", type: "text", placeholder: "inventory.resources.fields.serialPlaceholder" },
    { key: "assetTag", label: "inventory.resources.fields.assetTag", type: "text", placeholder: "inventory.resources.fields.assetTagPlaceholder" },
    { key: "discovered", label: "inventory.resources.fields.discovered", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  // ── VPN ──
  // Tunnel Groups: name (generic) + slug + description + comments.
  "Tunnel Groups": [
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Tunnels: name (generic) + encapsulation + group FK + ipsec_profile FK +
  // tunnel_id (int) + tenant FK + description + comments. Status is entity-level.
  Tunnels: [
    { key: "encapsulation", label: "inventory.resources.fields.encapsulation", type: "text", placeholder: "inventory.resources.fields.encapsulationPlaceholder" },
    { key: "tunnelGroupName", idKey: "tunnelGroupId", label: "inventory.resources.fields.tunnelGroup", type: "related", relatedTab: "Tunnel Groups", placeholder: "inventory.resources.fields.tunnelGroupPlaceholder" },
    { key: "ipsecProfileName", idKey: "ipsecProfileId", label: "inventory.resources.fields.ipsecProfile", type: "related", relatedTab: "IPsec Profiles", placeholder: "inventory.resources.fields.ipsecProfilePlaceholder" },
    { key: "tunnelId", label: "inventory.resources.fields.tunnelId", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.tunnelIdPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Tunnel Terminations: role + tunnel FK (required) + termination_type_id +
  // termination_id + outside_ip_id. Nameless — role stands in for display.
  "Tunnel Terminations": [
    { key: "vpnRole", label: "inventory.resources.fields.vpnRole", type: "text", placeholder: "inventory.resources.fields.vpnRolePlaceholder" },
    { key: "tunnelFkName", idKey: "tunnelFkId", label: "inventory.resources.fields.tunnel", type: "related", relatedTab: "Tunnels", placeholder: "inventory.resources.fields.tunnelPlaceholder", required: true },
    {
      key: "termination",
      label: "inventory.resources.fields.termination",
      type: "objectRef",
      objectTypes: [
        { token: "dcim.interface", relatedTab: "Interfaces", label: "inventory.resources.objectTypes.dcim.interface" },
        { token: "virtualization.vminterface", relatedTab: "VM Interfaces", label: "inventory.resources.objectTypes.virtualization.vminterface" },
      ],
    },
    { key: "outsideIpAddress", idKey: "outsideIpId", label: "inventory.resources.fields.outsideIpId", type: "related", relatedTab: "IP Addresses", placeholder: "inventory.resources.fields.outsideIpIdPlaceholder" },
  ],
  // IKE Policies: name (generic) + version (int) + mode + preshared_key +
  // description + comments.
  "IKE Policies": [
    { key: "vpnVersion", label: "inventory.resources.fields.ikeVersion", type: "text", placeholder: "inventory.resources.fields.ikeVersionPlaceholder" },
    { key: "vpnMode", label: "inventory.resources.fields.vpnMode", type: "text", placeholder: "inventory.resources.fields.vpnModePlaceholder" },
    { key: "presharedKey", label: "inventory.resources.fields.presharedKey", type: "text", placeholder: "inventory.resources.fields.presharedKeyPlaceholder", required: true },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // IKE Proposals: name (generic) + authentication_method + encryption_algorithm +
  // authentication_algorithm + group (int, reuses pfsGroup) + sa_lifetime (int) +
  // description + comments.
  "IKE Proposals": [
    { key: "authenticationMethod", label: "inventory.resources.fields.authenticationMethod", type: "text", placeholder: "inventory.resources.fields.authenticationMethodPlaceholder" },
    { key: "encryptionAlgorithm", label: "inventory.resources.fields.encryptionAlgorithm", type: "text", placeholder: "inventory.resources.fields.encryptionAlgorithmPlaceholder" },
    { key: "authenticationAlgorithm", label: "inventory.resources.fields.authenticationAlgorithm", type: "text", placeholder: "inventory.resources.fields.authenticationAlgorithmPlaceholder" },
    { key: "pfsGroup", label: "inventory.resources.fields.ikeGroup", type: "text", placeholder: "inventory.resources.fields.ikeGroupPlaceholder" },
    { key: "saLifetime", label: "inventory.resources.fields.saLifetime", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.saLifetimePlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // IPsec Policies: name (generic) + pfs_group (int, reuses pfsGroup) +
  // description + comments.
  "IPsec Policies": [
    { key: "pfsGroup", label: "inventory.resources.fields.pfsGroup", type: "text", placeholder: "inventory.resources.fields.pfsGroupPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // IPsec Proposals: name (generic) + encryption_algorithm + authentication_algorithm +
  // sa_lifetime_seconds (int) + sa_lifetime_data (int) + description + comments.
  "IPsec Proposals": [
    { key: "encryptionAlgorithm", label: "inventory.resources.fields.encryptionAlgorithm", type: "text", placeholder: "inventory.resources.fields.encryptionAlgorithmPlaceholder" },
    { key: "authenticationAlgorithm", label: "inventory.resources.fields.authenticationAlgorithm", type: "text", placeholder: "inventory.resources.fields.authenticationAlgorithmPlaceholder" },
    { key: "saLifetimeSeconds", label: "inventory.resources.fields.saLifetimeSeconds", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.saLifetimeSecondsPlaceholder" },
    { key: "saLifetimeData", label: "inventory.resources.fields.saLifetimeData", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.saLifetimeDataPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // IPsec Profiles: name (generic) + mode + ike_policy FK (required) +
  // ipsec_policy FK (required) + description + comments.
  "IPsec Profiles": [
    { key: "vpnMode", label: "inventory.resources.fields.vpnMode", type: "text", placeholder: "inventory.resources.fields.vpnModePlaceholder" },
    { key: "ikePolicyName", idKey: "ikePolicyId", label: "inventory.resources.fields.ikePolicy", type: "related", relatedTab: "IKE Policies", placeholder: "inventory.resources.fields.ikePolicyPlaceholder", required: true },
    { key: "ipsecPolicyName", idKey: "ipsecPolicyId", label: "inventory.resources.fields.ipsecPolicy", type: "related", relatedTab: "IPsec Policies", placeholder: "inventory.resources.fields.ipsecPolicyPlaceholder", required: true },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // ── Circuits remainder + DCIM gaps + IPAM gaps ──
  "Circuit Groups": [
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Provider Accounts": [
    { key: "providerName", idKey: "providerId", label: "inventory.resources.fields.provider", type: "related", relatedTab: "Providers", placeholder: "inventory.resources.fields.providerPlaceholder", required: true },
    { key: "account", label: "inventory.resources.fields.account", type: "text", placeholder: "inventory.resources.fields.accountPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Circuit Terminations": [
    { key: "termSide", label: "inventory.resources.fields.termSide", type: "select", options: ["A", "Z"], placeholder: "inventory.resources.fields.termSidePlaceholder" },
    { key: "circuitName", idKey: "circuitId", label: "inventory.resources.fields.circuit", type: "related", relatedTab: "Circuits", placeholder: "inventory.resources.fields.circuitPlaceholder", required: true },
    { key: "xconnectId", label: "inventory.resources.fields.xconnectId", type: "text", placeholder: "inventory.resources.fields.xconnectIdPlaceholder" },
    { key: "ppInfo", label: "inventory.resources.fields.ppInfo", type: "text", placeholder: "inventory.resources.fields.ppInfoPlaceholder" },
    { key: "portSpeed", label: "inventory.resources.fields.portSpeed", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.portSpeedPlaceholder" },
    { key: "upstreamSpeed", label: "inventory.resources.fields.upstreamSpeed", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.upstreamSpeedPlaceholder" },
    { key: "markConnected", label: "inventory.resources.fields.markConnected", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
  ],
  "Virtual Circuit Types": [
    { key: "iconName", iconDataKey: "iconDataUrl", label: "inventory.resources.fields.icon", type: "icon" },
    { key: "color", label: "inventory.resources.fields.color", type: "color", placeholder: "inventory.resources.fields.colorPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Virtual Circuits": [
    { key: "providerNetworkName", idKey: "providerNetworkId", label: "inventory.resources.fields.providerNetwork", type: "related", relatedTab: "Provider Networks", placeholder: "inventory.resources.fields.providerNetworkPlaceholder", required: true },
    { key: "virtualCircuitTypeName", idKey: "virtualCircuitTypeId", label: "inventory.resources.fields.virtualCircuitType", type: "related", relatedTab: "Virtual Circuit Types", placeholder: "inventory.resources.fields.virtualCircuitTypePlaceholder", required: true },
    { key: "providerName", idKey: "providerAccountId", label: "inventory.resources.fields.providerAccount", type: "related", relatedTab: "Provider Accounts", placeholder: "inventory.resources.fields.providerAccountPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Power Panels": [
    { key: "site", idKey: "siteId", label: "inventory.resources.fields.site", type: "related", relatedTab: "Sites", placeholder: "inventory.resources.fields.sitePlaceholder", required: true },
    { key: "parentName", idKey: "parentId", label: "inventory.resources.fields.location", type: "related", relatedTab: "Locations", placeholder: "inventory.resources.fields.locationPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Power Feeds": [
    { key: "powerPanelName", idKey: "powerPanelId", label: "inventory.resources.fields.powerPanel", type: "related", relatedTab: "Power Panels", placeholder: "inventory.resources.fields.powerPanelPlaceholder", required: true },
    { key: "rackName", idKey: "rackId", label: "inventory.resources.fields.rack", type: "related", relatedTab: "Racks", placeholder: "inventory.resources.fields.rackPlaceholder" },
    { key: "rackType", label: "inventory.resources.fields.type", type: "text", placeholder: "inventory.resources.fields.typePlaceholder" },
    { key: "supply", label: "inventory.resources.fields.supply", type: "text", placeholder: "inventory.resources.fields.supplyPlaceholder" },
    { key: "phase", label: "inventory.resources.fields.phase", type: "text", placeholder: "inventory.resources.fields.phasePlaceholder" },
    { key: "voltage", label: "inventory.resources.fields.voltage", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.voltagePlaceholder" },
    { key: "amperage", label: "inventory.resources.fields.amperage", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.amperagePlaceholder" },
    { key: "maxUtilization", label: "inventory.resources.fields.maxUtilization", type: "number", numberMode: "numeric", placeholder: "inventory.resources.fields.maxUtilizationPlaceholder" },
    { key: "markConnected", label: "inventory.resources.fields.markConnected", type: "select", options: ["true", "false"], placeholder: "inventory.resources.fields.enabledPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Rack Reservations": [
    { key: "rackName", idKey: "rackId", label: "inventory.resources.fields.rack", type: "related", relatedTab: "Racks", placeholder: "inventory.resources.fields.rackPlaceholder", required: true },
    { key: "userName", idKey: "userId", label: "inventory.resources.fields.userId", type: "text", placeholder: "inventory.resources.fields.userIdPlaceholder", required: true },
    { key: "units", label: "inventory.resources.fields.units", type: "text", placeholder: "inventory.resources.fields.unitsPlaceholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  // Nameless — the MAC string is the identity, so `macAddress` is required
  // (it replaces the dropped generic name field as the sole address input).
  "MAC Addresses": [
    { key: "macAddress", label: "inventory.resources.fields.macAddress", type: "text", placeholder: "inventory.resources.fields.macAddressPlaceholder", required: true },
    {
      key: "assignedObject",
      label: "inventory.resources.fields.assignedObject",
      type: "objectRef",
      objectTypes: [
        { token: "dcim.interface", relatedTab: "Interfaces", label: "inventory.resources.objectTypes.dcim.interface" },
        { token: "virtualization.vminterface", relatedTab: "VM Interfaces", label: "inventory.resources.objectTypes.virtualization.vminterface" },
      ],
    },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Virtual Device Contexts": [
    { key: "deviceName", idKey: "deviceId", label: "inventory.resources.fields.device", type: "related", relatedTab: "Devices", placeholder: "inventory.resources.fields.devicePlaceholder" },
    { key: "identifier", label: "inventory.resources.fields.identifier", type: "text", placeholder: "inventory.resources.fields.identifierPlaceholder" },
    { key: "primaryIp4", idKey: "primaryIp4Id", label: "inventory.resources.fields.primaryIp4", type: "related", relatedTab: "IP Addresses", placeholder: "inventory.resources.fields.primaryIp4Placeholder" },
    { key: "primaryIp6", idKey: "primaryIp6Id", label: "inventory.resources.fields.primaryIp6", type: "related", relatedTab: "IP Addresses", placeholder: "inventory.resources.fields.primaryIp6Placeholder" },
    { key: "tenant", idKey: "tenantId", label: "inventory.resources.fields.tenant", type: "related", relatedTab: "Tenants", placeholder: "inventory.resources.fields.tenantPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
  "Service Templates": [
    { key: "protocol", label: "inventory.resources.fields.protocol", type: "text", placeholder: "inventory.resources.fields.protocolPlaceholder" },
    { key: "ports", label: "inventory.resources.fields.ports", type: "text", placeholder: "inventory.resources.fields.portsPlaceholder" },
    { key: "description", label: "inventory.resources.fields.description", type: "textarea", placeholder: "inventory.resources.fields.descriptionPlaceholder", maxLength: 255 },
    { key: "comments", label: "inventory.resources.fields.comments", type: "textarea", placeholder: "inventory.resources.fields.commentsPlaceholder", maxLength: 500 },
  ],
};

// `Resource` is the shared inventory row shape. Live data is fetched through
// `src/api/inventory.ts` (`listResources`); no seed data lives here.

/**
 * Pure-data declaration of which child collections of a parent entity become
 * inline nested editors. Lives beside {@link fieldsByTab}: a tab whose
 * `fieldsByTab` carries a `nested-children` field points its `relationshipsKey`
 * at this map. `descriptorFromSpec` compiles each {@link RelationSpec} into the
 * runtime descriptor the headless editor consumes, so the same data drives the
 * editor from any host. Scalar `related` fields stay EntitySelect pickers and
 * are intentionally NOT listed here.
 *
 * Backend list routes mirror `ENTITY_LIST_PATH` in `features/inventory/api.ts`
 * (single source of truth for route choices); the IP level reuses the proven
 * `assigned_object_id` filter to scope IP addresses by interface (and VM
 * interface), and `interfaceId` is the camelCase `Resource` idKey the IP create
 * needs as its FK.
 */
export const relationshipsByTab: Partial<Record<InventoryEntityKey, RelationSpec[]>> = {
  Devices: [
    {
      key: "device.interfaces",
      childEntity: "Interfaces",
      title: "nestedEditor.relations.interfaces",
      icon: "Network",
      listFilterKey: "device_id",
      listRoute: "/api/v1/inventory/interfaces",
      parentFkField: "deviceId",
      rowForm: { kind: "config", entity: "Interfaces" },
      child: {
        key: "device.interfaces.ips",
        childEntity: "IP Addresses",
        title: "nestedEditor.relations.ipAddresses",
        icon: "Globe2",
        listFilterKey: "assigned_object_id",
        listRoute: "/api/v1/inventory/ip-addresses",
        parentFkField: "interfaceId",
        rowForm: { kind: "config", entity: "IP Addresses" },
      },
    },
    {
      key: "device.modules",
      childEntity: "Modules",
      title: "nestedEditor.relations.modules",
      icon: "Component",
      listFilterKey: "device_id",
      listRoute: "/api/v1/inventory/modules",
      parentFkField: "deviceId",
      rowForm: { kind: "config", entity: "Modules" },
    },
  ],
  "Virtual Machines": [
    {
      key: "vm.interfaces",
      childEntity: "VM Interfaces",
      title: "nestedEditor.relations.vmInterfaces",
      icon: "Wifi",
      listFilterKey: "virtual_machine_id",
      listRoute: "/api/v1/inventory/vm-interfaces",
      parentFkField: "virtualMachineId",
      rowForm: { kind: "config", entity: "VM Interfaces" },
      child: {
        key: "vm.interfaces.ips",
        childEntity: "IP Addresses",
        title: "nestedEditor.relations.ipAddresses",
        icon: "Globe2",
        listFilterKey: "assigned_object_id",
        listRoute: "/api/v1/inventory/ip-addresses",
        parentFkField: "interfaceId",
        rowForm: { kind: "config", entity: "IP Addresses" },
      },
    },
  ],
};
