import {
  Server,
  Network,
  Cpu,
  Building2,
  Cable,
  ShieldCheck,
  Layers,
  type LucideIcon,
} from "lucide-react";
import type { InventoryEntityKey } from "@/shared/api/types";

/**
 * The special config-layers rail entry. Config contexts are NOT a standard
 * inventory entity (no `ENTITY_API` entry, no `DataTable` flow): they render
 * their own management UI in the right pane. We model the rail key as a
 * superset {@link RailEntity} so this can live in the category rail without
 * widening {@link InventoryEntityKey} (which is `Record`-indexed across the
 * whole inventory API surface).
 */
export const CONFIG_CONTEXTS_RAIL_KEY = "config-contexts" as const;
export const IPAM_WORKSPACE_RAIL_KEY = "ipam-workspace" as const;
export const CABLING_WORKSTATION_RAIL_KEY = "cabling-workstation" as const;

/** A rail/chip key: a real inventory entity OR the config-layers pseudo-entity. */
export type RailEntity =
  | InventoryEntityKey
  | typeof CONFIG_CONTEXTS_RAIL_KEY
  | typeof IPAM_WORKSPACE_RAIL_KEY
  | typeof CABLING_WORKSTATION_RAIL_KEY;

/** True when a rail key is the config-layers pseudo-entity (not a real entity). */
export function isConfigContextsRailKey(
  key: RailEntity,
): key is typeof CONFIG_CONTEXTS_RAIL_KEY {
  return key === CONFIG_CONTEXTS_RAIL_KEY;
}

export function isIpamWorkspaceRailKey(
  key: RailEntity,
): key is typeof IPAM_WORKSPACE_RAIL_KEY {
  return key === IPAM_WORKSPACE_RAIL_KEY;
}

export function isCablingWorkstationRailKey(
  key: RailEntity,
): key is typeof CABLING_WORKSTATION_RAIL_KEY {
  return key === CABLING_WORKSTATION_RAIL_KEY;
}

export function isPseudoInventoryRailKey(
  key: RailEntity,
): key is
  | typeof CONFIG_CONTEXTS_RAIL_KEY
  | typeof IPAM_WORKSPACE_RAIL_KEY
  | typeof CABLING_WORKSTATION_RAIL_KEY {
  return isConfigContextsRailKey(key) || isIpamWorkspaceRailKey(key) || isCablingWorkstationRailKey(key);
}

/**
 * Category-grouped information architecture for the inventory page.
 *
 * Visible category links select the active group; one searchable picker
 * selects that category's entities and workspaces on every viewport.
 *
 * Every key here MUST exist in `tabConfig` / `ENTITY_API`; the ordering inside
 * each category drives the picker order.
 */
export interface InventoryCategory {
  key: "dcim" | "ipam" | "virtualization" | "tenancy" | "circuits" | "vpn" | "provisioning";
  /** i18n key under inventory.resources.categories. */
  labelKey: string;
  /** i18n key for the rail subtitle under inventory.resources.categories. */
  descriptionKey: string;
  icon: LucideIcon;
  /** Entities surfaced in this category's picker. May include the
   *  config-layers pseudo-entity, which renders a bespoke pane (not a table). */
  entities: RailEntity[];
}

/** The DCIM category is the default landing group; exported as a guaranteed
 *  non-undefined fallback so consumers avoid `noUncheckedIndexedAccess` noise. */
export const DEFAULT_CATEGORY: InventoryCategory = {
  key: "dcim",
  labelKey: "inventory.resources.categories.dcim",
  descriptionKey: "inventory.resources.categories.dcimDescription",
  icon: Server,
  entities: [
    "Sites",
    "Locations",
    "Regions",
    "Site Groups",
    "Racks",
    "Rack Roles",
    "Rack Types",
    "Devices",
    "Interfaces",
    "Device Roles",
    "Device Types",
    "Manufacturers",
    "Platforms",
    "Modules",
    "Module Types",
    "Module Type Profiles",
    "Tags",
    "Virtual Chassis",
    "Inventory Item Roles",
    "Inventory Items",
    "Power Panels",
    "Power Feeds",
    "Rack Reservations",
    "MAC Addresses",
    "Virtual Device Contexts",
  ],
};

export const inventoryCategories: InventoryCategory[] = [
  DEFAULT_CATEGORY,
  {
    key: "ipam",
    labelKey: "inventory.resources.categories.ipam",
    descriptionKey: "inventory.resources.categories.ipamDescription",
    icon: Network,
    entities: [
      IPAM_WORKSPACE_RAIL_KEY,
      "Prefixes",
      "IP Addresses",
      "IP Ranges",
      "Aggregates",
      "VLANs",
      "VRFs",
      "Route Targets",
      "RIRs",
      "ASNs",
      "ASN Ranges",
      "Services",
      "Service Templates",
    ],
  },
  {
    key: "virtualization",
    labelKey: "inventory.resources.categories.virtualization",
    descriptionKey: "inventory.resources.categories.virtualizationDescription",
    icon: Cpu,
    entities: [
      "Clusters",
      "Cluster Types",
      "Cluster Groups",
      "Virtual Machines",
      "Virtual Machine Types",
      "VM Interfaces",
      "Virtual Disks",
    ],
  },
  {
    key: "tenancy",
    labelKey: "inventory.resources.categories.tenancy",
    descriptionKey: "inventory.resources.categories.tenancyDescription",
    icon: Building2,
    entities: [
      "Tenants",
      "Tenant Groups",
      "Contacts",
      "Contact Roles",
      "Contact Groups",
      "Contact Assignments",
      "Owners",
      "Owner Groups",
    ],
  },
  {
    key: "circuits",
    labelKey: "inventory.resources.categories.circuits",
    descriptionKey: "inventory.resources.categories.circuitsDescription",
    icon: Cable,
    entities: [
      "Providers", "Circuits", "Circuit Types", CABLING_WORKSTATION_RAIL_KEY, "Cables", "Provider Networks",
      "Circuit Groups", "Provider Accounts", "Circuit Terminations",
      "Virtual Circuit Types", "Virtual Circuits",
    ],
  },
  {
    key: "vpn",
    labelKey: "inventory.resources.categories.vpn",
    descriptionKey: "inventory.resources.categories.vpnDescription",
    icon: ShieldCheck,
    entities: [
      "Tunnels",
      "Tunnel Groups",
      "Tunnel Terminations",
      "IKE Policies",
      "IKE Proposals",
      "IPsec Policies",
      "IPsec Proposals",
      "IPsec Profiles",
    ],
  },
  {
    key: "provisioning",
    labelKey: "inventory.resources.categories.provisioning",
    descriptionKey: "inventory.resources.categories.provisioningDescription",
    icon: Layers,
    entities: [CONFIG_CONTEXTS_RAIL_KEY],
  },
];

/** Flat ordered list of every *real* inventory entity surfaced through the
 *  category rail — the canonical set the page fetches counts for (replaces the
 *  old TAB_KEYS array). The config-layers pseudo-entity is excluded: it has no
 *  `listResources` count query. Interfaces and Cables are both surfaced because
 *  inspectors deep-link to their dedicated inventory tables. */
export const CATEGORIZED_ENTITIES: InventoryEntityKey[] = inventoryCategories
  .flatMap((category) => category.entities)
  .filter((key): key is InventoryEntityKey => !isPseudoInventoryRailKey(key));

/** The default rail entry to open when switching to a category (its first
 *  entry). May be the config-layers pseudo-entity for the provisioning
 *  category. */
export function firstEntityOf(categoryKey: InventoryCategory["key"]): RailEntity {
  const category = inventoryCategories.find((c) => c.key === categoryKey);
  return category?.entities[0] ?? "Sites";
}

/** Resolve which category owns a given rail entry (for deep-link / restore). */
export function categoryOf(entity: RailEntity): InventoryCategory["key"] {
  return inventoryCategories.find((c) => c.entities.includes(entity))?.key ?? "dcim";
}
