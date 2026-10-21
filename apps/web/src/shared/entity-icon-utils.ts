import {
  Antenna,
  ArrowLeftRight,
  Binary,
  Boxes,
  Building2,
  Cable,
  Camera,
  Cctv,
  CircleDot,
  Cloud,
  Combine,
  Component,
  Container,
  Cpu,
  Database,
  Disc,
  EthernetPort,
  Factory,
  FolderTree,
  Globe2,
  HardDrive,
  HardDriveDownload,
  Hash,
  KeyRound,
  Landmark,
  Laptop,
  Layers,
  LayoutGrid,
  Lock,
  Map,
  MapPin,
  Monitor,
  Network,
  Package,
  Phone,
  Plug,
  Printer,
  RadioTower,
  Route,
  Router,
  Server,
  Share2,
  Shield,
  ShieldAlert,
  ShieldCheck,
  Smartphone,
  Spline,
  Tag,
  Target,
  Tv,
  Users,
  Waypoints,
  Wifi,
  type LucideIcon,
} from "lucide-react";
import type { ConnectionTarget, InventoryEntityKey } from "@/shared/api/types";
import type { TreeGroupKey } from "@/features/connections/components/connections/treeConfig";

export const CUSTOM_ICON_FIELD = "piping_icon";

export type EntityIconName =
  | "antenna"
  | "asn"
  | "arrow-range"
  | "cable"
  | "camera"
  | "cctv"
  | "circuit"
  | "cluster"
  | "cloud"
  | "combine"
  | "component"
  | "container"
  | "database"
  | "device"
  | "device-type"
  | "disk"
  | "factory"
  | "firewall"
  | "folder"
  | "gateway"
  | "globe"
  | "hash"
  | "interface"
  | "ip"
  | "key"
  | "landmark"
  | "laptop"
  | "location"
  | "lock"
  | "module"
  | "monitor"
  | "nas"
  | "network"
  | "phone"
  | "platform"
  | "plug"
  | "printer"
  | "provider"
  | "rack"
  | "region"
  | "role"
  | "route"
  | "router"
  | "server"
  | "shield"
  | "site"
  | "smartphone"
  | "stack"
  | "switch"
  | "tag"
  | "tenant"
  | "terminal"
  | "target"
  | "tv"
  | "vlan"
  | "vm"
  | "vpn"
  | "wifi";

export interface IconPackEntry {
  name: EntityIconName;
  labelKey: string;
  fallback: string;
  Icon: LucideIcon;
}

export interface PersistedEntityIcon {
  name?: EntityIconName;
  dataUrl?: string;
}

export const ENTITY_ICONS: Record<EntityIconName, LucideIcon> = {
  antenna: Antenna,
  asn: Binary,
  "arrow-range": ArrowLeftRight,
  cable: Cable,
  camera: Camera,
  cctv: Cctv,
  circuit: Spline,
  cluster: Boxes,
  cloud: Cloud,
  combine: Combine,
  component: Component,
  container: Container,
  database: Database,
  device: Server,
  "device-type": Cpu,
  disk: Disc,
  factory: Factory,
  firewall: ShieldAlert,
  folder: FolderTree,
  gateway: RadioTower,
  globe: Globe2,
  hash: Hash,
  interface: Network,
  ip: Globe2,
  key: KeyRound,
  landmark: Landmark,
  laptop: Laptop,
  location: MapPin,
  lock: Lock,
  module: Package,
  monitor: Monitor,
  nas: HardDriveDownload,
  network: Network,
  phone: Phone,
  platform: Layers,
  plug: Plug,
  printer: Printer,
  provider: Waypoints,
  rack: HardDrive,
  region: Map,
  role: CircleDot,
  route: Route,
  router: Router,
  server: Server,
  shield: Shield,
  site: Building2,
  smartphone: Smartphone,
  stack: LayoutGrid,
  switch: EthernetPort,
  tag: Tag,
  tenant: Users,
  terminal: Monitor,
  target: Target,
  tv: Tv,
  vlan: Share2,
  vm: Cpu,
  vpn: ShieldCheck,
  wifi: Wifi,
};

export const ICON_PACK: readonly IconPackEntry[] = [
  { name: "server", labelKey: "inventory.resources.form.iconField.pack.server", fallback: "Server", Icon: Server },
  { name: "vm", labelKey: "inventory.resources.form.iconField.pack.vm", fallback: "VM", Icon: Cpu },
  { name: "router", labelKey: "inventory.resources.form.iconField.pack.router", fallback: "Router", Icon: Router },
  { name: "network", labelKey: "inventory.resources.form.iconField.pack.network", fallback: "Network", Icon: Network },
  { name: "gateway", labelKey: "inventory.resources.form.iconField.pack.gateway", fallback: "Gateway", Icon: RadioTower },
  { name: "database", labelKey: "inventory.resources.form.iconField.pack.database", fallback: "Database", Icon: Database },
  { name: "cloud", labelKey: "inventory.resources.form.iconField.pack.cloud", fallback: "Cloud", Icon: Cloud },
  { name: "shield", labelKey: "inventory.resources.form.iconField.pack.shield", fallback: "Security", Icon: Shield },
  { name: "rack", labelKey: "inventory.resources.form.iconField.pack.rack", fallback: "Rack", Icon: HardDrive },
  { name: "circuit", labelKey: "inventory.resources.form.iconField.pack.circuit", fallback: "Circuit", Icon: Spline },
  { name: "plug", labelKey: "inventory.resources.form.iconField.pack.plug", fallback: "Power", Icon: Plug },
  { name: "wifi", labelKey: "inventory.resources.form.iconField.pack.wifi", fallback: "Wireless", Icon: Wifi },
  { name: "monitor", labelKey: "inventory.resources.form.iconField.pack.monitor", fallback: "Monitor", Icon: Monitor },
  { name: "cctv", labelKey: "inventory.resources.form.iconField.pack.cctv", fallback: "CCTV", Icon: Cctv },
  { name: "laptop", labelKey: "inventory.resources.form.iconField.pack.laptop", fallback: "Laptop", Icon: Laptop },
  { name: "camera", labelKey: "inventory.resources.form.iconField.pack.camera", fallback: "Camera", Icon: Camera },
  { name: "phone", labelKey: "inventory.resources.form.iconField.pack.phone", fallback: "Phone", Icon: Phone },
  { name: "printer", labelKey: "inventory.resources.form.iconField.pack.printer", fallback: "Printer", Icon: Printer },
  { name: "tv", labelKey: "inventory.resources.form.iconField.pack.tv", fallback: "TV / Display", Icon: Tv },
  { name: "smartphone", labelKey: "inventory.resources.form.iconField.pack.smartphone", fallback: "Smartphone", Icon: Smartphone },
  { name: "antenna", labelKey: "inventory.resources.form.iconField.pack.antenna", fallback: "Antenna", Icon: Antenna },
  { name: "container", labelKey: "inventory.resources.form.iconField.pack.container", fallback: "Container", Icon: Container },
  { name: "firewall", labelKey: "inventory.resources.form.iconField.pack.firewall", fallback: "Firewall", Icon: ShieldAlert },
  { name: "switch", labelKey: "inventory.resources.form.iconField.pack.switch", fallback: "Switch", Icon: EthernetPort },
  { name: "nas", labelKey: "inventory.resources.form.iconField.pack.nas", fallback: "Storage / NAS", Icon: HardDriveDownload },
  { name: "disk", labelKey: "inventory.resources.form.iconField.pack.disk", fallback: "Disk", Icon: Disc },
  { name: "globe", labelKey: "inventory.resources.form.iconField.pack.globe", fallback: "Globe", Icon: Globe2 },
  { name: "lock", labelKey: "inventory.resources.form.iconField.pack.lock", fallback: "Lock", Icon: Lock },
  { name: "key", labelKey: "inventory.resources.form.iconField.pack.key", fallback: "Key", Icon: KeyRound },
  { name: "cable", labelKey: "inventory.resources.form.iconField.pack.cable", fallback: "Cable", Icon: Cable },
  { name: "cluster", labelKey: "inventory.resources.form.iconField.pack.cluster", fallback: "Cluster", Icon: Boxes },
];

const INVENTORY_ENTITY_ICON: Record<InventoryEntityKey, EntityIconName> = {
  Owners: "tenant",
  "Owner Groups": "folder",
  Sites: "site",
  Devices: "device",
  "Virtual Machines": "vm",
  "Virtual Machine Types": "cluster",
  Tenants: "tenant",
  "Tenant Groups": "folder",
  Regions: "region",
  "Device Roles": "role",
  Contacts: "tenant",
  "Contact Roles": "role",
  "Contact Groups": "folder",
  "Contact Assignments": "route",
  Manufacturers: "factory",
  "Site Groups": "stack",
  Platforms: "platform",
  "Device Types": "device-type",
  Racks: "rack",
  Interfaces: "interface",
  Cables: "cable",
  "Cluster Types": "cluster",
  "Cluster Groups": "stack",
  Clusters: "cluster",
  VLANs: "vlan",
  Prefixes: "hash",
  "IP Addresses": "ip",
  "VM Interfaces": "wifi",
  "Rack Roles": "rack",
  "Rack Types": "rack",
  Locations: "location",
  RIRs: "landmark",
  VRFs: "route",
  "Route Targets": "target",
  Aggregates: "combine",
  ASNs: "asn",
  "IP Ranges": "arrow-range",
  Services: "plug",
  Modules: "component",
  "Module Types": "module",
  "Module Type Profiles": "module",
  Tags: "tag",
  "Virtual Chassis": "stack",
  "Virtual Disks": "disk",
  Providers: "provider",
  "Circuit Types": "circuit",
  "Provider Networks": "network",
  Circuits: "circuit",
  "ASN Ranges": "arrow-range",
  "Inventory Item Roles": "role",
  "Inventory Items": "component",
  "Tunnel Groups": "network",
  Tunnels: "vpn",
  "Tunnel Terminations": "gateway",
  "IKE Policies": "shield",
  "IKE Proposals": "key",
  "IPsec Policies": "lock",
  "IPsec Proposals": "shield",
  "IPsec Profiles": "shield",
  "Circuit Groups": "combine",
  "Provider Accounts": "provider",
  "Circuit Terminations": "circuit",
  "Virtual Circuit Types": "circuit",
  "Virtual Circuits": "circuit",
  "Power Panels": "landmark",
  "Power Feeds": "plug",
  "Rack Reservations": "rack",
  "MAC Addresses": "hash",
  "Virtual Device Contexts": "platform",
  "Service Templates": "component",
};

const TREE_GROUP_ICON: Record<TreeGroupKey, EntityIconName> = {
  region: "region",
  site: "site",
  tenant: "tenant",
  role: "role",
  platform: "platform",
  deviceType: "device-type",
  cluster: "cluster",
};

const ROLE_LIKE_ENTITIES = new Set<InventoryEntityKey>([
  "Device Roles",
  "Rack Roles",
  "Inventory Item Roles",
  "Circuit Types",
  "Virtual Circuit Types",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isEntityIconName(value: unknown): value is EntityIconName {
  return typeof value === "string" && value in ENTITY_ICONS;
}

export function isSafeIconDataUrl(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length <= 96_000 &&
    /^data:image\/(?:png|jpe?g|webp);base64,[a-z0-9+/=]+$/i.test(value)
  );
}

export function inventoryEntityIconFor(entity: InventoryEntityKey): EntityIconName {
  return INVENTORY_ENTITY_ICON[entity] ?? "component";
}

export function treeGroupIconFor(key: TreeGroupKey): EntityIconName {
  return TREE_GROUP_ICON[key] ?? "component";
}

export function connectionTargetFallbackIcon(host: Pick<ConnectionTarget, "targetKind">): EntityIconName {
  return host.targetKind === "virtual_machine" ? "vm" : "device";
}

export function entitySupportsCustomIcon(entity: InventoryEntityKey): boolean {
  return ROLE_LIKE_ENTITIES.has(entity);
}

export function extractPersistedIcon(customFields: unknown): PersistedEntityIcon {
  const root = isRecord(customFields) ? customFields : {};
  const direct = root[CUSTOM_ICON_FIELD];
  const nested = isRecord(root.piping) ? root.piping.icon : undefined;
  const raw = isRecord(direct) ? direct : isRecord(nested) ? nested : undefined;
  if (!raw) return {};

  return {
    name: isEntityIconName(raw.name) ? raw.name : undefined,
    dataUrl: isSafeIconDataUrl(raw.data_url) ? raw.data_url : undefined,
  };
}

export function mergePersistedIcon(
  customFields: unknown,
  icon: PersistedEntityIcon,
): Record<string, unknown> | undefined {
  const next = isRecord(customFields) ? { ...customFields } : {};
  const hasIcon = !!icon.name || !!icon.dataUrl;
  if (hasIcon) {
    next[CUSTOM_ICON_FIELD] = {
      ...(icon.name ? { name: icon.name } : {}),
      ...(icon.dataUrl ? { data_url: icon.dataUrl } : {}),
    };
  } else {
    delete next[CUSTOM_ICON_FIELD];
  }
  return Object.keys(next).length > 0 ? next : undefined;
}
