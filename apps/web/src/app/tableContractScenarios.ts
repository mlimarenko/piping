import { tabConfig } from "@/features/inventory/components/inventory/inventoryTypes";
import { inventoryResourceChildRelations, inventoryResourceChildTableId } from "@/features/inventory/components/inventory/inventoryResourceChildLists";

// Hand-maintained carrier of the browser parity scenario ids for the
// standardized table contracts (PIP-125).
//
// The set used to be parsed out of the 08-table-column-contracts e2e spec with
// `fs`; it lives here now, so a spec file is no longer a build input of the unit
// suite. Both arrays keep the order that spec declared them in: the 70
// `inventory.${entity}` scenarios generated from its INVENTORY_TABLE_ENTITIES
// list, and the 43 literal `tableId: "..."` ones.
//
// TABLE_CONTRACT_SCENARIO_IDS must equal STANDARDIZED_TABLE_IDS from
// ./tableContracts as a set; tableContracts.test.ts reconciles
// them, so a new table id grows this file in the same pass as the registry.

/**
 * Generated inventory scenarios: `inventory.${entity}` for every
 * INVENTORY_TABLE_ENTITIES entry, in the retired spec's declaration order.
 */
const INVENTORY_TABLE_SCENARIO_IDS: readonly string[] = [
  "inventory.Owners",
  "inventory.Owner Groups",
  "inventory.Sites",
  "inventory.Devices",
  "inventory.Virtual Machines",
  "inventory.Virtual Machine Types",
  "inventory.Tenants",
  "inventory.Tenant Groups",
  "inventory.Regions",
  "inventory.Device Roles",
  "inventory.Contacts",
  "inventory.Contact Roles",
  "inventory.Contact Groups",
  "inventory.Contact Assignments",
  "inventory.Manufacturers",
  "inventory.Site Groups",
  "inventory.Platforms",
  "inventory.Device Types",
  "inventory.Racks",
  "inventory.Interfaces",
  "inventory.Cables",
  "inventory.Cluster Types",
  "inventory.Cluster Groups",
  "inventory.Clusters",
  "inventory.VLANs",
  "inventory.Prefixes",
  "inventory.IP Addresses",
  "inventory.VM Interfaces",
  "inventory.Rack Roles",
  "inventory.Rack Types",
  "inventory.Locations",
  "inventory.RIRs",
  "inventory.VRFs",
  "inventory.Route Targets",
  "inventory.Aggregates",
  "inventory.ASNs",
  "inventory.IP Ranges",
  "inventory.Services",
  "inventory.Modules",
  "inventory.Module Types",
  "inventory.Module Type Profiles",
  "inventory.Tags",
  "inventory.Virtual Chassis",
  "inventory.Virtual Disks",
  "inventory.Providers",
  "inventory.Circuit Types",
  "inventory.Provider Networks",
  "inventory.Circuits",
  "inventory.ASN Ranges",
  "inventory.Inventory Item Roles",
  "inventory.Inventory Items",
  "inventory.Tunnel Groups",
  "inventory.Tunnels",
  "inventory.Tunnel Terminations",
  "inventory.IKE Policies",
  "inventory.IKE Proposals",
  "inventory.IPsec Policies",
  "inventory.IPsec Proposals",
  "inventory.IPsec Profiles",
  "inventory.Circuit Groups",
  "inventory.Provider Accounts",
  "inventory.Circuit Terminations",
  "inventory.Virtual Circuit Types",
  "inventory.Virtual Circuits",
  "inventory.Power Panels",
  "inventory.Power Feeds",
  "inventory.Rack Reservations",
  "inventory.MAC Addresses",
  "inventory.Virtual Device Contexts",
  "inventory.Service Templates",
];

/** Literal non-inventory scenarios, in the retired spec's declaration order. */
const NON_RESOURCE_TABLE_SCENARIO_IDS: readonly string[] = [
  "inventory.config-contexts",
  "inventory.config-context-bindings",
  "inventory.customFields.definitions",
  "inventory.customFields.choiceSets",
  "inventory.ipamWorkspace.prefixes",
  "inventory.syncProviders",
  "inventory.syncMappings",
  "inventory.templates",
  "inventory.templateRuns",
  "connections.active",
  "connections.targets.desktop",
  "connections.targets.mobile",
  "connection-config.records",
  "connection-config.assignments",
  "connection-config.effective-preview",
  "connections.proxies",
  "connections.proxyAssignments",
  "audit.log",
  "backup.preview",
  "webhooks.list",
  "observability.exporters",
  "observability.inspector.profiles",
  "observability.inspector.metric_cards",
  "observability.inspector.metric_groups",
  "observability.inspector.sources",
  "observability.inspector.alert_sources",
  "observability.inspector.alert_assignments",
  "access.users",
  "access.groups",
  "access.roles",
  "access.grants",
  "access.policies",
  "userSettings.credentials",
  "userSettings.tokens",
  "playbooks.assignments",
  "playbooks.projects",
  "playbooks.jobTemplates",
  "playbooks.executionEnvironments",
  "automation-secrets.list",
  "playbooks.targetSets",
  "playbooks.schedules",
  "playbooks.servicePrincipals",
  "playbooks.runs",
];

/**
 * Full scenario set (unique, sorted), the loader output order of the retired
 * spec reader; must equal STANDARDIZED_TABLE_IDS as a set.
 *
 * Typed `readonly string[]` rather than an `as const` literal tuple on purpose:
 * the reconciling test spreads the list and calls `.includes(tableId)` with an
 * unconstrained `string`, which a literal-element array type rejects.
 */
export const TABLE_CONTRACT_SCENARIO_IDS: readonly string[] = [
  ...INVENTORY_TABLE_SCENARIO_IDS,
  ...NON_RESOURCE_TABLE_SCENARIO_IDS,
  // Scoped child projections derive their identities from the same relationship
  // declarations that supply their routes, rather than a second five-row matrix.
  ...tabConfig.flatMap(parent => inventoryResourceChildRelations(parent.key)
    .map(relation => inventoryResourceChildTableId(parent.key, relation))),
];
