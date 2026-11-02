/**
 * Stateful in-memory mock backend for demo mode.
 *
 * A module-level store (`Map<resourceKey, Record[]>`) seeded on first import with
 * believable data for the product's main resources. Generic CRUD operations
 * mutate this store so create/edit/delete actions taken in the demo are reflected
 * by subsequent list/detail reads *within the same page session*.
 *
 * Because the store lives in module memory (NOT localStorage), a full page reload
 * re-imports this module, re-runs the seed, and the demo snaps back to a clean,
 * untouched state. This is intentional: "после обновления демо слетает в чистый
 * вид". Do NOT persist demo mutations to localStorage/sessionStorage.
 *
 * Everything here is demo-only — only `src/demo/handlers.ts` imports it, and that
 * module is only pulled in by the `--mode demo` entry. The real app never touches
 * this file.
 */

import type { InventoryEntityKey } from "@/shared/api/types";
import type { components } from "@/shared/api/schema.gen";
import { inventoryModelCapabilities } from "@/features/inventory/components/inventory/inventoryTypes";
import { createDemoDataflows } from "./dataflows";

const NOW = Date.parse("2026-06-02T11:20:00Z");
const iso = (minutesAgo: number) => new Date(NOW - minutesAgo * 60_000).toISOString();

/** A demo record always carries a string `id`; everything else is free-form. */
export type DemoRecord = Record<string, unknown> & { id: string };

/** Registry entry describing one mockable resource collection. */
interface ResourceSpec {
  model?: InventoryEntityKey;
  /** Seed rows inserted on init / reset. */
  seed: DemoRecord[];
}

/**
 * The live store. Keys are the canonical resource path used by the handlers
 * (e.g. `inventory/sites`, `admin/access-control/users`). Seeded lazily on first
 * access via `ensureSeeded`.
 */
const store = new Map<string, DemoRecord[]>();

/** Resource specs by key. Drives both seeding and list-shape selection. */
const SPECS = new Map<string, ResourceSpec>();

let seeded = false;

function register(key: string, seed: DemoRecord[], model?: InventoryEntityKey): void {
  SPECS.set(key, { seed, model });
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** Seed (or re-seed) the store from the registered specs. Idempotent per import. */
function ensureSeeded(): void {
  if (seeded) return;
  for (const [key, spec] of SPECS) {
    store.set(key, clone(spec.seed));
  }
  seeded = true;
}

/** True when `key` is a registered store collection (has a seed spec). */
export function isCollection(key: string): boolean {
  return SPECS.has(key);
}

/** Returns the live rows for a key, creating an empty collection if needed. */
export function rows(key: string): DemoRecord[] {
  ensureSeeded();
  let list = store.get(key);
  if (!list) {
    list = [];
    store.set(key, list);
  }
  return list;
}

export function findById(key: string, id: string): DemoRecord | undefined {
  const record = rows(key).find((r) => r.id === id);
  return record ? hydrateCablingRelation(key, record) : undefined;
}

function hydrateCablingRelation(key: string, record: DemoRecord): DemoRecord {
  const relation = key === "inventory/racks"
    ? { collection: "inventory/rack-groups", id: "group_id", name: "group_name" }
    : key === "inventory/cables"
      ? { collection: "inventory/cable-bundles", id: "bundle_id", name: "bundle_name" }
      : undefined;
  if (!relation) return record;
  const related = rows(relation.collection).find((row) => row.id === record[relation.id]);
  return { ...record, [relation.name]: related?.name ?? null };
}

/** Demo defaults for freshly created hardware component templates: the native
 *  records carry no tags, custom fields, status, comments or timestamps. */
function componentTemplateDefaults(model?: InventoryEntityKey): Record<string, unknown> {
  switch (model) {
    case "Module Bay Templates":
      return { label: "", position: "", description: "", enabled: true, module_bay_types: [] };
    case "Console Port Templates":
    case "Console Server Port Templates":
      return { label: "", description: "", type: null, device_type_id: null, module_type_id: null };
    case "Power Port Templates":
      return { label: "", description: "", type: null, maximum_draw: null, allocated_draw: null, device_type_id: null, module_type_id: null };
    case "Power Outlet Templates":
      return { label: "", description: "", type: null, color: "", power_port_id: null, feed_leg: null, device_type_id: null, module_type_id: null };
    case "Interface Templates":
      return { label: "", description: "", enabled: true, mgmt_only: false, bridge_id: null, poe_mode: null, poe_type: null, rf_role: null, device_type_id: null, module_type_id: null };
    case "Front Port Templates":
    case "Rear Port Templates":
      return { label: "", description: "", type: "", color: "", positions: 1, device_type_id: null, module_type_id: null };
    case "Device Bay Templates":
      return { label: "", description: "" };
    case "Inventory Item Templates":
      return { label: "", description: "", parent_id: null, manufacturer_id: null, role_id: null, part_id: "" };
    default:
      return {};
  }
}

export function insert(key: string, body: Record<string, unknown>): DemoRecord {
  const list = rows(key);
  const id = typeof body.id === "string" && body.id ? body.id : crypto.randomUUID();
  const model = SPECS.get(key)?.model;
  const hasChangeDates = !model || inventoryModelCapabilities(model)?.change_dates !== false;
  const cooling = model?.startsWith("Cooling ");
  const collectionDates = cooling || model === "Rack Groups" || model === "Cable Bundles";
  const record: DemoRecord = {
    ...componentTemplateDefaults(model),
    ...(model === "Tags" ? { object_types: [] } : {}),
    ...(collectionDates ? { created: iso(0), last_updated: iso(0),
      ...(cooling ? { description: "", ...(model === "Cooling Sources" || model === "Cooling Feeds" ? { status: "active", comments: "" } : { label: "" }),
        ...(inventoryModelCapabilities(model!)?.custom_fields ? { custom_fields: {}, tags: [] } : {}) }
        : { description: "", comments: "", custom_fields: {} }) }
      : hasChangeDates ? { status: "active", created_at: iso(0), updated_at: iso(0) } : {}),
    ...body,
    id,
  };
  list.unshift(record);
  return record;
}

export function update(key: string, id: string, body: Record<string, unknown>): DemoRecord {
  const list = rows(key);
  const idx = list.findIndex((r) => r.id === id);
  if (idx === -1) {
    // Unknown id: synthesize a record so edit-on-a-fresh-seed never errors.
    return insert(key, { ...body, id });
  }
  const model = SPECS.get(key)?.model;
  const dates = model?.startsWith("Cooling ") || model === "Rack Groups" || model === "Cable Bundles"
    ? { last_updated: iso(0) }
    : !model || inventoryModelCapabilities(model)?.change_dates !== false ? { updated_at: iso(0) } : {};
  const merged: DemoRecord = { ...list[idx], ...body, id, ...dates };
  list[idx] = merged;
  return merged;
}

export function remove(key: string, id: string): void {
  const list = rows(key);
  const idx = list.findIndex((r) => r.id === id);
  if (idx !== -1) list.splice(idx, 1);
}

/** Build the canonical paginated envelope, honouring limit/offset/search. */
export function listEnvelope(
  key: string,
  opts: { limit?: number; offset?: number; search?: string } = {},
): { items: DemoRecord[]; total: number; limit: number; offset: number } {
  const all = filtered(key, opts.search);
  const offset = opts.offset ?? 0;
  const limit = opts.limit ?? 1000;
  const items = all.slice(offset, offset + limit).map((record) => hydrateCablingRelation(key, record));
  return { items, total: all.length, limit, offset };
}

function filtered(key: string, search?: string): DemoRecord[] {
  const all = rows(key);
  const q = search?.trim().toLowerCase();
  if (!q) return all;
  const str = (v: unknown): string => (typeof v === "string" ? v : "");
  return all.filter((r) => {
    const name = (str(r.name) || str(r.username) || str(r.slug)).toLowerCase();
    return name.includes(q);
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Seeds. Realistic-but-small fixtures so the main screens look populated. The
// connections workspace fixtures (hosts / launch options / sessions) stay in
// handlers.ts; everything else the navigable screens read lives here.
// ─────────────────────────────────────────────────────────────────────────────

const SITE_AMS = { id: "site-ams-0000-0000-0000-000000000001", name: "Amsterdam Edge", slug: "ams" };
const SITE_FRA = { id: "site-fra-0000-0000-0000-000000000002", name: "Frankfurt DC", slug: "fra" };
const TENANT_CORE = { id: "ten-core-0000-0000-0000-000000000001", name: "Core Infrastructure" };
const TENANT_CORP = { id: "ten-corp-0000-0000-0000-000000000002", name: "Corporate IT" };

register("inventory/dataflows", createDemoDataflows());
register("inventory/fhrp-groups", [{
  id: "a0000000-0000-4000-8000-000000000001", name: "Amsterdam gateway", protocol: "vrrp3", group_id: 10,
  auth_type: "", description: "Redundant gateway for the edge network", comments: "", owner_id: null,
  custom_fields: {}, tags: [], external_id: null, piping_synced_at: null, created: iso(100), last_updated: iso(10),
}], "FHRP Groups");
register("inventory/fhrp-group-assignments", [{
  id: "a0000000-0000-4000-8000-000000000002", group_id: "a0000000-0000-4000-8000-000000000001",
  interface_type: "dcim.interface", interface_id: "a0000000-0000-4000-8000-000000000003",
  priority: 100, created: iso(100), last_updated: iso(10), external_id: null, piping_synced_at: null,
}], "FHRP Group Assignments");
register("inventory/interfaces", [{
  id: "a0000000-0000-4000-8000-000000000003", name: "ether1", type: "1000base-t", enabled: true,
  device_id: "host-0000-0000-0000-0000-000000000005", device_name: "edge-router-01",
}], "Interfaces");
register("custom-fields", []);
register("custom-field-choice-sets", []);

// ── Inventory ────────────────────────────────────────────────────────────────
const OWNER_GROUP_ID = "a0000000-0000-4000-8000-000000000001";
register("inventory/owner_groups", [
  { id: OWNER_GROUP_ID, name: "Operations", description: "Infrastructure teams" },
], "Owner Groups");
register("inventory/owners", [
  { id: "a0000000-0000-4000-8000-000000000002", name: "Platform", description: "Platform ownership", group_id: OWNER_GROUP_ID, users: [], user_groups: [] },
], "Owners");

register("inventory/sites", [
  { id: SITE_AMS.id, name: SITE_AMS.name, slug: SITE_AMS.slug, latitude: "52.3676", longitude: "4.9041", status: "active", tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name },
  { id: SITE_FRA.id, name: SITE_FRA.name, slug: SITE_FRA.slug, latitude: "50.1109", longitude: "8.6821", status: "active", tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name },
  { id: "site-lab-0000-0000-0000-000000000003", name: "Remote Lab", slug: "remote-lab", status: "active", tenant_id: TENANT_CORP.id, tenant_name: TENANT_CORP.name },
]);

register("inventory/tenants", [
  { id: TENANT_CORE.id, name: TENANT_CORE.name, slug: "core-infra", status: "active" },
  { id: TENANT_CORP.id, name: TENANT_CORP.name, slug: "corporate-it", status: "active" },
]);

register("inventory/tenant_groups", [
  { id: "tg-0000-0000-0000-000000000001", name: "Production", slug: "production", status: "active" },
  { id: "tg-0000-0000-0000-000000000002", name: "Staging", slug: "staging", status: "active" },
]);

register("inventory/regions", [
  { id: "rg-0000-0000-0000-000000000001", name: "EU West", slug: "eu-west", status: "active" },
  { id: "rg-0000-0000-0000-000000000002", name: "EU Central", slug: "eu-central", status: "active" },
]);

register("inventory/site_groups", [
  { id: "sg-0000-0000-0000-000000000001", name: "Edge Sites", slug: "edge", status: "active" },
  { id: "sg-0000-0000-0000-000000000002", name: "Core Datacenters", slug: "core-dc", status: "active" },
]);

const CORE_TAG_ID = "00000000-0000-4000-8000-000000009401";
const CAMPUS_TAG_ID = "00000000-0000-4000-8000-000000009402";
register("inventory/tags", [
  { id: CORE_TAG_ID, name: "Core network", slug: "core-network", color: "155dfc", description: "Shared network infrastructure", weight: 0, object_types: [] },
  { id: CAMPUS_TAG_ID, name: "Campus", slug: "campus", color: "008236", description: "Campus infrastructure", weight: 100, object_types: [] },
]);
register("inventory/wireless_lan_groups", [
  { id: "00000000-0000-4000-8000-000000009101", name: "Campus Wi-Fi", slug: "campus-wifi", parent_id: null, description: "Campus wireless networks", comments: "", tags: [CORE_TAG_ID, CAMPUS_TAG_ID], custom_fields: {} },
]);
const MODULE_BAY_TYPE_ID = "00000000-0000-4000-8000-000000009201";
const MODULE_TYPE_ID = "00000000-0000-4000-8000-000000009202";
const MODULE_BAY_ID = "00000000-0000-4000-8000-000000009203";
register("inventory/module-bay-types", [
  { id: MODULE_BAY_TYPE_ID, name: "SFP uplink", slug: "sfp-uplink", manufacturer_id: null, color: "155dfc", description: "Pluggable uplink modules", comments: "Synthetic demo data", owner_id: null, tags: [], custom_fields: {}, created: iso(60), last_updated: null },
]);
register("inventory/module-types", [
  { id: MODULE_TYPE_ID, model: "10G SFP+", manufacturer_id: "mf-0000-0000-0000-000000000003", module_bay_types: [MODULE_BAY_TYPE_ID] },
]);
register("inventory/module-bays", [
  { id: MODULE_BAY_ID, name: "Uplink 1", device_id: "host-0000-0000-0000-0000-000000000005", device_name: "edge-router-01", module_id: null, enabled: true, module_bay_types: [MODULE_BAY_TYPE_ID], parent_id: null, installed_module: null, occupied: false, is_module_compatible: true, label: "WAN", position: "1", description: "Uplink slot" },
]);
register("inventory/module-bay-templates", [
  { id: "00000000-0000-4000-8000-000000009204", name: "Uplink Slot Template", label: "WAN", position: "2", enabled: false, description: "Synthetic module slot template", device_type_id: null, module_type_id: MODULE_TYPE_ID, module_bay_types: [MODULE_BAY_TYPE_ID] },
], "Module Bay Templates");
// Native DCIM component templates: one hardware parent, no tags/CF/status.
const DEMO_DEVICE_TYPE_ID = "dt-0000-0000-0000-000000000001";
const INVENTORY_ITEM_ROLE_ID = "00000000-0000-4000-8000-000000009601";
const POWER_PORT_TEMPLATE_ID = "00000000-0000-4000-8000-000000009703";
const REAR_PORT_TEMPLATE_ID = "00000000-0000-4000-8000-000000009707";
register("inventory/inventory-item-roles", [
  { id: INVENTORY_ITEM_ROLE_ID, name: "Power supply", slug: "power-supply", color: "155dfc", description: "Field-replaceable power units" },
], "Inventory Item Roles");
register("inventory/console-port-templates", [
  { id: "00000000-0000-4000-8000-000000009701", name: "Console", label: "OOB", type: "usb-c", description: "Out-of-band console connector", device_type_id: DEMO_DEVICE_TYPE_ID, module_type_id: null },
], "Console Port Templates");
register("inventory/console-server-port-templates", [
  { id: "00000000-0000-4000-8000-000000009702", name: "Console server", label: "", type: "rj-45", description: "Console server port on the uplink module", device_type_id: null, module_type_id: MODULE_TYPE_ID },
], "Console Server Port Templates");
register("inventory/power-port-templates", [
  { id: POWER_PORT_TEMPLATE_ID, name: "PSU 1", label: "PSU A", type: "iec-60320-c14", maximum_draw: 450, allocated_draw: 300, description: "Primary power inlet", device_type_id: DEMO_DEVICE_TYPE_ID, module_type_id: null },
], "Power Port Templates");
register("inventory/power-outlet-templates", [
  { id: "00000000-0000-4000-8000-000000009704", name: "PSU outlet", label: "", type: "iec-60320-c13", color: "155dfc", power_port_id: POWER_PORT_TEMPLATE_ID, feed_leg: "A", description: "Outlet fed by PSU 1", device_type_id: DEMO_DEVICE_TYPE_ID, module_type_id: null },
], "Power Outlet Templates");
register("inventory/interface-templates", [
  { id: "00000000-0000-4000-8000-000000009705", name: "mgmt0", label: "MGMT", type: "1000base-t", enabled: true, mgmt_only: true, bridge_id: null, poe_mode: null, poe_type: null, rf_role: null, description: "Management interface template", device_type_id: DEMO_DEVICE_TYPE_ID, module_type_id: null },
], "Interface Templates");
register("inventory/front-port-templates", [
  { id: "00000000-0000-4000-8000-000000009706", name: "Front 1", label: "", type: "lc", color: "", positions: 2, description: "Duplex front port", device_type_id: DEMO_DEVICE_TYPE_ID, module_type_id: null,
    rear_ports: [{ position: 1, rear_port: REAR_PORT_TEMPLATE_ID, rear_port_position: 2 }, { position: 2, rear_port: REAR_PORT_TEMPLATE_ID, rear_port_position: 1 }] },
], "Front Port Templates");
register("inventory/rear-port-templates", [
  { id: REAR_PORT_TEMPLATE_ID, name: "Rear 1", label: "", type: "lc", color: "", positions: 2, description: "Duplex rear port", device_type_id: DEMO_DEVICE_TYPE_ID, module_type_id: null,
    front_ports: [{ position: 2, front_port: "00000000-0000-4000-8000-000000009706", front_port_position: 1 }, { position: 1, front_port: "00000000-0000-4000-8000-000000009706", front_port_position: 2 }] },
], "Rear Port Templates");
register("inventory/device-bay-templates", [
  { id: "00000000-0000-4000-8000-000000009708", name: "Bay 1", label: "GPU", description: "Accelerator bay", device_type_id: DEMO_DEVICE_TYPE_ID },
], "Device Bay Templates");
register("inventory/inventory-item-templates", [
  { id: "00000000-0000-4000-8000-000000009709", name: "PSU", label: "Power supply", description: "", device_type_id: DEMO_DEVICE_TYPE_ID, parent_id: null, manufacturer_id: "mf-0000-0000-0000-000000000003", role_id: INVENTORY_ITEM_ROLE_ID, part_id: "74-000123", component_type_id: "ct-dcim-powerporttemplate", component_id: POWER_PORT_TEMPLATE_ID },
], "Inventory Item Templates");
register("inventory/modules", []);
const COOLING_SOURCE_ID = "00000000-0000-4000-8000-000000009301";
const COOLING_INTAKE_ID = "00000000-0000-4000-8000-000000009303";
register("inventory/cooling-sources", [
  { id: COOLING_SOURCE_ID, name: "AMS Chiller", type: "chiller", status: "active", fluid_type: "water-glycol", cooling_capacity: 120.5, site_id: SITE_AMS.id, location_id: null, description: "Synthetic facility cooling source", comments: "Synthetic demo data", owner_id: null, tags: [], custom_fields: {}, created: iso(60), last_updated: null },
], "Cooling Sources");
register("inventory/cooling-feeds", [
  { id: "00000000-0000-4000-8000-000000009302", name: "AMS Rack Loop", status: "active", cooling_source_id: COOLING_SOURCE_ID, rack_id: "rk-0000-0000-0000-000000000001", tenant_id: null, cooling_capacity: 30, max_flow: 12.5, max_flow_unit: "lpm", description: "Supply and return loop", comments: "", owner_id: null, tags: [], custom_fields: {}, created: iso(50), last_updated: null },
], "Cooling Feeds");
register("inventory/cooling-intakes", [
  { id: COOLING_INTAKE_ID, name: "CDU Inlet", label: "Supply", type: "qdc", device_id: "host-0000-0000-0000-0000-000000000005", module_id: null, cooling_outflow_id: null, diameter: 12.7, diameter_unit: "mm", max_flow: 12.5, max_flow_unit: "lpm", description: "Manifold supply inlet", owner_id: null, tags: [], custom_fields: {}, created: iso(40), last_updated: null },
], "Cooling Intakes");
register("inventory/cooling-outflows", [
  { id: "00000000-0000-4000-8000-000000009304", name: "CDU Outlet", label: "Server loop", type: "qdc", device_id: "host-0000-0000-0000-0000-000000000005", module_id: null, cooling_intake_id: COOLING_INTAKE_ID, diameter: 12.7, diameter_unit: "mm", description: "Manifold distribution outlet", owner_id: null, tags: [], custom_fields: {}, created: iso(30), last_updated: null },
], "Cooling Outflows");
register("inventory/cooling-intake-templates", [
  { id: "00000000-0000-4000-8000-000000009305", name: "Coolant Inlet", label: "Supply", type: "uqd", module_type_id: MODULE_TYPE_ID, device_type_id: null, diameter: 6.35, diameter_unit: "mm", max_flow: 1.5, max_flow_unit: "lpm", description: "Synthetic module inlet template", created: iso(20), last_updated: null },
], "Cooling Intake Templates");
register("inventory/cooling-outflow-templates", [
  { id: "00000000-0000-4000-8000-000000009306", name: "Coolant Outlet", label: "Return", type: "uqd", module_type_id: MODULE_TYPE_ID, device_type_id: null, cooling_intake_id: "00000000-0000-4000-8000-000000009305", diameter: 6.35, diameter_unit: "mm", description: "Synthetic module outlet template", created: iso(10), last_updated: null },
], "Cooling Outflow Templates");
register("inventory/wireless_lans", [
  { id: "00000000-0000-4000-8000-000000009102", ssid: "Campus Staff", group_id: "00000000-0000-4000-8000-000000009101", status: "active", auth_type: "wpa-enterprise", auth_cipher: "aes", auth_psk: "", description: "Staff wireless network", comments: "", tags: [], custom_fields: {} },
]);
register("inventory/wireless_links", [
  { id: "00000000-0000-4000-8000-000000009103", ssid: "Campus Bridge", interface_a_id: "00000000-0000-4000-8000-000000009104", interface_b_id: "00000000-0000-4000-8000-000000009105", status: "connected", distance: 120.5, distance_unit: "m", auth_psk: "", description: "Building-to-building bridge", comments: "", tags: [], custom_fields: {} },
]);
register("inventory/interfaces", [
  { id: "00000000-0000-4000-8000-000000009104", name: "AP East · wlan0", type: "ieee802.11ax", enabled: true },
  { id: "00000000-0000-4000-8000-000000009105", name: "AP West · wlan0", type: "ieee802.11ax", enabled: true },
]);

register("inventory/device_roles", [
  { id: "role-srv-0000-0000-0000-000000000001", name: "Linux Server", slug: "linux-server", status: "active", custom_fields: { piping_icon: { name: "server" } } },
  { id: "role-jump-0000-0000-0000-00000000002", name: "Jump Host", slug: "jump-host", status: "active", custom_fields: { piping_icon: { name: "monitor" } } },
  { id: "role-rtr-0000-0000-0000-000000000004", name: "Router", slug: "router", status: "active", custom_fields: { piping_icon: { name: "router" } } },
]);

register("inventory/manufacturers", [
  { id: "mf-0000-0000-0000-000000000001", name: "Dell", slug: "dell" },
  { id: "mf-0000-0000-0000-000000000002", name: "HPE", slug: "hpe" },
  { id: "mf-0000-0000-0000-000000000003", name: "Cisco", slug: "cisco" },
]);

register("inventory/platforms", [
  { id: "pf-0000-0000-0000-000000000001", name: "Ubuntu 22.04", slug: "ubuntu-2204" },
  { id: "pf-0000-0000-0000-000000000002", name: "Debian 12", slug: "debian-12" },
  { id: "pf-0000-0000-0000-000000000003", name: "Windows Server 2022", slug: "win-2022" },
]);

register("inventory/device_types", [
  { id: "dt-0000-0000-0000-000000000001", name: "PowerEdge R650", slug: "r650", model: "PowerEdge R650", manufacturer_id: "mf-0000-0000-0000-000000000001", manufacturer_name: "Dell", u_height: 1, status: "active", subdevice_role: "parent" },
  { id: "dt-0000-0000-0000-000000000002", name: "ProLiant DL360", slug: "dl360", model: "ProLiant DL360", manufacturer_id: "mf-0000-0000-0000-000000000002", manufacturer_name: "HPE", u_height: 1, status: "active", subdevice_role: "child" },
]);

register("inventory/devices", [
  { id: "host-0000-0000-0000-0000-000000000004", name: "win-rdp-jump", status: "active", primary_ip: "192.0.2.41", site_id: SITE_AMS.id, site_name: SITE_AMS.name, tenant_id: TENANT_CORP.id, tenant_name: TENANT_CORP.name, role_id: "role-jump-0000-0000-0000-00000000002", role_name: "Jump Host" },
  { id: "host-0000-0000-0000-0000-000000000012", name: "fra-db-01", status: "active", primary_ip: "198.51.100.21", site_id: SITE_FRA.id, site_name: SITE_FRA.name, tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name, role_id: "role-srv-0000-0000-0000-000000000001", role_name: "Linux Server" },
  { id: "host-0000-0000-0000-0000-000000000014", name: "fra-rdp-jump", status: "active", primary_ip: "198.51.100.41", site_id: SITE_FRA.id, site_name: SITE_FRA.name, tenant_id: TENANT_CORP.id, tenant_name: TENANT_CORP.name, role_id: "role-jump-0000-0000-0000-00000000002", role_name: "Jump Host" },
  { id: "host-0000-0000-0000-0000-000000000021", name: "nvr-ams-01", status: "active", primary_ip: "192.0.2.90", site_id: SITE_AMS.id, site_name: SITE_AMS.name, tenant_id: TENANT_CORP.id, tenant_name: TENANT_CORP.name, role_name: "CCTV Recorder" },
  { id: "host-0000-0000-0000-0000-000000000022", name: "cam-fra-entrance", status: "active", primary_ip: "198.51.100.90", site_id: SITE_FRA.id, site_name: SITE_FRA.name, tenant_id: TENANT_CORP.id, tenant_name: TENANT_CORP.name, role_name: "CCTV Recorder" },
  { id: "host-0000-0000-0000-0000-000000000001", name: "web-prod-01", status: "active", primary_ip: "192.0.2.11", site_id: SITE_AMS.id, site_name: SITE_AMS.name, tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name, role_id: "role-srv-0000-0000-0000-000000000001", role_name: "Linux Server", device_type_name: "PowerEdge R650" },
  { id: "host-0000-0000-0000-0000-000000000002", name: "db-primary-01", status: "active", primary_ip: "192.0.2.21", site_id: SITE_AMS.id, site_name: SITE_AMS.name, tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name, role_id: "role-srv-0000-0000-0000-000000000001", role_name: "Linux Server", device_type_name: "PowerEdge R650" },
  { id: "host-0000-0000-0000-0000-000000000005", name: "edge-router-01", status: "active", primary_ip: "192.0.2.1", site_id: SITE_AMS.id, site_name: SITE_AMS.name, tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name, role_id: "role-rtr-0000-0000-0000-000000000004", role_name: "Router", device_type_name: "ProLiant DL360" },
  { id: "host-0000-0000-0000-0000-000000000011", name: "fra-web-01", status: "active", primary_ip: "198.51.100.11", site_id: SITE_FRA.id, site_name: SITE_FRA.name, tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name, role_id: "role-srv-0000-0000-0000-000000000001", role_name: "Linux Server", device_type_name: "PowerEdge R650" },
]);

register("inventory/virtual_machines", [
  { id: "host-0000-0000-0000-0000-000000000003", name: "app-node-02", status: "active", primary_ip: "192.0.2.32", tenant_id: TENANT_CORE.id, tenant_name: TENANT_CORE.name, role_id: "role-srv-0000-0000-0000-000000000001", role_name: "Linux Server" },
  { id: "host-0000-0000-0000-0000-000000000013", name: "vnc-lab-01", status: "active", primary_ip: "198.51.100.51", site_id: "site-lab-0000-0000-0000-000000000003", site_name: "Remote Lab", tenant_id: TENANT_CORP.id, tenant_name: TENANT_CORP.name, role_id: "role-jump-0000-0000-0000-00000000002", role_name: "Lab Workstation" },
]);

const RACK_GROUP_ID = "a8400000-0000-4000-8000-000000000001";
const CABLE_BUNDLE_ID = "a8400000-0000-4000-8000-000000000002";
register("inventory/rack-groups", [
  { id: RACK_GROUP_ID, name: "Amsterdam Network Row", slug: "amsterdam-network-row", description: "Edge network racks", comments: "Access from the north aisle", custom_fields: {} },
], "Rack Groups");
register("inventory/cable-bundles", [
  { id: CABLE_BUNDLE_ID, name: "AMS Overhead Fiber", description: "Overhead fiber route", comments: "Keep service loops above the rack", custom_fields: {} },
], "Cable Bundles");
register("inventory/cables", [
  { id: "a8400000-0000-4000-8000-000000000003", label: "AMS uplink 01", status: "connected", type: "smf", bundle_id: CABLE_BUNDLE_ID, bundle_name: "AMS Overhead Fiber", color: "00aaff", length: 10, length_unit: "m" },
]);
register("inventory/racks", [
  { id: "rk-0000-0000-0000-000000000001", name: "AMS-R01", status: "active", site_id: SITE_AMS.id, site_name: SITE_AMS.name, u_height: 42, group_id: RACK_GROUP_ID, group_name: "Amsterdam Network Row" },
  { id: "rk-0000-0000-0000-000000000002", name: "FRA-R01", status: "active", site_id: SITE_FRA.id, site_name: SITE_FRA.name, u_height: 42 },
]);

register("inventory/contacts", [
  { id: "ct-0000-0000-0000-000000000001", name: "Alice Operator", title: "SRE Lead", email: "alice@piping.demo", phone: "+31 20 555 0101" },
  { id: "ct-0000-0000-0000-000000000002", name: "Bob Netadmin", title: "Network Engineer", email: "bob@piping.demo", phone: "+49 69 555 0202" },
]);

register("inventory/contact-roles", [
  { id: "cr-0000-0000-0000-000000000001", name: "Primary", slug: "primary" },
  { id: "cr-0000-0000-0000-000000000002", name: "Escalation", slug: "escalation" },
]);

register("inventory/contact-groups", [
  { id: "cg-0000-0000-0000-000000000001", name: "On-call", slug: "on-call" },
]);

register("inventory/vlans", [
  { id: "vl-0000-0000-0000-000000000001", name: "mgmt", vid: 10, status: "active", site_id: SITE_AMS.id, site_name: SITE_AMS.name },
  { id: "vl-0000-0000-0000-000000000002", name: "storage", vid: 20, status: "active", site_id: SITE_AMS.id, site_name: SITE_AMS.name },
]);

register("inventory/prefixes", [
  { id: "px-0000-0000-0000-000000000001", prefix: "192.0.2.0/24", status: "active", site_id: SITE_AMS.id, site_name: SITE_AMS.name },
  { id: "px-0000-0000-0000-000000000002", prefix: "198.51.100.0/24", status: "active", site_id: SITE_FRA.id, site_name: SITE_FRA.name },
]);

register("inventory/clusters", [
  { id: "cl-0000-0000-0000-000000000001", name: "vmware-ams", status: "active", site_id: SITE_AMS.id, site_name: SITE_AMS.name },
]);

register("inventory/cluster_types", [
  { id: "clt-0000-0000-0000-000000000001", name: "VMware vSphere", slug: "vmware" },
]);

register("inventory/virtual-machine-types", [
  { id: "01900000-0000-7000-8000-000000000095", name: "Small Linux VM", slug: "small-linux", default_platform_id: null, default_vcpus: 0.25, default_memory: 512, description: "Defaults for new lab VMs", comments: "", custom_fields: {}, virtual_machine_count: 0 },
]);

register("inventory/cluster_groups", [
  { id: "clg-0000-0000-0000-000000000001", name: "Primary Clusters", slug: "primary" },
]);

// Templates (Inventory automation). The list endpoint returns the canonical envelope.
register("inventory/templates", [
  { id: "tpl-0000-0000-0000-000000000001", name: "Provision Linux Host", description: "Create a device + assign role + open SSH", status: "active", steps: [] },
  { id: "tpl-0000-0000-0000-000000000002", name: "Onboard Jump Host", description: "Register an RDP jump host", status: "active", steps: [] },
]);

// ── Access control ───────────────────────────────────────────────────────────
register("admin/access-control/users", [
  { id: "u-admin", username: "admin", email: "admin@piping.demo", status: "active", auth_source: "local", external_id: null },
  { id: "u-alice", username: "alice", email: "alice@piping.demo", status: "active", auth_source: "local", external_id: null },
  { id: "u-bob", username: "bob", email: "bob@piping.demo", status: "active", auth_source: "oidc", external_id: "bob@corp" },
]);

register("admin/access-control/groups", [
  { id: "grp-0000-0000-0000-000000000001", name: "Administrators", external_id: null },
  { id: "grp-0000-0000-0000-000000000002", name: "Operators", external_id: null },
  { id: "grp-0000-0000-0000-000000000003", name: "Auditors", external_id: "auditors@corp" },
]);

register("admin/access-control/roles", [
  { id: "rol-0000-0000-0000-000000000001", name: "Platform Admin", description: "Full administrative access" },
  { id: "rol-0000-0000-0000-000000000002", name: "Connection Operator", description: "Can open and manage sessions" },
  { id: "rol-0000-0000-0000-000000000003", name: "Read Only", description: "Read-only access to inventory" },
]);

register("admin/access-control/policies", [
  { id: "pol-0000-0000-0000-000000000001", name: "Allow All Inventory", effect: "allow", enabled: true, actions: ["read", "write"], resource_types: ["inventory"], resources: ["*"], subjects: ["u-admin"] },
  { id: "pol-0000-0000-0000-000000000002", name: "Operators Connect", effect: "allow", enabled: true, actions: ["connect"], resource_types: ["connection"], resources: ["*"], subjects: ["grp-0000-0000-0000-000000000002"] },
  { id: "pol-0000-0000-0000-000000000003", name: "Deny Prod Delete", effect: "deny", enabled: true, actions: ["delete"], resource_types: ["device"], resources: ["*"], subjects: [] },
]);

register("admin/access-control/role-assignments", [
  { id: "ra-0000-0000-0000-000000000001", role_id: "rol-0000-0000-0000-000000000001", user_id: "u-admin", group_id: null },
  { id: "ra-0000-0000-0000-000000000002", role_id: "rol-0000-0000-0000-000000000002", user_id: "u-alice", group_id: null },
  { id: "ra-0000-0000-0000-000000000003", role_id: "rol-0000-0000-0000-000000000003", user_id: null, group_id: "grp-0000-0000-0000-000000000003" },
]);

// ── Connection config records / assignments ──────────────────────────────────
register("connection-config/records", [
  { id: "cc-0000-0000-0000-000000000001", name: "Linux root (SSH)", description: "", protocol: "ssh", enabled: true, custom_field_data: {}, base_record_id: null, params: { host: "{{primary_ip4}}", credentials: { username: "root" }, ui: { label: "root", color: "#22c55e" } } },
  { id: "cc-0000-0000-0000-000000000002", name: "Windows RDP", description: "Domain workstations", protocol: "rdp", enabled: true, custom_field_data: {}, base_record_id: null, params: { host: "{{primary_ip4}}", credentials: { username: "Administrator" }, ui: { label: "RDP", color: "#00BCD4" } } },
]);

register("connection-config/assignments", []);

// ── Config contexts ──────────────────────────────────────────────────────────
register("config-contexts", [
  { id: "cfx-0000-0000-0000-000000000001", name: "Global Defaults", weight: 1000, is_active: true, description: "Baseline configuration" },
]);

register("config-context-bindings", []);

// ── Webhooks ─────────────────────────────────────────────────────────────────
register("webhooks", [
  { id: "wh-0000-0000-0000-000000000001", name: "Audit Sink", url: "https://hooks.piping.demo/audit", enabled: true, event_types: ["session.started", "session.ended"] },
]);

// ── User settings: personal credentials + API tokens ─────────────────────────
register("user/settings/credentials", [
  { id: "pc-0000-0000-0000-000000000001", user_id: "u-admin", protocol: "ssh", name: "My SSH key", is_default: true, params: { credentials: { username: "admin" } } },
  { id: "pc-0000-0000-0000-000000000002", user_id: "u-admin", protocol: "rdp", name: "Corp RDP", is_default: false, params: { credentials: { username: "admin" } } },
]);

register("user/settings/api-tokens", [
  { id: "tok-0000-0000-0000-000000000001", name: "CI pipeline", token_prefix: "pk_demo01", status: "active", scopes: [{ action: "read", resource_type: "inventory" }], created_at: iso(60 * 24 * 3), expires_at: iso(-60 * 24 * 30) },
]);

// ── Proxy nodes (Administration) ─────────────────────────────────────────────
register("admin/proxy-nodes", [
  { id: "pn-0000-0000-0000-000000000001", name: "ams-proxy-01", slug: "ams-proxy-01", status: "online", region: "EU West", last_seen: iso(1) },
  { id: "pn-0000-0000-0000-000000000002", name: "fra-proxy-01", slug: "fra-proxy-01", status: "online", region: "EU Central", last_seen: iso(2) },
]);

// ── Monitoring / Observability (Service Discovery + inspector metrics) ────────
register("monitoring/exporters", [
  { id: "exp-node-0000-0000-0000-000000000001", name: "Node Exporter", slug: "node", exporter_type: "node", default_job: "node", default_metrics_path: "/metrics", default_port: 9100, default_scheme: "http", default_target_source: "primary_ip", description: "Host CPU / memory / disk / network metrics", enabled: true, labels: {}, created_at: iso(60 * 24 * 20), updated_at: iso(60 * 24 * 2) },
  { id: "exp-bbox-0000-0000-0000-000000000002", name: "Blackbox Exporter", slug: "blackbox", exporter_type: "blackbox", default_job: "blackbox", default_metrics_path: "/probe", default_port: 9115, default_scheme: "http", default_target_source: "primary_ip", description: "ICMP / TCP / HTTP probes", enabled: true, labels: {}, created_at: iso(60 * 24 * 18), updated_at: iso(60 * 24) },
  { id: "exp-snmp-0000-0000-0000-000000000003", name: "SNMP Exporter", slug: "snmp", exporter_type: "snmp", default_job: "snmp", default_metrics_path: "/snmp", default_port: 9116, default_scheme: "http", default_target_source: "primary_ip", description: "Network device metrics over SNMP", enabled: false, labels: {}, created_at: iso(60 * 24 * 10), updated_at: iso(60 * 24 * 3) },
]);

register("monitoring/exporter-assignments", [
  { id: "ea-0000-0000-0000-000000000001", enabled: true, exporter_id: "exp-node-0000-0000-0000-000000000001", exporter_name: "Node Exporter", exporter_slug: "node", job: "node", labels: {}, metrics_path: "/metrics", name: "Node Exporter — Linux servers", port: 9100, scheme: "http", scope_id: "role-srv-0000-0000-0000-000000000001", scope_type: "role", target: null, weight: 100, created_at: iso(60 * 24 * 5), updated_at: iso(120) },
  { id: "ea-0000-0000-0000-000000000002", enabled: true, exporter_id: "exp-bbox-0000-0000-0000-000000000002", exporter_name: "Blackbox Exporter", exporter_slug: "blackbox", job: "blackbox-icmp", labels: { module: "icmp" }, metrics_path: "/probe", name: "Blackbox ICMP — all hosts", port: 9115, scheme: "http", scope_id: null, scope_type: "global", target: null, weight: 50, created_at: iso(60 * 24 * 4), updated_at: iso(300) },
  { id: "ea-0000-0000-0000-000000000003", enabled: false, exporter_id: "exp-snmp-0000-0000-0000-000000000003", exporter_name: "SNMP Exporter", exporter_slug: "snmp", job: "snmp", labels: {}, metrics_path: "/snmp", name: "SNMP — routers", port: 9116, scheme: "http", scope_id: "role-rtr-0000-0000-0000-000000000004", scope_type: "role", target: null, weight: 80, created_at: iso(60 * 24 * 3), updated_at: iso(60 * 24) },
]);

register("monitoring/query-sources", [
  {
    id: "qs-0000-0000-0000-000000000001", name: "Prometheus (primary)", slug: "prometheus",
    source_type: "prometheus", query_url: "http://prometheus.demo:9090/api/v1/query", timeout_ms: 5000,
    credential_ref: null, options: {}, enabled: true,
    created_at: iso(60 * 24 * 12), updated_at: iso(60 * 24),
  },
] satisfies components["schemas"]["QuerySourceItem"][]);

register("monitoring/query-source-assignments", [
  { id: "qsa-0000-0000-0000-000000000001", enabled: true, query_source_id: "qs-0000-0000-0000-000000000001", query_source_name: "Prometheus (primary)", scope_type: "global", scope_id: null, weight: 100, created_at: iso(60 * 24 * 6), updated_at: iso(60 * 24) },
]);

register("monitoring/metric-sets", [
  {
    id: "ms-0000-0000-0000-000000000001", name: "Host vitals", slug: "host-vitals",
    description: "CPU and memory", enabled: true,
    items: [
      {
        id: "msi-0000-0000-0000-000000000001", metric_set_id: "ms-0000-0000-0000-000000000001",
        metric_query_id: "mq-0000-0000-0000-000000000001", metric_query_name: "CPU busy %", metric_query_slug: "cpu-busy",
        show_in_inspector: true, show_in_list: true, show_in_hover: true, list_render: "auto", sort_order: 0,
        label_override: null, unit_override: null, created_at: iso(60 * 24 * 9), updated_at: iso(60 * 24),
      },
      {
        id: "msi-0000-0000-0000-000000000002", metric_set_id: "ms-0000-0000-0000-000000000001",
        metric_query_id: "mq-0000-0000-0000-000000000002", metric_query_name: "Memory used %", metric_query_slug: "memory-used",
        show_in_inspector: true, show_in_list: true, show_in_hover: true, list_render: "auto", sort_order: 1,
        label_override: null, unit_override: null, created_at: iso(60 * 24 * 9), updated_at: iso(60 * 24),
      },
    ],
    created_at: iso(60 * 24 * 9), updated_at: iso(60 * 24),
  },
] satisfies components["schemas"]["MetricSetDetail"][]);

register("monitoring/metric-queries", [
  {
    id: "mq-0000-0000-0000-000000000001", name: "CPU busy %", slug: "cpu-busy", label: null,
    query: "100 - (avg by (instance)(rate(node_cpu_seconds_total{mode=\"idle\"}[5m])) * 100)",
    unit: "%", source_type: "prometheus", query_type: "promql", value_type: "auto", widget_type: "stat",
    default_show_in_inspector: true, default_show_in_list: true, sort_order: 0, options: {}, enabled: true,
    created_at: iso(60 * 24 * 9), updated_at: iso(60 * 24),
  },
  {
    id: "mq-0000-0000-0000-000000000002", name: "Memory used %", slug: "memory-used", label: null,
    query: "(1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes) * 100",
    unit: "%", source_type: "prometheus", query_type: "promql", value_type: "auto", widget_type: "stat",
    default_show_in_inspector: true, default_show_in_list: true, sort_order: 1, options: {}, enabled: true,
    created_at: iso(60 * 24 * 9), updated_at: iso(60 * 24),
  },
] satisfies components["schemas"]["MetricQueryItem"][]);

register("monitoring/metric-set-assignments", [
  { id: "msa-0000-0000-0000-000000000001", enabled: true, metric_set_id: "ms-0000-0000-0000-000000000001", metric_set_name: "Host vitals", scope_type: "role", scope_id: "role-srv-0000-0000-0000-000000000001", weight: 100, created_at: iso(60 * 24 * 6), updated_at: iso(60 * 24) },
]);

// ── Playbook execution environments ──────────────────────────────────────────
// The Automation → Setup screens were the one demo area that showed an empty
// table on every tab; the runner image every fresh install ships with is the
// obvious seed, and it is what `docker-compose.yml` builds.
register("playbooks/execution-environments", [
  { id: "ee-0000-0000-0000-000000000001", name: "Default Ansible", description: "Bundled ansible-runner image used unless a job template overrides it.", image: "piping-ansible-runner:local", pull_policy: "if_not_present", runner_group: "default", enabled: true, env: {}, settings: {}, metadata: {}, created_at: iso(60 * 24 * 30), updated_at: iso(60 * 24 * 2) },
]);

// ── Audit events ─────────────────────────────────────────────────────────────
register("audit/events", [
  { id: "ae-0000-0000-0000-000000000001", created_at: iso(3), actor_id: "admin", event_type: "session.started", target_type: "connection_target", target_id: "web-prod-01", metadata: { protocol: "ssh" } },
  { id: "ae-0000-0000-0000-000000000002", created_at: iso(12), actor_id: "alice", event_type: "user.login", target_type: "user", target_id: "alice", metadata: {} },
  { id: "ae-0000-0000-0000-000000000003", created_at: iso(40), actor_id: "admin", event_type: "policy.updated", target_type: "policy", target_id: "Allow All Inventory", metadata: {} },
]);
