import { apiClient, unwrap } from "@/shared/api/client";
import { collectAllPages } from "@/shared/api/pagination";
import type { components, paths } from "@/shared/api/schema.gen";
import { inventoryEntityHasStatus } from "@/shared/api/types";
import { normalizeHex } from "@/shared/forms/colorUtils";
import type { CustomFieldEntityKind } from "@/features/inventory/custom-fields-api";
import {
  entitySupportsCustomIcon,
  extractPersistedIcon,
  isEntityIconName,
  isSafeIconDataUrl,
  mergePersistedIcon,
} from "@/shared/entity-icon-utils";
import type {
  InventoryEntityKey,
  InventoryTemplate,
  ListQuery,
  ObjectRefProjection,
  Paginated,
  Resource,
  ResourceStatus,
  TemplateStep,
  TemplateRunResponse,
  TemplateRunStep,
  TemplateVariable,
} from "@/shared/api/types";

const ENTITY_RESOURCE_TYPE: Record<InventoryEntityKey, string> = {
  Owners: "owner",
  "Owner Groups": "owner_group",
  Sites: "site",
  Devices: "device",
  "Virtual Machines": "vm",
  "Virtual Machine Types": "virtual_machine_type",
  Tenants: "tenant",
  Regions: "region",
  "Device Roles": "role",
  "Tenant Groups": "tenant_group",
  Contacts: "contact",
  "Contact Roles": "contact_role",
  "Contact Groups": "contact_group",
  "Contact Assignments": "contact_assignment",
  Manufacturers: "manufacturer",
  "Site Groups": "site_group",
  Platforms: "platform",
  "Device Types": "device_type",
  Racks: "rack",
  Interfaces: "interface",
  Cables: "cable",
  "Cluster Types": "cluster_type",
  "Cluster Groups": "cluster_group",
  Clusters: "cluster",
  VLANs: "vlan",
  Prefixes: "prefix",
  "IP Addresses": "ip_address",
  "VM Interfaces": "vm_interface",
  "Rack Roles": "rack_role",
  "Rack Types": "rack_type",
  Locations: "location",
  RIRs: "rir",
  VRFs: "vrf",
  "Route Targets": "route_target",
  Aggregates: "aggregate",
  ASNs: "asn",
  "IP Ranges": "ip_range",
  Services: "service",
  Modules: "module",
  "Module Types": "module_type",
  "Module Type Profiles": "module_type_profile",
  Tags: "tag",
  "Virtual Chassis": "virtual_chassis",
  "Virtual Disks": "virtual_disk",
  // Providers / Circuit Types / Provider Networks / Circuits / ASN Ranges /
  // Inventory Item Roles / Inventory Items.
  Providers: "provider",
  "Circuit Types": "circuit_type",
  "Provider Networks": "provider_network",
  Circuits: "circuit",
  "ASN Ranges": "asn_range",
  "Inventory Item Roles": "inventory_item_role",
  "Inventory Items": "inventory_item",
  // VPN.
  "Tunnel Groups": "tunnel_group",
  Tunnels: "tunnel",
  "Tunnel Terminations": "tunnel_termination",
  "IKE Policies": "ike_policy",
  "IKE Proposals": "ike_proposal",
  "IPsec Policies": "ipsec_policy",
  "IPsec Proposals": "ipsec_proposal",
  "IPsec Profiles": "ipsec_profile",
  // Circuits remainder + DCIM gaps + IPAM gaps.
  "Circuit Groups": "circuit_group",
  "Provider Accounts": "provider_account",
  "Circuit Terminations": "circuit_termination",
  "Virtual Circuit Types": "virtual_circuit_type",
  "Virtual Circuits": "virtual_circuit",
  "Power Panels": "power_panel",
  "Power Feeds": "power_feed",
  "Rack Reservations": "rack_reservation",
  "MAC Addresses": "mac_address",
  "Virtual Device Contexts": "virtual_device_context",
  "Service Templates": "service_template",
};

/** Materialized polymorphic generic-FK target as it arrives on read. Aliased to
 *  the generated `ObjectRef` schema component (single source of truth); a `null`
 *  `label` marks a dangling reference (target row missing). */
type BackendObjectRef = components["schemas"]["ObjectRef"];

/** Raw row returned by the hydrated list and detail endpoints. The hydrated
 *  list carries *_name fields alongside *_id FKs; the detail endpoint
 *  (GET /:id) currently returns only the FK UUIDs. Extra keys are ignored. */
// Schema contract gap: No single generated schema component covers this polymorphic
// UI projection across all inventory entity list/detail responses.
interface BackendInventoryRecord {
  id: string;
  users?: string[];
  user_groups?: string[];
  name?: string;
  slug?: string;
  status?: string;
  description?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  custom_fields?: Record<string, unknown> | null;
  custom_field_data?: Record<string, unknown> | null;
  role_custom_fields?: Record<string, unknown> | null;
  // Device/VM local config context override (arbitrary JSON); absent on others.
  local_context_data?: unknown;
  // FK UUIDs (canonical backend shape):
  site_id?: string | null;
  tenant_id?: string | null;
  region_id?: string | null;
  // `role_id` is a UUID for device roles but a numeric external id for
  // VLANs/Prefixes (NetBox IPAM role). They never coexist on one row.
  role_id?: string | number | null;
  platform_id?: string | null;
  virtual_machine_type_id?: string | null;
  virtual_machine_type_name?: string | null;
  default_platform_id?: string | null;
  default_vcpus?: number | null;
  default_memory?: number | null;
  virtual_machine_count?: number;
  device_type_id?: string | null;
  cluster_id?: string | null;
  site_group_id?: string | null;
  device_id?: string | null;
  vm_id?: string | null;
  parent_id?: string | null;
  group_id?: string | null;
  contact_id?: string | null;
  manufacturer_id?: string | null;
  cluster_type_id?: string | null;
  cluster_group_id?: string | null;
  vlan_id?: string | null;
  virtual_machine_id?: string | null;
  interface_id?: string | null;
  // Resolved names returned by the hydrated list endpoint:
  site_name?: string | null;
  tenant_name?: string | null;
  region_name?: string | null;
  role_name?: string | null;
  platform_name?: string | null;
  device_type_name?: string | null;
  cluster_name?: string | null;
  site_group_name?: string | null;
  device_name?: string | null;
  vm_name?: string | null;
  parent_name?: string | null;
  group_name?: string | null;
  contact_name?: string | null;
  cluster_type_name?: string | null;
  cluster_group_name?: string | null;
  vlan_name?: string | null;
  virtual_machine_name?: string | null;
  interface_name?: string | null;
  // Device/VM fields:
  primary_ip?: string | null;
  position?: number | null;
  face?: string | null;
  airflow?: string | null;
  // Contact scalar fields:
  title?: string | null;
  phone?: string | null;
  email?: string | null;
  link?: string | null;
  // Contact-assignment polymorphic target. The backend materializes the raw
  // `(content_type, object_id)` pair into a nested `object` ObjectRef on read.
  object?: BackendObjectRef | null;
  priority?: string | null;
  // DCIM scalar fields. Numbers/booleans arrive in wire types and are
  // stringified into the UI `Resource` projection; toBackendPayload coerces back.
  // (`slug` is declared above and shared with the named entities.)
  model?: string | null;
  label?: string | null;
  napalm_driver?: string | null;
  facility_id?: string | null;
  serial?: string | null;
  asset_tag?: string | null;
  // Inventory-item scalar/FK fields (PIP-86). `discovered` arrives as a wire
  // boolean and is stringified into the UI `Resource`; `component` is
  // materialized into a nested ObjectRef on read.
  part_id?: string | null;
  discovered?: boolean | null;
  component?: BackendObjectRef | null;
  // Module-type-profile attribute schema (arbitrary JSON; read-only).
  schema?: unknown;
  width?: number | null;
  u_height?: number | null;
  type?: string | null;
  enabled?: boolean | null;
  color?: string | null;
  length?: number | null;
  manufacturer_name?: string | null;
  // Rack Types / Locations scalar fields.
  form_factor?: string | null;
  weight?: number | null;
  facility?: string | null;
  // Note: `address` doubles as a contact postal address (Contacts) and the
  // CIDR/host address (IP Addresses); `toResource` resolves the meaning per
  // entity.
  address?: string | null;
  // IPAM + Virtualization scalar fields. Numbers/booleans arrive in
  // wire types and are stringified into the UI `Resource`; toBackendPayload
  // coerces back. `vrf_id` is numeric for Prefixes and a string for IP Addresses.
  vid?: number | null;
  prefix?: string | null;
  dns_name?: string | null;
  is_pool?: boolean | null;
  mark_utilized?: boolean | null;
  utilized_addresses?: number | null;
  usable_addresses?: number | null;
  utilization_percent?: number | null;
  vrf_id?: string | number | null;
  role?: string | null;
  mac_address?: string | null;
  mtu?: number | null;
  mode?: string | null;
  vcpus?: number | null;
  memory?: number | null;
  disk?: number | null;
  // Scalar/FK fields for RIRs / VRFs / Route Targets / Aggregates / ASNs /
  // IP Ranges / Services / Modules / Module Types / Virtual Chassis /
  // Virtual Disks. Numbers/booleans arrive in wire types and are stringified
  // into the UI `Resource`; toBackendPayload coerces back.
  is_private?: boolean | null;
  rd?: string | null;
  enforce_unique?: boolean | null;
  asn?: number | null;
  date_added?: string | null;
  start_address?: string | null;
  end_address?: string | null;
  protocol?: string | null;
  ports?: number[] | null;
  port_mappings?: string[];
  ipaddresses?: components["schemas"]["ServiceResponse"]["ipaddresses"];
  size?: number | null;
  domain?: string | null;
  part_number?: string | null;
  comments?: string | null;
  // Service polymorphic parent — materialized into a nested `parent_object`
  // ObjectRef on read (was the raw `parent_object_type_id` + `parent_object_id`).
  parent_object?: BackendObjectRef | null;
  // FK UUIDs + resolved names.
  rir_id?: string | null;
  rir_name?: string | null;
  vrf_name?: string | null;
  module_type_id?: string | null;
  module_type_name?: string | null;
  module_bay_id?: string | null;
  module_bay_name?: string | null;
  master_id?: string | null;
  master_name?: string | null;
  // VPN scalar + FK fields.
  encapsulation?: string | null;
  preshared_key?: string | null;
  pfs_group?: number | null;
  sa_lifetime?: number | null;
  sa_lifetime_seconds?: number | null;
  sa_lifetime_data?: number | null;
  authentication_method?: string | null;
  encryption_algorithm?: string | null;
  authentication_algorithm?: string | null;
  version?: number | null;
  // `mode` already declared above (shared with IPAM interfaces).
  // `role` already declared above (shared with IPAM).
  // tunnel_id is the numeric VPN tunnel ID on vpn_tunnel, but the (uuid) tunnel FK
  // on vpn_tunneltermination — same wire key, disjoint rows (cf. role_id/vrf_id).
  tunnel_id?: number | string | null;
  group_id_vpn?: string | null;
  ipsec_profile_id?: string | null;
  ike_policy_id?: string | null;
  ipsec_policy_id?: string | null;
  // Tunnel-termination polymorphic target — materialized into a nested
  // `termination` ObjectRef on read (was `termination_type_id` + `termination_id`).
  termination?: BackendObjectRef | null;
  outside_ip_id?: string | null;
  outside_ip?: string | null;
  // Resolved names for VPN FK relations.
  tunnel_group_name?: string | null;
  ipsec_profile_name?: string | null;
  ike_policy_name?: string | null;
  ipsec_policy_name?: string | null;
  tunnel_name?: string | null;
  tunnel_fk_name?: string | null;
  // Scalar + FK fields (Circuits / Providers / Provider Networks /
  // Circuit Types / ASN Ranges / Inventory Item Roles).
  cid?: string | null;
  service_id?: string | null;
  provider_id?: string | null;
  provider_name?: string | null;
  provider_account_name?: string | null;
  type_id?: string | null;
  type_name?: string | null;
  provider_account_id?: string | null;
  termination_a_id?: string | null;
  termination_z_id?: string | null;
  install_date?: string | null;
  termination_date?: string | null;
  commit_rate?: number | null;
  distance?: number | null;
  distance_unit?: string | null;
  start?: number | null;
  end?: number | null;
  // Circuits remainder scalar + FK fields.
  term_side?: string | null;
  xconnect_id?: string | null;
  pp_info?: string | null;
  port_speed?: number | null;
  upstream_speed?: number | null;
  mark_connected?: boolean | null;
  account?: string | null;
  circuit_id?: string | null;
  circuit_name?: string | null;
  provider_network_id?: string | null;
  provider_network_name?: string | null;
  virtual_circuit_type_id?: string | null;
  virtual_circuit_type_name?: string | null;
  // DCIM gaps scalar + FK fields.
  supply?: string | null;
  phase?: string | null;
  voltage?: number | null;
  amperage?: number | null;
  max_utilization?: number | null;
  available_power?: number | null;
  units?: number[] | null;
  user_id?: string | null;
  user_name?: string | null;
  owner_id?: string | null;
  owner_name?: string | null;
  // MAC-address polymorphic target — materialized into a nested `assigned_object`
  // ObjectRef on read (was `assigned_object_type_id` + `assigned_object_id`).
  assigned_object?: BackendObjectRef | null;
  power_panel_id?: string | null;
  power_panel_name?: string | null;
  rack_id?: string | null;
  rack_name?: string | null;
  identifier?: number | null;
  primary_ip4_id?: string | null;
  primary_ip6_id?: string | null;
  primary_ip4?: string | null;
  primary_ip6?: string | null;
  // Form field expansion: additional writable scalar/FK fields per backend payload.
  time_zone?: string | null;
  physical_address?: string | null;
  shipping_address?: string | null;
  latitude?: number | null;
  longitude?: number | null;
  vm_role?: boolean | null;
  desc_units?: boolean | null;
  outer_width?: number | null;
  outer_depth?: number | null;
  outer_height?: number | null;
  outer_unit?: string | null;
  starting_unit?: number | null;
  mounting_depth?: number | null;
  max_weight?: number | null;
  weight_unit?: string | null;
  mgmt_only?: boolean | null;
  length_unit?: string | null;
  mark_populated?: boolean | null;
  rack_role_id?: string | null;
  rack_role_name?: string | null;
}

/** Canonical list envelope returned by every inventory list endpoint. */
// Schema contract gap: Inventory list envelopes are generated per entity and no single component matches
// this polymorphic record envelope.
export interface BackendInventoryEnvelope<T = BackendInventoryRecord> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
}

interface InventoryListParams {
  limit: number;
  offset: number;
  search?: string;
}

interface InventoryEntityApi {
  list: (query: InventoryListParams) => Promise<BackendInventoryEnvelope>;
  get: (id: string) => Promise<BackendInventoryRecord>;
  // Optional: a read-only backend entity (no create/update/delete routes)
  // exposes only list/get, and its write affordances are suppressed.
  create?: (body: Record<string, unknown>) => Promise<BackendInventoryRecord>;
  update?: (id: string, body: Record<string, unknown>) => Promise<BackendInventoryRecord>;
  delete?: (id: string) => Promise<void>;
}

function asBackendInventoryEnvelope<T = BackendInventoryRecord>(
  raw: unknown,
): BackendInventoryEnvelope<T> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Inventory list endpoint returned a non-envelope payload");
  }
  const envelope = raw as Partial<BackendInventoryEnvelope<T>>;
  if (
    !Array.isArray(envelope.items) ||
    typeof envelope.total !== "number" ||
    typeof envelope.limit !== "number" ||
    typeof envelope.offset !== "number"
  ) {
    throw new Error("Inventory list endpoint returned an invalid envelope");
  }
  return envelope as BackendInventoryEnvelope<T>;
}

function asBackendInventoryRecord(raw: unknown): BackendInventoryRecord {
  return raw as BackendInventoryRecord;
}

/** Runtime path helper for inventory list endpoints.
 *  Generated path types are per-entity, while the inventory resource page keeps
 *  route selection data-driven; this cast is only for the dynamic path. */
async function listInventoryEnvelope(
  path: string,
  query: InventoryListParams,
): Promise<BackendInventoryEnvelope> {
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: InventoryListParams } },
    ) => Promise<{ data?: unknown; error?: unknown }>)(path, { params: { query } }),
  );
  return asBackendInventoryEnvelope(raw);
}

/*
 * The generated schema gives some inventory detail/create/update responses
 * concrete DTOs and others `unknown`. These typed path calls are canonical;
 * the small projection helpers above normalize each entity response into the
 * UI-facing inventory record shape.
 */
const ENTITY_API: Record<InventoryEntityKey, InventoryEntityApi> = {
  Owners: {
    list: async (query) => asBackendInventoryEnvelope(unwrap(await apiClient.GET("/api/v1/inventory/owners", { params: { query } }))),
    get: async (id) => asBackendInventoryRecord(unwrap(await apiClient.GET("/api/v1/inventory/owners/{id}", { params: { path: { id } } }))),
    create: async (body) => asBackendInventoryRecord(unwrap(await apiClient.POST("/api/v1/inventory/owners", { body: body as components["schemas"]["OwnerCreatePayload"] }))),
    update: async (id, body) => asBackendInventoryRecord(unwrap(await apiClient.PATCH("/api/v1/inventory/owners/{id}", { params: { path: { id } }, body }))),
    delete: async (id) => { await apiClient.DELETE("/api/v1/inventory/owners/{id}", { params: { path: { id } } }); },
  },
  "Owner Groups": {
    list: async (query) => asBackendInventoryEnvelope(unwrap(await apiClient.GET("/api/v1/inventory/owner_groups", { params: { query } }))),
    get: async (id) => asBackendInventoryRecord(unwrap(await apiClient.GET("/api/v1/inventory/owner_groups/{id}", { params: { path: { id } } }))),
    create: async (body) => asBackendInventoryRecord(unwrap(await apiClient.POST("/api/v1/inventory/owner_groups", { body: body as components["schemas"]["OwnerGroupCreatePayload"] }))),
    update: async (id, body) => asBackendInventoryRecord(unwrap(await apiClient.PATCH("/api/v1/inventory/owner_groups/{id}", { params: { path: { id } }, body }))),
    delete: async (id) => { await apiClient.DELETE("/api/v1/inventory/owner_groups/{id}", { params: { path: { id } } }); },
  },
  Sites: {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/sites", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/sites/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/sites", {
            body: body as components["schemas"]["SitePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/sites/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["SitePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/sites/{id}", {
        params: { path: { id } },
      });
    },
  },
  Devices: {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/devices", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/devices/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/devices", {
            body: body as components["schemas"]["DevicePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/devices/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["DevicePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/devices/{id}", {
        params: { path: { id } },
      });
    },
  },
  "Virtual Machines": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/virtual_machines", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/virtual_machines/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/virtual_machines", {
            body: body as components["schemas"]["VirtualMachinePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/virtual_machines/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["VirtualMachinePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/virtual_machines/{id}", {
        params: { path: { id } },
      });
    },
  },
  Tenants: {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/tenants", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/tenants/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/tenants", {
            body: body as components["schemas"]["TenantPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/tenants/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["TenantPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/tenants/{id}", {
        params: { path: { id } },
      });
    },
  },
  Regions: {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/regions", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/regions/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/regions", {
            body: body as components["schemas"]["RegionPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/regions/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["RegionPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/regions/{id}", {
        params: { path: { id } },
      });
    },
  },
  "Device Roles": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/device_roles", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/device_roles/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/device_roles", {
            body: body as components["schemas"]["DeviceRolePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/device_roles/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["DeviceRolePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/device_roles/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Tenancy: tenant_groups exposes the canonical `{items,total,limit,offset}`
  // envelope and accepts limit/offset, matching the FK-carrying entities above.
  "Tenant Groups": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/tenant_groups", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/tenant_groups/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/tenant_groups", {
            body: body as components["schemas"]["TenantGroupPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/tenant_groups/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["TenantGroupPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/tenant_groups/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Contacts: normalize through the same paginated list helper as every other
  // inventory entity so count badges and pickers never trigger full-list fan-out.
  Contacts: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/contacts", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/contacts/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/contacts", {
            body: body as components["schemas"]["ContactCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/contacts/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/contacts/{id}", {
        params: { path: { id } },
      });
    },
  },
  "Contact Roles": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/contact-roles", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/contact-roles/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/contact-roles", {
            body: body as components["schemas"]["ContactRoleCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/contact-roles/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/contact-roles/{id}", {
        params: { path: { id } },
      });
    },
  },
  "Contact Groups": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/contact-groups", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/contact-groups/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/contact-groups", {
            body: body as components["schemas"]["ContactGroupCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/contact-groups/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/contact-groups/{id}", {
        params: { path: { id } },
      });
    },
  },
  "Contact Assignments": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/contact-assignments", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/contact-assignments/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/contact-assignments", {
            body: body as components["schemas"]["ContactAssignmentCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/contact-assignments/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/contact-assignments/{id}", {
        params: { path: { id } },
      });
    },
  },
  // DCIM. Manufacturers expose untyped (`unknown`) bodies/responses in
  // the generated schema, but the runtime list endpoint is still paginated.
  Manufacturers: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/manufacturers", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/manufacturers/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/manufacturers", {
            body: body as components["schemas"]["ManufacturerPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/manufacturers/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ManufacturerPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/manufacturers/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Site groups expose the canonical `{items,total,limit,offset}` envelope and
  // accept limit/offset, like the FK-carrying entities above.
  "Site Groups": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/site_groups", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/site_groups/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/site_groups", {
            body: body as components["schemas"]["SiteGroupPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/site_groups/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["SiteGroupPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/site_groups/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Platforms are normalized through the paginated list helper.
  Platforms: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/platforms", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/platforms/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/platforms", {
            body: body as components["schemas"]["PlatformCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/platforms/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/platforms/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Device types expose the canonical envelope and accept limit/offset.
  "Device Types": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/device_types", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/device_types/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/device_types", {
            body: body as components["schemas"]["DeviceTypePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/device_types/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["DeviceTypePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/device_types/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Racks are normalized through the paginated list helper.
  Racks: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/racks", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/racks/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/racks", {
            body: body as components["schemas"]["RackCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/racks/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/racks/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Interfaces are normalized through the paginated list helper.
  Interfaces: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/interfaces", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/interfaces/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/interfaces", {
            body: body as components["schemas"]["InterfaceCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/interfaces/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/interfaces/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Cables are normalized through the paginated list helper. They are a nameless
  // termination record (label only), handled like contact assignments in
  // toResource/toBackendPayload.
  Cables: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/cables", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/cables/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/cables", {
            body: body,
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/cables/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/cables/{id}", {
        params: { path: { id } },
      });
    },
  },
  // ── Virtualization ──
  // Cluster types expose the canonical `{items,total,limit,offset}` envelope and
  // accept limit/offset query params. ClusterTypePayload is name + slug only.
  "Cluster Types": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/cluster_types", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/cluster_types/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/cluster_types", {
            body: body as components["schemas"]["ClusterTypePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/cluster_types/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ClusterTypePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/cluster_types/{id}", {
        params: { path: { id } },
      });
    },
  },
  "Virtual Machine Types": {
    list: async (query) => asBackendInventoryEnvelope(unwrap(await apiClient.GET("/api/v1/inventory/virtual-machine-types", { params: { query } }))),
    get: async (id) => asBackendInventoryRecord(unwrap(await apiClient.GET("/api/v1/inventory/virtual-machine-types/{id}", { params: { path: { id } } }))),
    create: async (body) => asBackendInventoryRecord(unwrap(await apiClient.POST("/api/v1/inventory/virtual-machine-types", { body: body as components["schemas"]["VirtualMachineTypePayload"] }))),
    update: async (id, body) => asBackendInventoryRecord(unwrap(await apiClient.PATCH("/api/v1/inventory/virtual-machine-types/{id}", { params: { path: { id } }, body }))),
    delete: async (id) => { await apiClient.DELETE("/api/v1/inventory/virtual-machine-types/{id}", { params: { path: { id } } }); },
  },
  // Cluster groups expose the canonical envelope and accept limit/offset.
  // ClusterGroupPayload is name + slug only.
  "Cluster Groups": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/cluster_groups", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/cluster_groups/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/cluster_groups", {
            body: body as components["schemas"]["ClusterGroupPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/cluster_groups/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ClusterGroupPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/cluster_groups/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Clusters expose the canonical envelope and accept limit/offset. ClusterPayload
  // is name + cluster_type_id + cluster_group_id + site_id + tenant_id + status
  // + description + comments.
  Clusters: {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/clusters", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/clusters/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/clusters", {
            body: body as components["schemas"]["ClusterPayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/clusters/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ClusterPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/clusters/{id}", {
        params: { path: { id } },
      });
    },
  },
  // ── IPAM ──
  // VLANs are normalized through the paginated list helper. VlanCreatePayload carries vid + name + status + FKs.
  VLANs: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/vlans", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/vlans/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/vlans", {
            body: body as components["schemas"]["VlanCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/vlans/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/vlans/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Prefixes are normalized through the paginated list helper.
  Prefixes: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/prefixes", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/prefixes/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/prefixes", {
            body: body as components["schemas"]["PrefixCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/prefixes/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/prefixes/{id}", {
        params: { path: { id } },
      });
    },
  },
  // IP addresses use the kebab route and are normalized through the paginated list helper. IpAddressCreatePayload requires address + interface_id.
  "IP Addresses": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ip-addresses", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/ip-addresses/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/ip-addresses", {
            body: body as components["schemas"]["IpAddressCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ip-addresses/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ip-addresses/{id}", {
        params: { path: { id } },
      });
    },
  },
  // VM interfaces use the kebab route and are normalized through the paginated list helper.
  // VmInterfaceCreatePayload requires virtual_machine_id + name.
  "VM Interfaces": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/vm-interfaces", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/vm-interfaces/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/vm-interfaces", {
            body: body as components["schemas"]["VmInterfaceCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/vm-interfaces/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/vm-interfaces/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Rack Roles: canonical envelope, accept limit/offset.
  "Rack Roles": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/rack_roles", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/rack_roles/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/rack_roles", {
            body: body as components["schemas"]["RackRoleCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/rack_roles/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/rack_roles/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Rack Types: canonical envelope, accept limit/offset.
  "Rack Types": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/rack_types", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/rack_types/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/rack_types", {
            body: body as components["schemas"]["RackTypeCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/rack_types/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/rack_types/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Locations: canonical envelope, accept limit/offset.
  Locations: {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/locations", {
            params: { query },
          }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/locations/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/locations", {
            body: body as components["schemas"]["LocationCreatePayload"],
          }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/locations/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/locations/{id}", {
        params: { path: { id } },
      });
    },
  },
  // IPAM/DCIM/Virtualization entities below all go through listInventoryEnvelope
  // so the UI sends limit/offset consistently even where generated response
  // types are still weak or temporarily array-shaped.
  // RIRs: name + slug + is_private + description.
  RIRs: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/rirs", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/rirs/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/rirs", { body: body as components["schemas"]["RirCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/rirs/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/rirs/{id}", { params: { path: { id } } });
    },
  },
  // VRFs: name + rd + tenant_id + enforce_unique + description + comments.
  VRFs: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/vrfs", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/vrfs/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/vrfs", { body: body as components["schemas"]["VrfCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/vrfs/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/vrfs/{id}", { params: { path: { id } } });
    },
  },
  // Route Targets: name + tenant_id + description.
  "Route Targets": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/route_targets", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/route_targets/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/route_targets", { body: body as components["schemas"]["RouteTargetCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/route_targets/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/route_targets/{id}", { params: { path: { id } } });
    },
  },
  // Aggregates: prefix (required) + rir_id (required) + tenant_id + date_added +
  // description + comments. Nameless on the wire — prefix stands in for the name.
  Aggregates: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/aggregates", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/aggregates/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/aggregates", { body: body as components["schemas"]["AggregateCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/aggregates/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/aggregates/{id}", { params: { path: { id } } });
    },
  },
  // ASNs: asn (required int) + rir_id (required) + tenant_id + description +
  // comments. Nameless on the wire — the AS number stands in for the name.
  ASNs: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/asns", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/asns/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/asns", { body: body as components["schemas"]["AsnCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/asns/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/asns/{id}", { params: { path: { id } } });
    },
  },
  // IP Ranges: start_address + end_address (both required) + status + tenant_id +
  // vrf_id + description. Nameless — the start→end range stands in for the name.
  "IP Ranges": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ip_ranges", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/ip_ranges/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/ip_ranges", { body: body as components["schemas"]["IpRangeCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ip_ranges/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ip_ranges/{id}", { params: { path: { id } } });
    },
  },
  // Services: name + protocol + ports + parent_object_type_id + parent_object_id +
  // description. ports is a comma-separated form string coerced to number[].
  Services: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/services", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/services/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/services", { body: body as components["schemas"]["ServiceCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/services/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/services/{id}", { params: { path: { id } } });
    },
  },
  // Modules: device FK (required) + module_type FK (required) + module_bay_id
  // (required) + status + serial + asset_tag + description. Nameless — the module
  // type name stands in for the table/search name.
  Modules: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/modules", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/modules/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/modules", { body: body as components["schemas"]["ModuleCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/modules/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/modules/{id}", { params: { path: { id } } });
    },
  },
  // Module Types: manufacturer FK (required) + model (required). Exposes the
  // canonical envelope and accepts limit/offset. `model` is the display field.
  "Module Types": {
    list: async (query) =>
      asBackendInventoryEnvelope(
        unwrap(
          await apiClient.GET("/api/v1/inventory/module-types", { params: { query } }),
        ),
      ),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/module-types/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/module-types", { body: body as components["schemas"]["ModuleTypePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/module-types/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ModuleTypePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/module-types/{id}", { params: { path: { id } } });
    },
  },
  // Module Type Profiles: read-only native list/detail. No create/update/delete
  // routes exist, so the write methods are intentionally absent and the UI
  // suppresses every write affordance for this entity.
  "Module Type Profiles": {
    list: async (query) =>
      listInventoryEnvelope("/api/v1/inventory/module-type-profiles", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/module-type-profiles/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
  },
  Tags: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/tags", query),
    get: async (id) => asBackendInventoryRecord(unwrap(
      await apiClient.GET("/api/v1/inventory/tags/{id}", { params: { path: { id } } }),
    )),
  },
  // Virtual Chassis: name (required) + domain + master FK + description + comments.
  "Virtual Chassis": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/virtual-chassis", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/virtual-chassis/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/virtual-chassis", { body: body as components["schemas"]["VirtualChassisCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/virtual-chassis/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/virtual-chassis/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Virtual Disks: virtual_machine FK (required) + name (required) + size
  // (required int) + description.
  "Virtual Disks": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/virtual-disks", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/virtual-disks/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/virtual-disks", { body: body as components["schemas"]["VirtualDiskCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/virtual-disks/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/virtual-disks/{id}", { params: { path: { id } } });
    },
  },
  // ── Circuits / Providers / Provider Networks / … ──
  // Providers: name + slug + description + comments.
  Providers: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/providers", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/providers/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/providers", { body: body as components["schemas"]["ProviderPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/providers/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ProviderPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/providers/{id}", { params: { path: { id } } });
    },
  },
  // Circuit Types: name + slug + color + description. Paginated envelope.
  "Circuit Types": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/circuit-types", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/circuit-types/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/circuit-types", { body: body as components["schemas"]["CircuitTypeCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/circuit-types/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/circuit-types/{id}", { params: { path: { id } } });
    },
  },
  // Provider Networks: provider_id (required FK) + name + service_id + description +
  // comments. Paginated envelope.
  "Provider Networks": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/provider-networks", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/provider-networks/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/provider-networks", { body: body as components["schemas"]["ProviderNetworkCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/provider-networks/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/provider-networks/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Circuits: cid (required, stands in for name) + provider_id (required FK) +
  // type_id (required FK) + status + tenant_id + description + comments + more.
  // Paginated envelope.
  Circuits: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/circuits", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/circuits/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/circuits", { body: body as components["schemas"]["CircuitCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/circuits/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/circuits/{id}", { params: { path: { id } } });
    },
  },
  // ASN Ranges: name + slug + rir_id (required FK) + start (required int) +
  // end (required int) + tenant_id + description + comments. Paginated envelope.
  "ASN Ranges": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/asn-ranges", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/asn-ranges/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/asn-ranges", { body: body as components["schemas"]["AsnRangeCreatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/asn-ranges/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/asn-ranges/{id}", { params: { path: { id } } });
    },
  },
  // Inventory Item Roles: name + slug + color + description. Paginated envelope.
  "Inventory Item Roles": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/inventory-item-roles", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/inventory-item-roles/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/inventory-item-roles", { body: body as components["schemas"]["InventoryItemRoleCreatePayload"] }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/inventory-item-roles/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/inventory-item-roles/{id}", {
        params: { path: { id } },
      });
    },
  },
  // Inventory Items: device + optional parent/role/manufacturer + name + label +
  // part_id + serial + asset_tag + discovered + description + status + component.
  "Inventory Items": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/inventory-items", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/inventory-items/{id}", {
            params: { path: { id } },
          }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.POST("/api/v1/inventory/inventory-items", { body: body as components["schemas"]["InventoryItemCreatePayload"] }),
        ),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/inventory-items/{id}", {
            params: { path: { id } },
            body: body,
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/inventory-items/{id}", {
        params: { path: { id } },
      });
    },
  },
  // ── VPN ──
  // Tunnel Groups: name + slug + description + comments. Paginated envelope.
  "Tunnel Groups": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/tunnel-groups", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/tunnel-groups/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/tunnel-groups", { body: body as components["schemas"]["TunnelGroupPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/tunnel-groups/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["TunnelGroupPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/tunnel-groups/{id}", { params: { path: { id } } });
    },
  },
  // Tunnels: name + status + encapsulation + group_id + ipsec_profile_id + tunnel_id +
  // tenant_id + description + comments. Paginated envelope.
  Tunnels: {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/tunnels", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/tunnels/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/tunnels", { body: body as components["schemas"]["TunnelPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/tunnels/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["TunnelPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/tunnels/{id}", { params: { path: { id } } });
    },
  },
  // Tunnel Terminations: role + tunnel_id (FK, required) + termination_type_id +
  // termination_id + outside_ip_id. Paginated envelope.
  "Tunnel Terminations": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/tunnel-terminations", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/tunnel-terminations/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/tunnel-terminations", { body: body as components["schemas"]["TunnelTerminationPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/tunnel-terminations/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["TunnelTerminationPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/tunnel-terminations/{id}", { params: { path: { id } } });
    },
  },
  // IKE Policies: name + version + mode + preshared_key + description + comments.
  // Paginated envelope.
  "IKE Policies": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ike-policies", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/ike-policies/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/ike-policies", { body: body as components["schemas"]["IkePolicyPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ike-policies/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["IkePolicyPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ike-policies/{id}", { params: { path: { id } } });
    },
  },
  // IKE Proposals: name + authentication_method + encryption_algorithm +
  // authentication_algorithm + group + sa_lifetime + description + comments.
  // Paginated envelope.
  "IKE Proposals": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ike-proposals", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/ike-proposals/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/ike-proposals", { body: body as components["schemas"]["IkeProposalPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ike-proposals/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["IkeProposalPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ike-proposals/{id}", { params: { path: { id } } });
    },
  },
  // IPsec Policies: name + pfs_group + description + comments.
  // Paginated envelope.
  "IPsec Policies": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ipsec-policies", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/ipsec-policies/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/ipsec-policies", { body: body as components["schemas"]["IpsecPolicyPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ipsec-policies/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["IpsecPolicyPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ipsec-policies/{id}", { params: { path: { id } } });
    },
  },
  // IPsec Proposals: name + encryption_algorithm + authentication_algorithm +
  // sa_lifetime_seconds + sa_lifetime_data + description + comments.
  // Paginated envelope.
  "IPsec Proposals": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ipsec-proposals", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/ipsec-proposals/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/ipsec-proposals", { body: body as components["schemas"]["IpsecProposalPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ipsec-proposals/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["IpsecProposalPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ipsec-proposals/{id}", { params: { path: { id } } });
    },
  },
  // IPsec Profiles: name + mode + ike_policy_id (required) + ipsec_policy_id (required) +
  // description + comments. Paginated envelope.
  "IPsec Profiles": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/ipsec-profiles", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/ipsec-profiles/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/ipsec-profiles", { body: body as components["schemas"]["IpsecProfilePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/ipsec-profiles/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["IpsecProfilePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/ipsec-profiles/{id}", { params: { path: { id } } });
    },
  },
  // ── Circuits remainder ──
  "Circuit Groups": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/circuit-groups", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/circuit-groups/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/circuit-groups", { body: body as components["schemas"]["CircuitGroupPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/circuit-groups/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["CircuitGroupPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/circuit-groups/{id}", { params: { path: { id } } });
    },
  },
  "Provider Accounts": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/provider-accounts", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/provider-accounts/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/provider-accounts", { body: body as components["schemas"]["ProviderAccountPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/provider-accounts/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ProviderAccountPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/provider-accounts/{id}", { params: { path: { id } } });
    },
  },
  "Circuit Terminations": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/circuit-terminations", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/circuit-terminations/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/circuit-terminations", { body: body as components["schemas"]["CircuitTerminationPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/circuit-terminations/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["CircuitTerminationPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/circuit-terminations/{id}", { params: { path: { id } } });
    },
  },
  "Virtual Circuit Types": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/virtual-circuit-types", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/virtual-circuit-types/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/virtual-circuit-types", { body: body as components["schemas"]["VirtualCircuitTypePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/virtual-circuit-types/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["VirtualCircuitTypePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/virtual-circuit-types/{id}", { params: { path: { id } } });
    },
  },
  "Virtual Circuits": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/virtual-circuits", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/virtual-circuits/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/virtual-circuits", { body: body as components["schemas"]["VirtualCircuitPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/virtual-circuits/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["VirtualCircuitPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/virtual-circuits/{id}", { params: { path: { id } } });
    },
  },
  // ── DCIM gaps ──
  "Power Panels": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/power-panels", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/power-panels/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/power-panels", { body: body as components["schemas"]["PowerPanelPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/power-panels/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["PowerPanelPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/power-panels/{id}", { params: { path: { id } } });
    },
  },
  "Power Feeds": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/power-feeds", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/power-feeds/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/power-feeds", { body: body as components["schemas"]["PowerFeedPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/power-feeds/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["PowerFeedPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/power-feeds/{id}", { params: { path: { id } } });
    },
  },
  "Rack Reservations": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/rack-reservations", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/rack-reservations/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/rack-reservations", { body: body as components["schemas"]["RackReservationPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/rack-reservations/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["RackReservationPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/rack-reservations/{id}", { params: { path: { id } } });
    },
  },
  "MAC Addresses": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/mac-addresses", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/mac-addresses/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/mac-addresses", { body: body as components["schemas"]["MacAddressPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/mac-addresses/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["MacAddressPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/mac-addresses/{id}", { params: { path: { id } } });
    },
  },
  "Virtual Device Contexts": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/virtual-device-contexts", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.GET("/api/v1/inventory/virtual-device-contexts/{id}", { params: { path: { id } } }),
        ),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/virtual-device-contexts", { body: body as components["schemas"]["VirtualDeviceContextPayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/virtual-device-contexts/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["VirtualDeviceContextPayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/virtual-device-contexts/{id}", { params: { path: { id } } });
    },
  },
  // ── IPAM gaps ──
  "Service Templates": {
    list: async (query) => listInventoryEnvelope("/api/v1/inventory/service-templates", query),
    get: async (id) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.GET("/api/v1/inventory/service-templates/{id}", { params: { path: { id } } })),
      ),
    create: async (body) =>
      asBackendInventoryRecord(
        unwrap(await apiClient.POST("/api/v1/inventory/service-templates", { body: body as components["schemas"]["ServiceTemplatePayload"] })),
      ),
    update: async (id, body) =>
      asBackendInventoryRecord(
        unwrap(
          await apiClient.PUT("/api/v1/inventory/service-templates/{id}", {
            params: { path: { id } },
            body: body as components["schemas"]["ServiceTemplatePayload"],
          }),
        ),
      ),
    delete: async (id) => {
      await apiClient.DELETE("/api/v1/inventory/service-templates/{id}", { params: { path: { id } } });
    },
  },
};

/** Map a raw `ObjectRef` (or null) into the UI `ObjectRefProjection`. Returns
 *  `null` when the assignment itself is absent. A dangling reference (target row
 *  gone) keeps `label: null` so the UI can render a "Deleted object" hint. */
function toObjectRefProjection(ref: BackendObjectRef | null | undefined): ObjectRefProjection | null {
  if (!ref) return null;
  return { objectType: ref.object_type, objectId: ref.object_id, label: ref.label ?? null };
}

/** Short display string for a generic-FK target, used inside synthesized row
 *  labels (e.g. contact-assignment names). Falls back to the token + a short id
 *  when the human label is missing (dangling reference). */
function objectRefDisplay(ref: BackendObjectRef | null | undefined): string | undefined {
  if (!ref) return undefined;
  if (ref.label) return ref.label;
  return `${ref.object_type} #${ref.object_id.slice(0, 8)}`;
}

function toResource(entity: InventoryEntityKey, raw: BackendInventoryRecord): Resource {
  if (entity === "Owners" || entity === "Owner Groups") {
    const resource: Resource = { id: raw.id, name: raw.name ?? raw.id, type: ENTITY_RESOURCE_TYPE[entity], description: raw.description ?? "" };
    if (entity === "Owners") {
      resource.groupId = raw.group_id ?? null;
      resource.users = raw.users ?? [];
      resource.userGroups = raw.user_groups ?? [];
    }
    return resource;
  }
  // `address` is overloaded: a postal address for Contacts and an IPAM address
  // for IP Addresses.
  const isContact = entity === "Contacts";
  const isAssignment = entity === "Contact Assignments";
  // Device types have no `name` column — `model` is the display field.
  const isDeviceType = entity === "Device Types";
  // Cables are nameless on the wire; `label` stands in for the table/search
  // name when present.
  const isCable = entity === "Cables";
  // Contact assignments carry no name column; synthesize a readable label so
  // the shared table/search (`r.name`) renders something meaningful.
  const assignmentLabel = isAssignment
    ? [raw.contact_name, objectRefDisplay(raw.object)]
        .filter((part): part is string => Boolean(part))
        .join(" → ")
    : undefined;
  const cableLabel = isCable ? raw.label ?? undefined : undefined;
  // Prefixes and IP Addresses are nameless on the wire — the CIDR/address
  // stands in for the table/search name.
  const ipamLabel =
    entity === "Prefixes" ? raw.prefix : entity === "IP Addresses" ? raw.address : undefined;
  // VLANs carry `name` but the operator scans by vid; prefix it for the label.
  const vlanLabel =
    entity === "VLANs" && raw.vid != null
      ? [`${raw.vid}`, raw.name].filter((part): part is string => Boolean(part)).join(" · ")
      : undefined;
  // ── Nameless entities ──
  // Aggregates carry no `name` — the CIDR prefix stands in for the table/search.
  // ASNs carry no `name` — render the AS number. IP Ranges carry no `name` —
  // render the start→end span. Modules carry no `name` — use the module type.
  // Module Types have no `name` column — `model` is the display field.
  const isModuleType = entity === "Module Types";
  const wave2Label =
    entity === "Aggregates"
      ? raw.prefix
      : entity === "ASNs"
        ? raw.asn != null
          ? `AS${raw.asn}`
          : undefined
        : entity === "IP Ranges"
          ? [raw.start_address, raw.end_address]
              .filter((part): part is string => Boolean(part))
              .join(" → ")
          : entity === "Modules"
            ? raw.module_type_name ?? undefined
            : undefined;
  // Circuits are nameless on the wire — `cid` stands in for the display name.
  const circuitLabel = entity === "Circuits" ? raw.cid ?? undefined : undefined;
  const tunnelTerminationLabel = entity === "Tunnel Terminations" ? raw.role ?? undefined : undefined;
  const customFields =
    raw.custom_fields ?? raw.custom_field_data ?? raw.role_custom_fields ?? undefined;
  const persistedIcon = extractPersistedIcon(customFields);
  return {
    users: raw.users,
    userGroups: raw.user_groups,
    id: raw.id,
    name:
      assignmentLabel ||
      cableLabel ||
      vlanLabel ||
      ipamLabel ||
      wave2Label ||
      circuitLabel ||
      tunnelTerminationLabel ||
      (isDeviceType || isModuleType ? raw.model : undefined) ||
      raw.name ||
      raw.slug ||
      raw.id,
    status: raw.status as ResourceStatus | undefined,
    type: ENTITY_RESOURCE_TYPE[entity],
    customFields,
    // Device/VM local config context override (arbitrary JSON); other entities
    // never carry it (the field is absent from their responses).
    localContextData: raw.local_context_data ?? undefined,
    iconName: persistedIcon.name,
    iconDataUrl: persistedIcon.dataUrl,
    primaryIp: raw.primary_ip ?? (isContact ? undefined : raw.address ?? undefined),
    description: raw.description ?? undefined,
    createdAt: raw.created_at ?? undefined,
    updatedAt: raw.updated_at ?? undefined,
    site: raw.site_name ?? undefined,
    tenant: raw.tenant_name ?? undefined,
    region: raw.region_name ?? undefined,
    role: raw.role_name ?? undefined,
    platform: raw.platform_name ?? undefined,
    deviceType: raw.device_type_name ?? undefined,
    cluster: raw.cluster_name ?? undefined,
    siteGroupName: raw.site_group_name ?? undefined,
    parentName: raw.parent_name ?? raw.parent_id ?? undefined,
    groupName: raw.group_name ?? undefined,
    deviceName: raw.device_name ?? undefined,
    vmName: raw.vm_name ?? undefined,
    contactName: raw.contact_name ?? undefined,
    // Contact scalar fields.
    title: raw.title ?? undefined,
    phone: raw.phone ?? undefined,
    email: raw.email ?? undefined,
    address: isContact ? raw.address ?? undefined : undefined,
    link: raw.link ?? undefined,
    // Polymorphic generic-FK targets — materialized ObjectRef projections.
    object: toObjectRefProjection(raw.object),
    parentObject: toObjectRefProjection(raw.parent_object),
    assignedObject: toObjectRefProjection(raw.assigned_object),
    termination: toObjectRefProjection(raw.termination),
    component: toObjectRefProjection(raw.component),
    priority: raw.priority ?? undefined,
    // DCIM scalar fields (stringified for the form). `type` is overloaded by the
    // wire as the resource type marker AND as a rack/cable/interface attribute;
    // entity guards keep them from leaking onto unrelated entities.
    model: raw.model ?? undefined,
    slug: raw.slug ?? undefined,
    napalmDriver: raw.napalm_driver ?? undefined,
    facilityId: raw.facility_id ?? undefined,
    serial: raw.serial ?? undefined,
    assetTag: raw.asset_tag ?? undefined,
    partId: raw.part_id ?? undefined,
    discovered: raw.discovered != null ? String(raw.discovered) : undefined,
    position: raw.position != null ? String(raw.position) : undefined,
    face: raw.face ?? undefined,
    airflow: raw.airflow ?? undefined,
    width: raw.width != null ? String(raw.width) : undefined,
    uHeight: raw.u_height != null ? String(raw.u_height) : undefined,
    rackType: entity === "Racks" || entity === "Power Feeds" ? raw.type ?? undefined : undefined,
    // `interfaceType` carries the wire `type` string for both Interfaces and
    // Cables (they never coexist on one entity, so the key is unambiguous).
    interfaceType: entity === "Interfaces" || entity === "Cables" ? raw.type ?? undefined : undefined,
    enabled: raw.enabled != null ? String(raw.enabled) : undefined,
    color: raw.color ?? undefined,
    length: raw.length != null ? String(raw.length) : undefined,
    siteId: raw.site_id ?? undefined,
    tenantId: raw.tenant_id ?? undefined,
    regionId: raw.region_id ?? undefined,
    roleId: raw.role_id != null ? String(raw.role_id) : undefined,
    platformId: raw.platform_id ?? undefined,
    deviceTypeId: raw.device_type_id ?? undefined,
    clusterId: raw.cluster_id ?? undefined,
    siteGroupId: raw.site_group_id ?? undefined,
    deviceId: raw.device_id ?? undefined,
    vmId: raw.vm_id ?? undefined,
    parentId: raw.parent_id ?? undefined,
    groupId: raw.group_id ?? undefined,
    contactId: raw.contact_id ?? undefined,
    manufacturerId: raw.manufacturer_id ?? undefined,
    manufacturerName: raw.manufacturer_name ?? undefined,
    // IPAM + Virtualization scalar fields. Numbers/booleans are
    // stringified here; toBackendPayload coerces them back on write.
    // For IP Addresses the CIDR/host string lives in `address` on the wire;
    // we store it in `prefix` so the shared form field key round-trips.
    vid: raw.vid != null ? String(raw.vid) : undefined,
    prefix: entity === "IP Addresses" ? (raw.address ?? undefined) : (raw.prefix ?? undefined),
    dnsName: raw.dns_name ?? undefined,
    isPool: raw.is_pool != null ? String(raw.is_pool) : undefined,
    vrf: raw.vrf_id != null ? String(raw.vrf_id) : undefined,
    ipamRole: raw.role ?? undefined,
    macAddress: raw.mac_address ?? undefined,
    mtu: raw.mtu != null ? String(raw.mtu) : undefined,
    mode: raw.mode ?? undefined,
    vcpus: raw.vcpus != null ? String(raw.vcpus) : undefined,
    virtualMachineTypeId: raw.virtual_machine_type_id ?? undefined,
    virtualMachineType: raw.virtual_machine_type_name ?? raw.virtual_machine_type_id ?? undefined,
    defaultPlatformId: raw.default_platform_id ?? undefined,
    defaultPlatform: raw.default_platform_id ?? undefined,
    defaultVcpus: raw.default_vcpus != null ? String(raw.default_vcpus) : undefined,
    defaultMemory: raw.default_memory != null ? String(raw.default_memory) : undefined,
    virtualMachineCount: raw.virtual_machine_count != null ? String(raw.virtual_machine_count) : undefined,
    memory: raw.memory != null ? String(raw.memory) : undefined,
    disk: raw.disk != null ? String(raw.disk) : undefined,
    // FK UUIDs for IPAM + Virtualization relations.
    clusterTypeId: raw.cluster_type_id ?? undefined,
    clusterGroupId: raw.cluster_group_id ?? undefined,
    vlanId: raw.vlan_id ?? undefined,
    virtualMachineId: raw.virtual_machine_id ?? undefined,
    interfaceId: raw.interface_id ?? undefined,
    // Resolved names for IPAM + Virtualization FK relations.
    clusterTypeName: raw.cluster_type_name ?? undefined,
    clusterGroupName: raw.cluster_group_name ?? undefined,
    vlanName: raw.vlan_name ?? undefined,
    virtualMachineName: raw.virtual_machine_name ?? undefined,
    interfaceName: raw.interface_name ?? undefined,
    // Rack Types / Locations scalar fields.
    formFactor: raw.form_factor ?? undefined,
    weight: raw.weight != null ? String(raw.weight) : undefined,
    facility: raw.facility ?? undefined,
    // ── Scalar fields (stringified for the form). ──
    isPrivate: raw.is_private != null ? String(raw.is_private) : undefined,
    rd: raw.rd ?? undefined,
    enforceUnique: raw.enforce_unique != null ? String(raw.enforce_unique) : undefined,
    asn: raw.asn != null ? String(raw.asn) : undefined,
    // For Aggregates the CIDR lives in `prefix`; reuse the `prefixCidr` form key
    // so the shared field round-trips without colliding with IPAM `prefix`.
    prefixCidr: entity === "Aggregates" ? raw.prefix ?? undefined : undefined,
    startAddress: raw.start_address ?? undefined,
    endAddress: raw.end_address ?? undefined,
    protocol: raw.protocol ?? undefined,
    ports: Array.isArray(raw.ports) ? raw.ports.join(", ") : undefined,
    portMappings: raw.port_mappings?.join(", "),
    serviceIpAddressIds: raw.ipaddresses,
    size: raw.size != null ? String(raw.size) : undefined,
    serialNumber: entity === "Modules" ? raw.serial ?? undefined : undefined,
    assetTagModule: entity === "Modules" ? raw.asset_tag ?? undefined : undefined,
    domain: raw.domain ?? undefined,
    partNumber: raw.part_number ?? undefined,
    comments: raw.comments ?? undefined,
    schemaSummary: raw.schema != null ? JSON.stringify(raw.schema) : undefined,
    // ── FK UUIDs + resolved names. ──
    rirId: raw.rir_id ?? undefined,
    rirName: raw.rir_name ?? undefined,
    vrfId: typeof raw.vrf_id === "string" ? raw.vrf_id : undefined,
    vrfName: raw.vrf_name ?? (raw.vrf_id != null ? String(raw.vrf_id) : undefined),
    moduleTypeId: raw.module_type_id ?? undefined,
    moduleTypeName: raw.module_type_name ?? undefined,
    moduleBayId: raw.module_bay_id ?? undefined,
    moduleBayName: raw.module_bay_name ?? undefined,
    masterId: raw.master_id ?? undefined,
    masterName: raw.master_name ?? undefined,
    // ── VPN scalar fields + FK UUIDs + resolved names. ──
    encapsulation: raw.encapsulation ?? undefined,
    presharedKey: raw.preshared_key ?? undefined,
    pfsGroup: raw.pfs_group != null ? String(raw.pfs_group) : undefined,
    saLifetime: raw.sa_lifetime != null ? String(raw.sa_lifetime) : undefined,
    saLifetimeSeconds: raw.sa_lifetime_seconds != null ? String(raw.sa_lifetime_seconds) : undefined,
    saLifetimeData: raw.sa_lifetime_data != null ? String(raw.sa_lifetime_data) : undefined,
    authenticationMethod: raw.authentication_method ?? undefined,
    encryptionAlgorithm: raw.encryption_algorithm ?? undefined,
    authenticationAlgorithm: raw.authentication_algorithm ?? undefined,
    vpnMode: raw.mode ?? undefined,
    vpnVersion: raw.version != null ? String(raw.version) : undefined,
    tunnelId: raw.tunnel_id != null ? String(raw.tunnel_id) : undefined,
    vpnRole: raw.role ?? undefined,
    tunnelGroupId: raw.group_id_vpn ?? raw.group_id ?? undefined,
    ipsecProfileId: raw.ipsec_profile_id ?? undefined,
    ikePolicyId: raw.ike_policy_id ?? undefined,
    ipsecPolicyId: raw.ipsec_policy_id ?? undefined,
    tunnelFkId: typeof raw.tunnel_id === "string" ? raw.tunnel_id : undefined,
    outsideIpId: raw.outside_ip_id ?? undefined,
    outsideIpAddress: raw.outside_ip ?? raw.outside_ip_id ?? undefined,
    tunnelGroupName: raw.tunnel_group_name ?? undefined,
    ipsecProfileName: raw.ipsec_profile_name ?? undefined,
    ikePolicyName: raw.ike_policy_name ?? undefined,
    ipsecPolicyName: raw.ipsec_policy_name ?? undefined,
    tunnelFkName: raw.tunnel_name ?? raw.tunnel_fk_name ?? undefined,
    // ── FK UUIDs + resolved names + scalars. ──
    providerId: raw.provider_id ?? undefined,
    providerName: raw.provider_name ?? raw.provider_account_name ?? undefined,
    circuitTypeId: raw.type_id ?? undefined,
    circuitTypeName: raw.type_name ?? undefined,
    cid: raw.cid ?? undefined,
    serviceId: raw.service_id ?? undefined,
    providerAccountId: raw.provider_account_id ?? undefined,
    terminationAId: raw.termination_a_id ?? undefined,
    terminationZId: raw.termination_z_id ?? undefined,
    installDate: raw.install_date ?? undefined,
    terminationDate: raw.termination_date ?? undefined,
    commitRate: raw.commit_rate != null ? String(raw.commit_rate) : undefined,
    distance: raw.distance != null ? String(raw.distance) : undefined,
    distanceUnit: raw.distance_unit ?? undefined,
    asnStart: raw.start != null ? String(raw.start) : undefined,
    asnEnd: raw.end != null ? String(raw.end) : undefined,
    // ── Scalar fields + FK UUIDs + resolved names. ──
    account: raw.account ?? undefined,
    termSide: raw.term_side ?? undefined,
    xconnectId: raw.xconnect_id ?? undefined,
    ppInfo: raw.pp_info ?? undefined,
    portSpeed: raw.port_speed != null ? String(raw.port_speed) : undefined,
    upstreamSpeed: raw.upstream_speed != null ? String(raw.upstream_speed) : undefined,
    markConnected: raw.mark_connected != null ? String(raw.mark_connected) : undefined,
    circuitId: raw.circuit_id ?? undefined,
    circuitName: raw.circuit_name ?? undefined,
    providerNetworkId: raw.provider_network_id ?? undefined,
    providerNetworkName: raw.provider_network_name ?? undefined,
    virtualCircuitTypeId: raw.virtual_circuit_type_id ?? undefined,
    virtualCircuitTypeName: raw.virtual_circuit_type_name ?? undefined,
    supply: raw.supply ?? undefined,
    phase: raw.phase ?? undefined,
    voltage: raw.voltage != null ? String(raw.voltage) : undefined,
    amperage: raw.amperage != null ? String(raw.amperage) : undefined,
    maxUtilization: raw.max_utilization != null ? String(raw.max_utilization) : undefined,
    availablePower: raw.available_power != null ? String(raw.available_power) : undefined,
    units: Array.isArray(raw.units) ? raw.units.join(", ") : undefined,
    userId: raw.user_id ?? undefined,
    userName: raw.user_name ?? raw.user_id ?? undefined,
    ownerId: raw.owner_id ?? undefined,
    ownerName: raw.owner_name ?? raw.owner_id ?? undefined,
    powerPanelId: raw.power_panel_id ?? undefined,
    powerPanelName: raw.power_panel_name ?? undefined,
    rackId: raw.rack_id ?? undefined,
    rackName: raw.rack_name ?? undefined,
    identifier: raw.identifier != null ? String(raw.identifier) : undefined,
    primaryIp4Id: raw.primary_ip4_id ?? undefined,
    primaryIp6Id: raw.primary_ip6_id ?? undefined,
    primaryIp4: raw.primary_ip4 ?? raw.primary_ip4_id ?? undefined,
    primaryIp6: raw.primary_ip6 ?? raw.primary_ip6_id ?? undefined,
    // ── Form field expansion scalar/FK prefill (stringified for the form). ──
    timeZone: raw.time_zone ?? undefined,
    physicalAddress: raw.physical_address ?? undefined,
    shippingAddress: raw.shipping_address ?? undefined,
    latitude: raw.latitude != null ? String(raw.latitude) : undefined,
    longitude: raw.longitude != null ? String(raw.longitude) : undefined,
    vmRole: raw.vm_role != null ? String(raw.vm_role) : undefined,
    descUnits: raw.desc_units != null ? String(raw.desc_units) : undefined,
    outerWidth: raw.outer_width != null ? String(raw.outer_width) : undefined,
    outerDepth: raw.outer_depth != null ? String(raw.outer_depth) : undefined,
    outerHeight: raw.outer_height != null ? String(raw.outer_height) : undefined,
    outerUnit: raw.outer_unit ?? undefined,
    startingUnit: raw.starting_unit != null ? String(raw.starting_unit) : undefined,
    mountingDepth: raw.mounting_depth != null ? String(raw.mounting_depth) : undefined,
    maxWeight: raw.max_weight != null ? String(raw.max_weight) : undefined,
    weightUnit: raw.weight_unit ?? undefined,
    // `label` is the cable display name; for Interfaces and Inventory Items it
    // is a writable scalar.
    label:
      entity === "Interfaces" || entity === "Inventory Items"
        ? (raw.label ?? undefined)
        : undefined,
    mgmtOnly: raw.mgmt_only != null ? String(raw.mgmt_only) : undefined,
    lengthUnit: raw.length_unit ?? undefined,
    markUtilized: raw.mark_utilized != null ? String(raw.mark_utilized) : undefined,
    prefixUtilizedAddresses:
      raw.utilized_addresses != null ? Number(raw.utilized_addresses) : undefined,
    prefixUsableAddresses: raw.usable_addresses != null ? Number(raw.usable_addresses) : undefined,
    prefixUtilizationPercent:
      raw.utilization_percent != null ? Number(raw.utilization_percent) : undefined,
    markPopulated: raw.mark_populated != null ? String(raw.mark_populated) : undefined,
    dateAdded: raw.date_added ?? undefined,
    rackRoleId: raw.rack_role_id ?? undefined,
    rackRoleName: raw.rack_role_name ?? undefined,
  };
}

/** Default page size the UI requests from the canonical hydrated endpoint.
 *  Matches the backend's MAX_PAGINATION_LIMIT so a single request yields the
 *  full small-tenant inventory. The server returns the resolved `limit`/
 *  `offset` in the envelope so the UI can page correctly when needed. */
const DEFAULT_PAGE_SIZE = 1000;

/** GET /api/v1/inventory/{entity}. The response is the canonical envelope
 *  `{items, total, limit, offset}` with *_name fields hydrated where relevant. */
export async function listResources(
  entity: InventoryEntityKey,
  q?: ListQuery,
): Promise<Paginated<Resource>> {
  const limit = q?.limit ?? DEFAULT_PAGE_SIZE;
  const offset = q?.offset ?? 0;
  const search = q?.search || undefined;

  const envelope = await ENTITY_API[entity].list({ limit, offset, search });
  const items = envelope.items.map((r) => toResource(entity, r));
  return { items, total: envelope.total, limit: envelope.limit, offset: envelope.offset };
}

/**
 * Exhaustively loads an entity list across all pages.
 *
 * FK filter pickers must not silently depend on the first page only: otherwise
 * a visible row can reference a related object whose option never appears in
 * the funnel. This helper walks the paginated inventory list until the server's
 * reported `total` is exhausted and returns a single merged item array.
 */
export async function listAllResources(
  entity: InventoryEntityKey,
  q?: Omit<ListQuery, "limit" | "offset">,
): Promise<Resource[]> {
  const search = q?.search || undefined;
  return collectAllPages(
    (limit, offset) => listResources(entity, { search, limit, offset }),
    DEFAULT_PAGE_SIZE,
  );
}

export async function getResource(entity: InventoryEntityKey, id: string): Promise<Resource> {
  const raw = await ENTITY_API[entity].get(id);
  return toResource(entity, raw);
}

export interface ResourceWithRaw {
  resource: Resource;
  /**
   * Raw snake_case backend record. The read-only inspector completeness pass
   * reads this because `toResource` intentionally drops fields the CRUD UI does
   * not need (external_id, tags, asns, sync timestamps, …).
   */
  raw: Record<string, unknown>;
}

/**
 * Single fetch returning BOTH the mapped `Resource` (resolved FK names for the
 * curated rows) and the raw backend record (full field set for the inspector's
 * completeness rows). One round-trip; the inspector needs both shapes.
 */
export async function getResourceWithRaw(
  entity: InventoryEntityKey,
  id: string,
): Promise<ResourceWithRaw> {
  const raw = await ENTITY_API[entity].get(id);
  return { resource: toResource(entity, raw), raw: raw as unknown as Record<string, unknown> };
}

type InventoryRouteQuery = InventoryListParams & Record<string, string | number | boolean | undefined>;

async function listResourcesByRouteQuery(
  entity: InventoryEntityKey,
  path: keyof paths,
  query: InventoryRouteQuery,
): Promise<Resource[]> {
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: InventoryRouteQuery } },
    ) => Promise<{ data?: unknown; error?: unknown }>)(path, { params: { query } }),
  );
  const envelope = asBackendInventoryEnvelope(raw);
  return envelope.items.map((r) => toResource(entity, r));
}

/** A parent-scoped child page that preserves the server `total`, so callers can
 *  detect truncation (`offset + rows.length < total`). The plain
 *  `listResourcesByRouteQuery` returns only the row array, which is enough for
 *  option pickers but loses the count the nested editor needs to block deletes
 *  on a partially-loaded level. */
export interface InventoryChildPage {
  rows: Resource[];
  total: number;
  limit: number;
  offset: number;
}

/** Hard server cap on a single inventory list page (mirrors the backend MAX).
 *  The nested editor requests at most this many child rows per level. */
export const INVENTORY_MAX_PAGE_SIZE = 1000;

/** Envelope-preserving variant of {@link listResourcesByRouteQuery}: returns the
 *  hydrated rows plus the server `total`/`limit`/`offset`. Single source for the
 *  nested-editor child lists (it must know `total` to flag truncation). */
export async function listChildResourcesByRouteQuery(
  entity: InventoryEntityKey,
  path: keyof paths,
  query: InventoryRouteQuery,
): Promise<InventoryChildPage> {
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: InventoryRouteQuery } },
    ) => Promise<{ data?: unknown; error?: unknown }>)(path, { params: { query } }),
  );
  const envelope = asBackendInventoryEnvelope(raw);
  return {
    rows: envelope.items.map((r) => toResource(entity, r)),
    total: envelope.total,
    limit: envelope.limit,
    offset: envelope.offset,
  };
}

/** IP addresses assigned to a single interface (or VM interface), via the
 *  proven `assigned_object_id` filter the backend resolves to `interface_id`.
 *  Used by the nested editor's Interface → IP level. */
export function listIpAddressesForInterface(
  interfaceId: string,
  opts?: { limit?: number; offset?: number },
): Promise<InventoryChildPage> {
  return listChildResourcesByRouteQuery("IP Addresses", "/api/v1/inventory/ip-addresses", {
    assigned_object_id: interfaceId,
    limit: opts?.limit ?? DEFAULT_PAGE_SIZE,
    offset: opts?.offset ?? 0,
  });
}

export function listIpAddressesForPrefix(
  prefix: string,
  opts?: { limit?: number; offset?: number; vrfId?: string | null },
): Promise<Paginated<Resource>> {
  const filters: Record<string, string[]> = {
    parent_prefix: [prefix],
  };
  if (opts?.vrfId) filters.vrf_id = [opts.vrfId];

  return listResourcesForTable("IP Addresses", {
    limit: opts?.limit ?? DEFAULT_PAGE_SIZE,
    offset: opts?.offset ?? 0,
    sort: "address",
    order: "asc",
    filters,
  });
}

export function listIpRangesForPrefix(
  prefix: string,
  opts?: { limit?: number; offset?: number; vrfId?: string | null },
): Promise<Paginated<Resource>> {
  const filters: Record<string, string[]> = {
    parent_prefix: [prefix],
  };
  if (opts?.vrfId) filters.vrf_id = [opts.vrfId];

  return listResourcesForTable("IP Ranges", {
    limit: opts?.limit ?? DEFAULT_PAGE_SIZE,
    offset: opts?.offset ?? 0,
    sort: "start_address",
    order: "asc",
    filters,
  });
}

/** Route path per entity for the table-fetch path. Every entity that carries a
 *  `tableConfig` entry (sortable columns and/or column filters) MUST appear
 *  here, or `listResourcesForTable` falls back to the plain `listResources`
 *  path and silently drops the `sort`/`filters` params — leaving the funnel and
 *  sort header inert. The backend list endpoints for all of these accept the
 *  allowlisted sort + multi-value filter params; the strings mirror the
 *  per-entity `apiClient.GET` list routes in `ENTITY_API` (single source of
 *  truth for route choices). Entities without a `tableConfig` entry render no
 *  funnels/sort affordances, so omitting them here is intentional. */
const ENTITY_LIST_PATH: Partial<Record<InventoryEntityKey, keyof paths>> = {
  Owners: "/api/v1/inventory/owners",
  "Owner Groups": "/api/v1/inventory/owner_groups",
  Devices: "/api/v1/inventory/devices",
  Sites: "/api/v1/inventory/sites",
  Clusters: "/api/v1/inventory/clusters",
  "Device Types": "/api/v1/inventory/device_types",
  Racks: "/api/v1/inventory/racks",
  Prefixes: "/api/v1/inventory/prefixes",
  VLANs: "/api/v1/inventory/vlans",
  "Virtual Machines": "/api/v1/inventory/virtual_machines",
  "Virtual Machine Types": "/api/v1/inventory/virtual-machine-types",
  Regions: "/api/v1/inventory/regions",
  Tenants: "/api/v1/inventory/tenants",
  "Tenant Groups": "/api/v1/inventory/tenant_groups",
  "Site Groups": "/api/v1/inventory/site_groups",
  "Device Roles": "/api/v1/inventory/device_roles",
  Platforms: "/api/v1/inventory/platforms",
  "Provider Networks": "/api/v1/inventory/provider-networks",
  Circuits: "/api/v1/inventory/circuits",
  "Provider Accounts": "/api/v1/inventory/provider-accounts",
  "Virtual Circuits": "/api/v1/inventory/virtual-circuits",
  Contacts: "/api/v1/inventory/contacts",
  "Contact Roles": "/api/v1/inventory/contact-roles",
  "Contact Groups": "/api/v1/inventory/contact-groups",
  "Contact Assignments": "/api/v1/inventory/contact-assignments",
  Manufacturers: "/api/v1/inventory/manufacturers",
  "Circuit Types": "/api/v1/inventory/circuit-types",
  Providers: "/api/v1/inventory/providers",
  "Cluster Types": "/api/v1/inventory/cluster_types",
  "Cluster Groups": "/api/v1/inventory/cluster_groups",
  "ASN Ranges": "/api/v1/inventory/asn-ranges",
  "Inventory Item Roles": "/api/v1/inventory/inventory-item-roles",
  "Inventory Items": "/api/v1/inventory/inventory-items",
  "Tunnel Groups": "/api/v1/inventory/tunnel-groups",
  Tunnels: "/api/v1/inventory/tunnels",
  "Tunnel Terminations": "/api/v1/inventory/tunnel-terminations",
  "IKE Policies": "/api/v1/inventory/ike-policies",
  "IKE Proposals": "/api/v1/inventory/ike-proposals",
  "IPsec Policies": "/api/v1/inventory/ipsec-policies",
  "IPsec Proposals": "/api/v1/inventory/ipsec-proposals",
  "IPsec Profiles": "/api/v1/inventory/ipsec-profiles",
  "Circuit Groups": "/api/v1/inventory/circuit-groups",
  "Circuit Terminations": "/api/v1/inventory/circuit-terminations",
  "Virtual Circuit Types": "/api/v1/inventory/virtual-circuit-types",
  "Power Panels": "/api/v1/inventory/power-panels",
  "Power Feeds": "/api/v1/inventory/power-feeds",
  "Rack Reservations": "/api/v1/inventory/rack-reservations",
  "MAC Addresses": "/api/v1/inventory/mac-addresses",
  "Module Type Profiles": "/api/v1/inventory/module-type-profiles",
  Tags: "/api/v1/inventory/tags",
  "Virtual Device Contexts": "/api/v1/inventory/virtual-device-contexts",
  "Service Templates": "/api/v1/inventory/service-templates",
  "Rack Roles": "/api/v1/inventory/rack_roles",
  "Rack Types": "/api/v1/inventory/rack_types",
  Locations: "/api/v1/inventory/locations",
  Interfaces: "/api/v1/inventory/interfaces",
  Cables: "/api/v1/inventory/cables",
  "IP Addresses": "/api/v1/inventory/ip-addresses",
  "VM Interfaces": "/api/v1/inventory/vm-interfaces",
  RIRs: "/api/v1/inventory/rirs",
  VRFs: "/api/v1/inventory/vrfs",
  "Route Targets": "/api/v1/inventory/route_targets",
  Aggregates: "/api/v1/inventory/aggregates",
  ASNs: "/api/v1/inventory/asns",
  "IP Ranges": "/api/v1/inventory/ip_ranges",
  Services: "/api/v1/inventory/services",
  Modules: "/api/v1/inventory/modules",
  "Module Types": "/api/v1/inventory/module-types",
  "Virtual Chassis": "/api/v1/inventory/virtual-chassis",
  "Virtual Disks": "/api/v1/inventory/virtual-disks",
};

export function inventoryTableListPathFor(
  entity: InventoryEntityKey,
): keyof paths | undefined {
  return ENTITY_LIST_PATH[entity];
}

export function inventoryTableListPathEntities(): InventoryEntityKey[] {
  return Object.keys(ENTITY_LIST_PATH) as InventoryEntityKey[];
}

/** Structured input for a sort/filter table fetch. `sort`/`order` map to the
 *  backend's allowlisted sort contract; `filters` is `param -> selected values`
 *  and is serialized to comma-joined query strings (SQL `IN`). Empty filter
 *  value lists are dropped so an inactive filter sends no param. */
export interface InventoryTableQuery {
  limit: number;
  offset: number;
  search?: string;
  sort?: string;
  order?: "asc" | "desc";
  /** Backend param name -> selected values (joined with `,`). */
  filters?: Record<string, string[]>;
}

/** Build the flat query object sent to a sort/filter-aware list endpoint.
 *  Exported for unit testing the param-building contract (the new backend is
 *  not reachable from the local stack, so this is the unit under test). */
export function buildInventoryTableParams(query: InventoryTableQuery): InventoryRouteQuery {
  const params: InventoryRouteQuery = {
    limit: query.limit,
    offset: query.offset,
  };
  const search = query.search?.trim();
  if (search) params.search = search;
  if (query.sort) {
    params.sort = query.sort;
    params.order = query.order ?? "asc";
  }
  for (const [param, values] of Object.entries(query.filters ?? {})) {
    if (values.length > 0) params[param] = values.join(",");
  }
  return params;
}

/** Table-fetch path: lists an entity with the NEW backend sort + multi-value
 *  filter params and returns the FULL `{items,total,limit,offset}` envelope so
 *  the table can paginate. Falls back to the typed `listResources` path for
 *  entities without sort/filter support (degrade gracefully). */
export async function listResourcesForTable(
  entity: InventoryEntityKey,
  query: InventoryTableQuery,
): Promise<Paginated<Resource>> {
  const path = ENTITY_LIST_PATH[entity];
  if (!path) {
    return listResources(entity, {
      limit: query.limit,
      offset: query.offset,
      search: query.search,
    });
  }
  const params = buildInventoryTableParams(query);
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: InventoryRouteQuery } },
    ) => Promise<{ data?: unknown; error?: unknown }>)(path, { params: { query: params } }),
  );
  const envelope = asBackendInventoryEnvelope(raw);
  return {
    items: envelope.items.map((r) => toResource(entity, r)),
    total: envelope.total,
    limit: envelope.limit,
    offset: envelope.offset,
  };
}

export function listDeviceInterfaces(deviceId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Interfaces", "/api/v1/inventory/interfaces", {
    device_id: deviceId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listVmInterfaces(virtualMachineId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("VM Interfaces", "/api/v1/inventory/vm-interfaces", {
    virtual_machine_id: virtualMachineId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listIpAddressesForDevice(deviceId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("IP Addresses", "/api/v1/inventory/ip-addresses", {
    device_id: deviceId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export interface PrefixTreeNode {
  id: string;
  node_id: string;
  prefix: string;
  parent_id?: string | null;
  parent_prefix?: string | null;
  root_id?: string | null;
  root_prefix?: string | null;
  depth: number;
  child_count: number;
  status: string;
  family: number;
  site_id?: string | null;
  site_name?: string | null;
  vrf_id?: string | null;
  vrf_name?: string | null;
  tenant_id?: string | null;
  tenant_name?: string | null;
  mark_utilized: boolean;
  utilized_addresses: number;
  usable_addresses?: number | null;
  utilization_percent?: number | null;
  breadcrumbs: string[];
}

export interface PrefixTreeResponse {
  nodes: PrefixTreeNode[];
  total: number;
  limit: number;
  applied_filters: {
    search?: string | null;
    root_id?: string | null;
    root_prefix?: string | null;
    family?: number | null;
    site_id: string[];
    vrf_id: string[];
    tenant_id: string[];
    status: string[];
    limit: number;
  };
}

export interface PrefixTreeQuery {
  search?: string;
  root_id?: string;
  root_prefix?: string;
  family?: 4 | 6;
  site_id?: string;
  vrf_id?: string;
  tenant_id?: string;
  status?: string;
  limit?: number;
}

export interface PrefixPlanningPreview {
  parent_id: string;
  parent_prefix: string;
  requested_prefix_length: number;
  status: string;
  candidate_prefix?: string | null;
  reason?: string | null;
  conflicts: string[];
  searched_candidates: number;
}

export async function listPrefixTree(query: PrefixTreeQuery): Promise<PrefixTreeResponse> {
  return unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: PrefixTreeQuery } },
    ) => Promise<{ data?: PrefixTreeResponse; error?: unknown }>)(
      "/api/v1/inventory/prefixes/tree",
      { params: { query } },
    ),
  );
}

export async function previewNextAvailablePrefix(
  prefixId: string,
  prefixLength: number,
): Promise<PrefixPlanningPreview> {
  return unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { path: { id: string }; query: { prefix_length: number } } },
    ) => Promise<{ data?: PrefixPlanningPreview; error?: unknown }>)(
      "/api/v1/inventory/prefixes/{id}/next-available-prefix/preview",
      { params: { path: { id: prefixId }, query: { prefix_length: prefixLength } } },
    ),
  );
}

export type CablingEndpointKind =
  | "interface"
  | "front-port"
  | "rear-port"
  | "console-port"
  | "console-server-port"
  | "power-port"
  | "power-outlet"
  | "power-feed"
  | "circuit-termination";

export type CablingLinkState = "available" | "cabled" | "marked-connected" | "unknown";
export type CablingTraceState =
  | "complete"
  | "incomplete"
  | "split"
  | "invalid"
  | "position-conflict"
  | "unsupported";
export type CablingTraversal = "cable" | "front-rear-map" | "power-chain" | "circuit-hand-off";
export type CablingIssueCode =
  | "open-end"
  | "multiple-next-hops"
  | "dangling-reference"
  | "position-mismatch"
  | "unsupported-endpoint"
  | "marked-connected"
  | "trace-truncated";

export interface CablingResourceLink {
  entity: string;
  id: string;
  label: string;
}

export interface CablingEndpointRef {
  kind: CablingEndpointKind;
  id: string;
  label: string;
}

export interface CablingLocation {
  site?: CablingResourceLink | null;
  rack?: CablingResourceLink | null;
  location?: CablingResourceLink | null;
}

export interface CablingCableSummary {
  id: string;
  label: string;
  status: string;
  color?: string | null;
}

export interface CablingEndpointItem {
  endpoint: CablingEndpointRef;
  parent?: CablingResourceLink | null;
  location: CablingLocation;
  link_state: CablingLinkState;
  trace_state?: CablingTraceState | null;
  cable?: CablingCableSummary | null;
  related_links: CablingResourceLink[];
}

export interface CablingTraceSegment {
  index: number;
  traversal: CablingTraversal;
  from: CablingEndpointRef;
  to?: CablingEndpointRef | null;
  cable?: CablingCableSummary | null;
  note?: string | null;
}

export interface CablingTraceIssue {
  code: CablingIssueCode;
  endpoint?: CablingEndpointRef | null;
  message: string;
}

export interface CablingTrace {
  start: CablingEndpointRef;
  state: CablingTraceState;
  summary: {
    hop_count: number;
    cable_count: number;
    issue_count: number;
  };
  segments: CablingTraceSegment[];
  issues: CablingTraceIssue[];
  related_links: CablingResourceLink[];
  generated_at: string;
}

export interface CablingEndpointQuery {
  query?: string;
  endpoint_kind?: CablingEndpointKind[];
  site_id?: string[];
  rack_id?: string[];
  device_id?: string[];
  link_state?: CablingLinkState[];
  trace_state?: CablingTraceState[];
  issues_only?: boolean;
  limit?: number;
}

function cablingCsv<T extends string>(values?: T[]): string | undefined {
  return values && values.length > 0 ? values.join(",") : undefined;
}

export async function listCablingEndpoints(
  query: CablingEndpointQuery,
): Promise<Paginated<CablingEndpointItem>> {
  return unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: Record<string, string | number | boolean | undefined> } },
    ) => Promise<{ data?: Paginated<CablingEndpointItem>; error?: unknown }>)(
      "/api/v1/inventory/cabling/endpoints",
      {
        params: {
          query: {
            query: query.query,
            endpoint_kind: cablingCsv(query.endpoint_kind),
            site_id: cablingCsv(query.site_id),
            rack_id: cablingCsv(query.rack_id),
            device_id: cablingCsv(query.device_id),
            link_state: cablingCsv(query.link_state),
            trace_state: cablingCsv(query.trace_state),
            issues_only: query.issues_only,
            limit: query.limit,
          },
        },
      },
    ),
  );
}

export async function traceCablingEndpoint(
  endpoint: CablingEndpointRef,
  maxHops = 50,
): Promise<CablingTrace> {
  return unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: {
        params: {
          query: { endpoint_kind: CablingEndpointKind; endpoint_id: string; max_hops: number };
        };
      },
    ) => Promise<{ data?: CablingTrace; error?: unknown }>)(
      "/api/v1/inventory/cabling/trace",
      {
        params: {
          query: {
            endpoint_kind: endpoint.kind,
            endpoint_id: endpoint.id,
            max_hops: maxHops,
          },
        },
      },
    ),
  );
}

export function listIpAddressesForVirtualMachine(virtualMachineId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("IP Addresses", "/api/v1/inventory/ip-addresses", {
    virtual_machine_id: virtualMachineId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

/* ── Parent-scoped relation lists (inspector RelatedEntitySection) ──
 * Each filters an inventory list endpoint by a single parent FK so the
 * connections inspector can render an entity's child collections (devices at a
 * site, VMs in a cluster, …). The route strings stay in this module (single
 * source of truth); `listResourcesByRouteQuery` threads the typed parent filter.
 * The `site_id`/`cluster_id`/… server-side filters are live on the backend. */
export function listDevicesForSite(siteId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Devices", "/api/v1/inventory/devices", {
    site_id: siteId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listRacksForSite(siteId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Racks", "/api/v1/inventory/racks", {
    site_id: siteId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listLocationsForSite(siteId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Locations", "/api/v1/inventory/locations", {
    site_id: siteId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listSitesForRegion(regionId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Sites", "/api/v1/inventory/sites", {
    region_id: regionId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listDevicesForCluster(clusterId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Devices", "/api/v1/inventory/devices", {
    cluster_id: clusterId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listVmsForCluster(clusterId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Virtual Machines", "/api/v1/inventory/virtual_machines", {
    cluster_id: clusterId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listDevicesForTenant(tenantId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Devices", "/api/v1/inventory/devices", {
    tenant_id: tenantId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

export function listVmsForTenant(tenantId: string): Promise<Resource[]> {
  return listResourcesByRouteQuery("Virtual Machines", "/api/v1/inventory/virtual_machines", {
    tenant_id: tenantId,
    limit: DEFAULT_PAGE_SIZE,
    offset: 0,
  });
}

/* ── Parent-scoped relation COUNTS (inspector RelatedEntitySection badge) ──
 * A cheap eager count for a parent-FK relation: requests `limit=1` and returns
 * only the envelope `total`, not the rows. The inspector fires these on card-open
 * (independent of section expansion) so the count badge shows the real number
 * without a click, while the full LIST fetchers above stay lazy (on expand). */
async function countResourcesByRouteQuery(
  path: string,
  query: Omit<InventoryRouteQuery, "limit" | "offset"> & { offset?: number },
): Promise<number> {
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: string,
      init: { params: { query: InventoryRouteQuery } },
    ) => Promise<{ data?: unknown; error?: unknown }>)(path, {
      params: { query: { ...query, limit: 1, offset: query.offset ?? 0 } },
    }),
  );
  return asBackendInventoryEnvelope(raw).total;
}

export function countDevicesForSite(siteId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/devices", { site_id: siteId });
}

export function countRacksForSite(siteId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/racks", { site_id: siteId });
}

export function countLocationsForSite(siteId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/locations", { site_id: siteId });
}

export function countSitesForRegion(regionId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/sites", { region_id: regionId });
}

export function countDevicesForCluster(clusterId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/devices", { cluster_id: clusterId });
}

export function countVmsForCluster(clusterId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/virtual_machines", { cluster_id: clusterId });
}

export function countDevicesForTenant(tenantId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/devices", { tenant_id: tenantId });
}

export function countVmsForTenant(tenantId: string): Promise<number> {
  return countResourcesByRouteQuery("/api/v1/inventory/virtual_machines", { tenant_id: tenantId });
}

/** Drop empty/undefined keys so optional fields stay absent on the wire. */
function compactPayload(payload: Record<string, unknown>): Record<string, unknown> {
  for (const k of Object.keys(payload)) if (payload[k] === undefined) delete payload[k];
  return payload;
}

/** Project a UI `ObjectRefProjection` into the wire `ObjectRefInput` shape
 *  (`{ object_type, object_id }` — `label` is response-only and rejected by
 *  `deny_unknown_fields`). Returns `undefined` when the target is unset so the
 *  field is omitted; `compactPayload` then drops it. */
function toObjectRefInput(
  ref: ObjectRefProjection | null | undefined,
): { object_type: string; object_id: string } | undefined {
  if (!ref || !ref.objectType || !ref.objectId) return undefined;
  return { object_type: ref.objectType, object_id: ref.objectId };
}

function withIconPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
  payload: Record<string, unknown>,
): Record<string, unknown> {
  const compacted = compactPayload(payload);
  if (!entitySupportsCustomIcon(entity)) return compacted;

  const iconWasEdited = "iconName" in body || "iconDataUrl" in body;
  if (!iconWasEdited && body.customFields === undefined) return compacted;

  const customFields = mergePersistedIcon(body.customFields, {
    name: isEntityIconName(body.iconName) ? body.iconName : undefined,
    dataUrl: isSafeIconDataUrl(body.iconDataUrl) ? body.iconDataUrl : undefined,
  });
  compacted.custom_fields = customFields ?? {};
  return compacted;
}

/** Parse a numeric form string; empty/non-numeric → undefined (field omitted). */
function toNumberOrUndefined(value?: string): number | undefined {
  if (value == null || value === "") return undefined;
  const n = Number(value);
  return Number.isNaN(n) ? undefined : n;
}

/** Nullable VM compute input: only an intentional empty value clears storage. */
function toVmComputeNumber(value: string | undefined, field: string): number | null | undefined {
  if (value === undefined) return undefined;
  if (value == null || value.trim() === "") return null;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) throw new Error(`${field} must be a finite number`);
  return parsed;
}

/** NetBox stores color columns as 6 hex digits without the UI-facing `#`. */
function toNetBoxColor(value?: string | null): string | undefined {
  if (value == null || value.trim() === "") return undefined;
  return normalizeHex(value)?.slice(1);
}

/** Map a "true"/"false" select value to a boolean; empty → undefined. */
function toBoolOrUndefined(value?: string): boolean | undefined {
  if (value === "true") return true;
  if (value === "false") return false;
  return undefined;
}

/** Parse a comma/space-separated numeric list (e.g. "80, 443") into number[];
 *  empty/no valid numbers → undefined (field omitted). */
function toNumberArray(value?: string): number[] | undefined {
  if (value == null || value.trim() === "") return undefined;
  const nums = value
    .split(/[,\s]+/)
    .map((part) => Number(part.trim()))
    .filter((n) => !Number.isNaN(n));
  return nums.length > 0 ? nums : undefined;
}

/** One entity family's payload builder: returns `undefined` when `entity` belongs
 *  to another family, so the dispatcher can fall through to the next builder. */
type EntityPayloadBuilder = (
  entity: InventoryEntityKey,
  body: Partial<Resource>,
  mode: "create" | "update",
) => Record<string, unknown> | undefined;

function toContactAssignmentPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // Contact assignments are a GFK record, not a named entity: their payload has
  // no `name`/`slug`/`description`, just the polymorphic target + contact/role.
  if (entity === "Contact Assignments") {
    // ContactAssignment{Create,Update}Payload carry the polymorphic target as
    // FLAT top-level `object_type` + `object_id` (the only generic-FK entity
    // whose write shape is not the nested `ObjectRefInput`).
    const payload: Record<string, unknown> = {
      contact_id: body.contactId,
      object_type: body.object?.objectType || undefined,
      object_id: body.object?.objectId || undefined,
      role_id: body.roleId,
      priority: body.priority,
    };
    for (const k of Object.keys(payload)) if (payload[k] === undefined) delete payload[k];
    return payload;
  }
  return undefined;
}

/** DCIM naming/FK payloads: manufacturers, sites, platforms, device types,
 *  racks, interfaces and cables. */
function toDcimPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── DCIM payloads — each emits only the fields its backend payload
  // schema accepts (derived from schema.gen.ts), coercing the string-typed form
  // values to the numeric/boolean wire types. ──

  // ManufacturerPayload: name + slug + description (untyped `unknown` body).
  if (entity === "Manufacturers") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      description: body.description,
    });
  }

  // SiteGroupPayload: name + slug + parent_id + description.
  if (entity === "Site Groups") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      parent_id: body.parentId,
      description: body.description,
    });
  }

  // PlatformCreatePayload: name + slug + manufacturer_id + napalm_driver + description.
  if (entity === "Platforms") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      manufacturer_id: body.manufacturerId,
      napalm_driver: body.napalmDriver,
      description: body.description,
    });
  }

  // DeviceTypePayload: manufacturer_id + model + slug + description + comments.
  // `name` from the generic form maps to `model`.
  if (entity === "Device Types") {
    const model = body.model ?? body.name;
    return compactPayload({
      manufacturer_id: body.manufacturerId,
      model,
      slug: model ? slugify(model) : undefined,
      description: body.description,
      comments: body.comments,
    });
  }

  // RackCreatePayload: name + site_id (+ status/type/facility_id/serial/asset_tag/
  // width/u_height/tenant_id). location_id and rack_role_id are not surfaced.
  if (entity === "Racks") {
    return compactPayload({
      name: body.name,
      site_id: body.siteId,
      tenant_id: body.tenantId,
      rack_role_id: body.rackRoleId,
      status: body.status,
      type: body.rackType,
      facility_id: body.facilityId,
      serial: body.serial,
      asset_tag: body.assetTag,
      width: toNumberOrUndefined(body.width),
      u_height: toNumberOrUndefined(body.uHeight),
      outer_width: toNumberOrUndefined(body.outerWidth),
      outer_depth: toNumberOrUndefined(body.outerDepth),
      desc_units: toBoolOrUndefined(body.descUnits),
    });
  }

  // InterfaceCreatePayload: device_id + name + type + enabled + description.
  if (entity === "Interfaces") {
    return compactPayload({
      device_id: body.deviceId,
      name: body.name,
      type: body.interfaceType,
      label: body.label,
      enabled: toBoolOrUndefined(body.enabled),
      mgmt_only: toBoolOrUndefined(body.mgmtOnly),
      description: body.description,
    });
  }

  // CableCreatePayload: status + type + label + physical attributes. Cables are
  // nameless on the wire, so the generic name field maps to `label`.
  if (entity === "Cables") {
    return compactPayload({
      status: body.status,
      type: body.interfaceType,
      label: body.name,
      color: toNetBoxColor(body.color),
      length: toNumberOrUndefined(body.length),
      length_unit: body.lengthUnit,
    });
  }
  return undefined;
}

/** Virtualization payloads: cluster types, cluster groups, clusters and VM
 *  interfaces. */
function toVirtualizationPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── Virtualization payloads ──

  if (entity === "Virtual Machines") {
    // VirtualMachinePayload is strict: do not forward generic inventory fields.
    // Compute/platform/type omission preserves updates; empty form values clear.
    return withCustomFieldsPayload(entity, body, compactPayload({
      name: body.name,
      site_id: body.siteId,
      cluster_id: body.clusterId,
      tenant_id: body.tenantId,
      role_id: body.roleId,
      platform_id: body.platformId === undefined ? undefined : body.platformId || null,
      virtual_machine_type_id: body.virtualMachineTypeId === undefined ? undefined : body.virtualMachineTypeId || null,
      status: body.status,
      primary_ip: body.primaryIp,
      vcpus: toVmComputeNumber(body.vcpus, "vcpus"),
      memory: toVmComputeNumber(body.memory, "memory"),
      disk: toNumberOrUndefined(body.disk),
      description: body.description,
      comments: body.comments,
    }));
  }

  if (entity === "Virtual Machine Types") {
    return compactPayload({
      name: body.name,
      slug: body.slug,
      default_platform_id: body.defaultPlatformId === undefined ? undefined : body.defaultPlatformId || null,
      default_vcpus: toVmComputeNumber(body.defaultVcpus, "default_vcpus"),
      default_memory: toVmComputeNumber(body.defaultMemory, "default_memory"),
      owner_id: body.ownerId === undefined ? undefined : body.ownerId || null,
      description: body.description,
      comments: body.comments,
      custom_fields: body.customFields,
    });
  }

  // ClusterTypePayload: name + slug + description.
  if (entity === "Cluster Types") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      description: body.description,
    });
  }

  // ClusterGroupPayload: name + slug + description.
  if (entity === "Cluster Groups") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      description: body.description,
    });
  }

  // ClusterPayload: name + cluster_type_id + cluster_group_id + site_id +
  // tenant_id + status + description + comments.
  if (entity === "Clusters") {
    return withCustomFieldsPayload(entity, body, compactPayload({
      name: body.name,
      cluster_type_id: body.clusterTypeId,
      cluster_group_id: body.clusterGroupId,
      site_id: body.siteId,
      tenant_id: body.tenantId,
      status: body.status,
      description: body.description,
      comments: body.comments,
    }));
  }

  // VmInterfaceCreatePayload: virtual_machine_id (required) + name + enabled +
  // description. mac_address/mtu/mode are in the schema but omitted per task
  // scope (not listed in the surfaced fields).
  if (entity === "VM Interfaces") {
    return compactPayload({
      virtual_machine_id: body.virtualMachineId ?? body.vmId,
      name: body.name,
      enabled: toBoolOrUndefined(body.enabled),
      mtu: toNumberOrUndefined(body.mtu),
      mode: body.mode,
      description: body.description,
    });
  }
  return undefined;
}

/** IPAM address payloads: VLANs, prefixes and IP addresses. */
function toIpamAddressPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // VlanCreatePayload: vid (required, int) + name + status + group_id + site_id +
  // tenant_id. role_id is int64 (NetBox numeric id) — omitted: no VlanRole tab
  // exists so a numeric input would be meaningless without a resolver.
  if (entity === "VLANs") {
    return compactPayload({
      vid: toNumberOrUndefined(body.vid),
      name: body.name,
      status: body.status,
      group_id: body.groupId,
      site_id: body.siteId,
      tenant_id: body.tenantId,
      description: body.description,
    });
  }

  // PrefixCreatePayload: prefix (required) + status + site_id + tenant_id +
  // is_pool. vrf_id and role_id are int64 (NetBox numeric ids) — omitted: no
  // VRF/IPAM-Role tabs exist so numeric input would be unresolvable in the UI.
  // vlan_id is a UUID reference but no VLAN FK selector is in the prefix form.
  if (entity === "Prefixes") {
    return compactPayload({
      prefix: body.prefix ?? body.name,
      status: body.status,
      site_id: body.siteId,
      vrf_id: body.vrfId,
      tenant_id: body.tenantId,
      is_pool: toBoolOrUndefined(body.isPool),
      mark_utilized: toBoolOrUndefined(body.markUtilized),
      description: body.description,
    });
  }

  // IpAddressCreatePayload: address (required) + interface_id (required) +
  // status + tenant_id + vrf_id + dns_name + description. role is response-only.
  if (entity === "IP Addresses") {
    return compactPayload({
      address: body.prefix ?? body.name,
      interface_id: body.interfaceId,
      status: body.status,
      tenant_id: body.tenantId,
      vrf_id: body.vrfId,
      dns_name: body.dnsName,
      description: body.description,
    });
  }
  return undefined;
}

/** DCIM rack payloads: rack roles, rack types and locations. */
function toDcimRackPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── DCIM payloads ──

  // RackRoleCreatePayload: name + slug + color + description.
  if (entity === "Rack Roles") {
    return withIconPayload(entity, body, {
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      color: toNetBoxColor(body.color),
      description: body.description,
    });
  }

  // RackTypeCreatePayload: manufacturer_id (required) + model (required) + slug +
  // form_factor + width + u_height + weight + description.
  if (entity === "Rack Types") {
    const model = body.model ?? body.name;
    return compactPayload({
      manufacturer_id: body.manufacturerId,
      model,
      slug: model ? slugify(model) : undefined,
      form_factor: body.formFactor,
      width: toNumberOrUndefined(body.width),
      u_height: toNumberOrUndefined(body.uHeight),
      starting_unit: toNumberOrUndefined(body.startingUnit),
      desc_units: toBoolOrUndefined(body.descUnits),
      outer_width: toNumberOrUndefined(body.outerWidth),
      outer_depth: toNumberOrUndefined(body.outerDepth),
      outer_height: toNumberOrUndefined(body.outerHeight),
      outer_unit: body.outerUnit,
      mounting_depth: toNumberOrUndefined(body.mountingDepth),
      weight: toNumberOrUndefined(body.weight),
      max_weight: toNumberOrUndefined(body.maxWeight),
      weight_unit: body.weightUnit,
      description: body.description,
    });
  }

  // LocationCreatePayload: name (required) + site_id (required) + slug +
  // status + parent_id + tenant_id + facility + description.
  if (entity === "Locations") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      site_id: body.siteId,
      status: body.status,
      parent_id: body.parentId,
      tenant_id: body.tenantId,
      facility: body.facility,
      description: body.description,
    });
  }
  return undefined;
}

/** IPAM registry payloads: RIRs, VRFs, route targets, aggregates, ASNs, IP
 *  ranges and services. */
function toIpamRegistryPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── IPAM registry payloads (RIRs, VRFs, Route Targets, Aggregates, ASNs, IP
  // Ranges, Services) and the DCIM/virtualization component payloads below
  // (Modules, Module Types, Virtual Chassis, Virtual Disks) — each emits only the
  // fields its backend create/update payload schema accepts (derived from
  // schema.gen.ts), coercing string-typed form values to the numeric/boolean/array
  // wire types. ──

  // RirCreatePayload: name + slug + is_private + description.
  if (entity === "RIRs") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      is_private: toBoolOrUndefined(body.isPrivate),
      description: body.description,
    });
  }

  // VrfCreatePayload: name + rd + tenant_id + enforce_unique + description + comments.
  if (entity === "VRFs") {
    return compactPayload({
      name: body.name,
      rd: body.rd,
      tenant_id: body.tenantId,
      enforce_unique: toBoolOrUndefined(body.enforceUnique),
      description: body.description,
      comments: body.comments,
    });
  }

  // RouteTargetCreatePayload: name + tenant_id + description.
  if (entity === "Route Targets") {
    return compactPayload({
      name: body.name,
      tenant_id: body.tenantId,
      description: body.description,
    });
  }

  // AggregateCreatePayload: prefix (required) + rir_id (required) + tenant_id +
  // date_added + description + comments. Nameless — generic name field maps to prefix.
  if (entity === "Aggregates") {
    return compactPayload({
      prefix: body.prefixCidr ?? body.name,
      rir_id: body.rirId,
      tenant_id: body.tenantId,
      date_added: body.dateAdded,
      description: body.description,
      comments: body.comments,
    });
  }

  // AsnCreatePayload: asn (required int) + rir_id (required) + tenant_id +
  // description + comments. Nameless — generic name field maps to asn.
  if (entity === "ASNs") {
    return compactPayload({
      asn: toNumberOrUndefined(body.asn ?? body.name),
      rir_id: body.rirId,
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }

  // IpRangeCreatePayload: start_address + end_address (both required) + status +
  // tenant_id + vrf_id + description.
  if (entity === "IP Ranges") {
    return compactPayload({
      start_address: body.startAddress,
      end_address: body.endAddress,
      status: body.status,
      tenant_id: body.tenantId,
      vrf_id: body.vrfId,
      mark_utilized: toBoolOrUndefined(body.markUtilized),
      mark_populated: toBoolOrUndefined(body.markPopulated),
      description: body.description,
    });
  }

  // ServiceCreatePayload: name + protocol + ports (int[]) + parent_object
  // (nested ObjectRefInput, required) + description. ports is a comma-separated
  // form string.
  if (entity === "Services") {
    return compactPayload({
      name: body.name,
      protocol: body.protocol,
      ports: toNumberArray(body.ports),
      ipaddresses: body.serviceIpAddressIds,
      parent_object: toObjectRefInput(body.parentObject),
      description: body.description,
    });
  }
  return undefined;
}

/** DCIM/virtualization component payloads: modules, module types, virtual
 *  chassis and virtual disks. */
function toDcimComponentPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ModuleCreatePayload: device_id + module_type_id + module_bay_id (all required) +
  // status + serial + asset_tag + description.
  if (entity === "Modules") {
    return compactPayload({
      device_id: body.deviceId,
      module_type_id: body.moduleTypeId,
      module_bay_id: body.moduleBayId,
      status: body.status,
      serial: body.serialNumber,
      asset_tag: body.assetTagModule,
      description: body.description,
    });
  }

  // ModuleTypePayload: manufacturer_id + model. Model captured via generic name field.
  if (entity === "Module Types") {
    const model = body.model ?? body.name;
    return compactPayload({
      manufacturer_id: body.manufacturerId,
      model,
    });
  }

  // VirtualChassisCreatePayload: name (required) + domain + master_id + description +
  // comments.
  if (entity === "Virtual Chassis") {
    return compactPayload({
      name: body.name,
      domain: body.domain,
      master_id: body.masterId,
      description: body.description,
      comments: body.comments,
    });
  }

  // VirtualDiskCreatePayload: virtual_machine_id (required) + name (required) +
  // size (required int) + description.
  if (entity === "Virtual Disks") {
    return compactPayload({
      virtual_machine_id: body.virtualMachineId,
      name: body.name,
      size: toNumberOrUndefined(body.size),
      description: body.description,
    });
  }
  return undefined;
}

/** Circuits payloads: providers, circuit types, provider networks, circuits and
 *  ASN ranges. */
function toCircuitsPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── Circuits payloads (Providers, Circuit Types, Provider Networks, Circuits,
  // ASN Ranges) and the inventory item payloads below (Inventory Item Roles,
  // Inventory Items). ──

  // ProviderCreatePayload: name + slug + description + comments.
  if (entity === "Providers") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      description: body.description,
      comments: body.comments,
    });
  }

  // CircuitTypeCreatePayload: name + slug + color + description.
  if (entity === "Circuit Types") {
    return withIconPayload(entity, body, {
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      color: toNetBoxColor(body.color),
      description: body.description,
    });
  }

  // ProviderNetworkCreatePayload: provider_id (required) + name + service_id +
  // description + comments.
  if (entity === "Provider Networks") {
    return compactPayload({
      provider_id: body.providerId,
      name: body.name,
      service_id: body.serviceId,
      description: body.description,
      comments: body.comments,
    });
  }

  // CircuitCreatePayload: cid (required) + provider_id (required) + type_id
  // (required) + status + tenant_id + description + comments + scalars.
  // Nameless — cid captured via generic name field.
  if (entity === "Circuits") {
    return compactPayload({
      cid: body.cid ?? body.name,
      provider_id: body.providerId,
      type_id: body.circuitTypeId,
      status: body.status,
      tenant_id: body.tenantId,
      provider_account_id: body.providerAccountId,
      termination_a_id: body.terminationAId,
      termination_z_id: body.terminationZId,
      install_date: body.installDate,
      termination_date: body.terminationDate,
      commit_rate: toNumberOrUndefined(body.commitRate),
      distance: toNumberOrUndefined(body.distance),
      distance_unit: body.distanceUnit,
      description: body.description,
      comments: body.comments,
    });
  }

  // AsnRangeCreatePayload: name + slug + rir_id (required) + start (required int) +
  // end (required int) + tenant_id + description + comments.
  if (entity === "ASN Ranges") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      rir_id: body.rirId,
      start: toNumberOrUndefined(body.asnStart),
      end: toNumberOrUndefined(body.asnEnd),
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }
  return undefined;
}

/** Inventory item payloads: item roles and items (the item payload's `device_id`
 *  is create-only, gated on `mode`). */
function toInventoryItemPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
  mode: "create" | "update",
): Record<string, unknown> | undefined {
  // InventoryItemRoleCreatePayload: name + slug + color + description.
  if (entity === "Inventory Item Roles") {
    return withIconPayload(entity, body, {
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      color: toNetBoxColor(body.color),
      description: body.description,
    });
  }

  // InventoryItemCreatePayload / InventoryItemUpdatePayload: device_id is
  // create-only (the update schema has no such field and denies unknown keys);
  // everything else is shared. `discovered` is a bool on the wire.
  if (entity === "Inventory Items") {
    return compactPayload({
      ...(mode === "create" ? { device_id: body.deviceId } : {}),
      parent_id: body.parentId,
      role_id: body.roleId,
      manufacturer_id: body.manufacturerId,
      name: body.name,
      label: body.label,
      part_id: body.partId,
      serial: body.serial,
      asset_tag: body.assetTag,
      discovered: toBoolOrUndefined(body.discovered),
      description: body.description,
      status: body.status,
      component: toObjectRefInput(body.component),
    });
  }
  return undefined;
}

/** VPN payloads: tunnels, tunnel groups/terminations and the IKE/IPsec policy
 *  families. */
function toVpnPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── VPN payloads ──

  // TunnelGroupCreatePayload: name + slug + description + comments.
  if (entity === "Tunnel Groups") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      description: body.description,
      comments: body.comments,
    });
  }

  // TunnelCreatePayload: name + status + encapsulation + group_id + ipsec_profile_id +
  // tunnel_id (int) + tenant_id + description + comments.
  if (entity === "Tunnels") {
    return compactPayload({
      name: body.name,
      status: body.status,
      encapsulation: body.encapsulation,
      group_id: body.tunnelGroupId,
      ipsec_profile_id: body.ipsecProfileId,
      tunnel_id: toNumberOrUndefined(body.tunnelId),
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }

  // TunnelTerminationPayload: role + tunnel_id (FK, required) + termination
  // (nested ObjectRefInput, optional) + outside_ip_id.
  if (entity === "Tunnel Terminations") {
    return compactPayload({
      role: body.vpnRole,
      tunnel_id: body.tunnelFkId,
      termination: toObjectRefInput(body.termination),
      outside_ip_id: body.outsideIpId,
    });
  }

  // IkePolicyCreatePayload: name + version (int) + mode + preshared_key +
  // description + comments.
  if (entity === "IKE Policies") {
    return compactPayload({
      name: body.name,
      version: toNumberOrUndefined(body.vpnVersion),
      mode: body.vpnMode,
      preshared_key: body.presharedKey,
      description: body.description,
      comments: body.comments,
    });
  }

  // IkeProposalCreatePayload: name + authentication_method + encryption_algorithm +
  // authentication_algorithm + group (int) + sa_lifetime (int) + description + comments.
  if (entity === "IKE Proposals") {
    return compactPayload({
      name: body.name,
      authentication_method: body.authenticationMethod,
      encryption_algorithm: body.encryptionAlgorithm,
      authentication_algorithm: body.authenticationAlgorithm,
      group: toNumberOrUndefined(body.pfsGroup),
      sa_lifetime: toNumberOrUndefined(body.saLifetime),
      description: body.description,
      comments: body.comments,
    });
  }

  // IpsecPolicyCreatePayload: name + pfs_group (int) + description + comments.
  if (entity === "IPsec Policies") {
    return compactPayload({
      name: body.name,
      pfs_group: toNumberOrUndefined(body.pfsGroup),
      description: body.description,
      comments: body.comments,
    });
  }

  // IpsecProposalCreatePayload: name + encryption_algorithm + authentication_algorithm +
  // sa_lifetime_seconds (int) + sa_lifetime_data (int) + description + comments.
  if (entity === "IPsec Proposals") {
    return compactPayload({
      name: body.name,
      encryption_algorithm: body.encryptionAlgorithm,
      authentication_algorithm: body.authenticationAlgorithm,
      sa_lifetime_seconds: toNumberOrUndefined(body.saLifetimeSeconds),
      sa_lifetime_data: toNumberOrUndefined(body.saLifetimeData),
      description: body.description,
      comments: body.comments,
    });
  }

  // IpsecProfileCreatePayload: name + mode + ike_policy_id (required) +
  // ipsec_policy_id (required) + description + comments.
  if (entity === "IPsec Profiles") {
    return compactPayload({
      name: body.name,
      mode: body.vpnMode,
      ike_policy_id: body.ikePolicyId,
      ipsec_policy_id: body.ipsecPolicyId,
      description: body.description,
      comments: body.comments,
    });
  }
  return undefined;
}

/** Circuit terminations payloads: circuit groups, provider accounts, circuit
 *  terminations and virtual circuits. */
function toCircuitTerminationPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // ── Circuits remainder payloads ──

  if (entity === "Circuit Groups") {
    return compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Provider Accounts") {
    return compactPayload({
      account: body.account ?? body.name,
      name: body.name,
      provider_id: body.providerId,
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Circuit Terminations") {
    return compactPayload({
      term_side: body.termSide,
      circuit_id: body.circuitId,
      xconnect_id: body.xconnectId,
      pp_info: body.ppInfo,
      port_speed: toNumberOrUndefined(body.portSpeed),
      upstream_speed: toNumberOrUndefined(body.upstreamSpeed),
      mark_connected: toBoolOrUndefined(body.markConnected),
      description: body.description,
    });
  }

  if (entity === "Virtual Circuit Types") {
    return withIconPayload(entity, body, {
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      color: toNetBoxColor(body.color),
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Virtual Circuits") {
    return compactPayload({
      cid: body.cid ?? body.name,
      status: body.status,
      provider_network_id: body.providerNetworkId,
      type_id: body.virtualCircuitTypeId,
      provider_account_id: body.providerAccountId,
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }
  return undefined;
}

/** Power payloads: power panels, power feeds and rack reservations. */
function toPowerPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  if (entity === "Power Panels") {
    return compactPayload({
      name: body.name,
      site_id: body.siteId,
      location_id: body.parentId,
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Power Feeds") {
    return compactPayload({
      name: body.name,
      status: body.status,
      type: body.rackType,
      supply: body.supply,
      phase: body.phase,
      voltage: toNumberOrUndefined(body.voltage),
      amperage: toNumberOrUndefined(body.amperage),
      max_utilization: toNumberOrUndefined(body.maxUtilization),
      available_power: toNumberOrUndefined(body.availablePower),
      power_panel_id: body.powerPanelId,
      rack_id: body.rackId,
      mark_connected: toBoolOrUndefined(body.markConnected),
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Rack Reservations") {
    return compactPayload({
      units: toNumberArray(body.units),
      rack_id: body.rackId,
      user_id: body.userId,
      status: body.status,
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }
  return undefined;
}

/** Form-field expansion payloads: MAC addresses, virtual device contexts, service
 *  templates, sites and device roles. */
function toExpandedFormPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> | undefined {
  // MacAddressPayload: mac_address + assigned_object (nested ObjectRefInput,
  // optional) + description + comments.
  if (entity === "MAC Addresses") {
    return compactPayload({
      mac_address: body.macAddress ?? body.name,
      assigned_object: toObjectRefInput(body.assignedObject),
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Virtual Device Contexts") {
    return compactPayload({
      name: body.name,
      status: body.status,
      identifier: toNumberOrUndefined(body.identifier),
      device_id: body.deviceId,
      primary_ip4_id: body.primaryIp4Id,
      primary_ip6_id: body.primaryIp6Id,
      tenant_id: body.tenantId,
      description: body.description,
      comments: body.comments,
    });
  }

  if (entity === "Service Templates") {
    return compactPayload({
      name: body.name,
      protocol: body.protocol,
      ports: toNumberArray(body.ports),
      description: body.description,
      comments: body.comments,
    });
  }

  // ── Form field expansion payloads ──

  // SitePayload: name + slug + status + tenant_id + region_id + site_group_id +
  // facility + time_zone + physical_address + shipping_address + latitude +
  // longitude + description + comments.
  if (entity === "Sites") {
    return withCustomFieldsPayload(entity, body, compactPayload({
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      status: body.status,
      tenant_id: body.tenantId,
      region_id: body.regionId,
      site_group_id: body.siteGroupId,
      facility: body.facility,
      time_zone: body.timeZone,
      physical_address: body.physicalAddress,
      shipping_address: body.shippingAddress,
      latitude: toNumberOrUndefined(body.latitude),
      longitude: toNumberOrUndefined(body.longitude),
      description: body.description,
      comments: body.comments,
    }));
  }

  // DeviceRolePayload: name + slug + color + vm_role + description.
  if (entity === "Device Roles") {
    return withIconPayload(entity, body, {
      name: body.name,
      slug: body.name ? slugify(body.name) : undefined,
      color: toNetBoxColor(body.color),
      vm_role: toBoolOrUndefined(body.vmRole),
      description: body.description,
    });
  }
  return undefined;
}

/** Fallback payload for entities with no bespoke shape: the generic fields the
 *  backend accepts, plus the Contacts scalar swap and conditional status. */
function toGenericInventoryPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    name: body.name,
    slug: body.name ? slugify(body.name) : undefined,
    description: body.description,
    comments: body.comments,
    site_id: body.siteId,
    tenant_id: body.tenantId,
    region_id: body.regionId,
    role_id: body.roleId,
    platform_id: body.platformId,
    device_type_id: body.deviceTypeId,
    cluster_id: body.clusterId,
    rack_id: body.rackId,
    site_group_id: body.siteGroupId,
    device_id: body.deviceId,
    vm_id: body.vmId,
    parent_id: body.parentId,
    group_id: body.groupId,
    primary_ip: body.primaryIp,
    serial: body.serial,
    asset_tag: body.assetTag,
    position: toNumberOrUndefined(body.position),
    face: body.face,
    airflow: body.airflow,
    vcpus: toNumberOrUndefined(body.vcpus),
    memory: toNumberOrUndefined(body.memory),
    disk: toNumberOrUndefined(body.disk),
  };
  // Contacts have no slug; they carry scalar contact attributes instead.
  if (entity === "Contacts") {
    delete payload.slug;
    delete payload.primary_ip;
    payload.title = body.title;
    payload.phone = body.phone;
    payload.email = body.email;
    payload.address = body.address;
    payload.link = body.link;
  }
  if (inventoryEntityHasStatus(entity)) {
    payload.status = body.status;
  }
  for (const k of Object.keys(payload)) if (payload[k] === undefined) delete payload[k];
  return withCustomFieldsPayload(entity, body, payload);
}

function toOwnerPayload(entity: InventoryEntityKey, body: Partial<Resource>): Record<string, unknown> | undefined {
  if (entity !== "Owners" && entity !== "Owner Groups") return undefined;
  // AdminModel whitelist: never send slug, status, tags, custom fields or dates.
  const payload: Record<string, unknown> = { name: body.name, description: body.description };
  if (entity === "Owners") {
    payload.group_id = body.groupId;
    payload.users = body.users;
    payload.user_groups = body.userGroups;
  }
  return Object.fromEntries(Object.entries(payload).filter(([, value]) => value !== undefined));
}

const ENTITY_PAYLOAD_BUILDERS: readonly EntityPayloadBuilder[] = [
  toOwnerPayload,
  toContactAssignmentPayload,
  toDcimPayload,
  toVirtualizationPayload,
  toIpamAddressPayload,
  toDcimRackPayload,
  toIpamRegistryPayload,
  toDcimComponentPayload,
  toCircuitsPayload,
  toInventoryItemPayload,
  toVpnPayload,
  toCircuitTerminationPayload,
  toPowerPayload,
  toExpandedFormPayload,
];

/** Convert UI-shape Resource to the snake_case FK-UUID payload the backend expects.
 *  `mode` distinguishes create from update where the two payload schemas differ
 *  (e.g. Inventory Items: `device_id` is create-only, rejected on update). */
function toBackendPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
  mode: "create" | "update" = "create",
): Record<string, unknown> {
  for (const build of ENTITY_PAYLOAD_BUILDERS) {
    const payload = build(entity, body, mode);
    if (payload !== undefined) return payload;
  }
  return toGenericInventoryPayload(entity, body);
}

/**
 * The inventory entities that carry per-definition custom fields, mapped to the
 * reveal-endpoint path segment. Kept in lockstep with the backend
 * `InventoryEntityKind`.
 */
const CUSTOM_FIELD_ENTITY_KIND: Partial<Record<InventoryEntityKey, CustomFieldEntityKind>> = {
  Sites: "sites",
  Devices: "devices",
  Regions: "regions",
  Clusters: "clusters",
  Tenants: "tenants",
  "Virtual Machines": "virtual-machines",
  "Virtual Machine Types": "virtual-machine-types",
};

/** Reveal-endpoint entity kind for a custom-field-bearing inventory entity. */
export function customFieldEntityKind(
  entity: InventoryEntityKey,
): CustomFieldEntityKind | undefined {
  return CUSTOM_FIELD_ENTITY_KIND[entity];
}

/**
 * Forward an entity's `custom_fields` map onto its update/create payload for the
 * three custom-field-bearing entities. A present map is a wholesale replace of
 * the entity's plaintext custom-field data (backend `COALESCE($n, …)` semantics)
 * — the caller must therefore pass the FULL current non-secret value set plus
 * only the edited secret values (never the `"****"` mask). An omitted map
 * (`undefined`) leaves stored custom fields untouched.
 */
function withCustomFieldsPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
  payload: Record<string, unknown>,
): Record<string, unknown> {
  if (!CUSTOM_FIELD_ENTITY_KIND[entity]) return payload;
  if (body.customFields !== undefined) payload.custom_fields = body.customFields;
  return withLocalContextDataPayload(entity, body, payload);
}

/**
 * Forward a Device/VM `local_context_data` JSON onto its create/update payload.
 * Only Devices and Virtual Machines carry a local config context override. An
 * omitted value (`undefined`) leaves the stored override untouched; an explicit
 * `null` clears it.
 */
function withLocalContextDataPayload(
  entity: InventoryEntityKey,
  body: Partial<Resource>,
  payload: Record<string, unknown>,
): Record<string, unknown> {
  if (entity !== "Devices" && entity !== "Virtual Machines") return payload;
  if (body.localContextData !== undefined) payload.local_context_data = body.localContextData;
  return payload;
}

const CYRILLIC_SLUG_MAP: Record<string, string> = {
  а: "a",
  б: "b",
  в: "v",
  г: "g",
  д: "d",
  е: "e",
  ё: "e",
  ж: "zh",
  з: "z",
  и: "i",
  й: "y",
  к: "k",
  л: "l",
  м: "m",
  н: "n",
  о: "o",
  п: "p",
  р: "r",
  с: "s",
  т: "t",
  у: "u",
  ф: "f",
  х: "h",
  ц: "ts",
  ч: "ch",
  ш: "sh",
  щ: "sch",
  ъ: "",
  ы: "y",
  ь: "",
  э: "e",
  ю: "yu",
  я: "ya",
};

export function slugify(s: string): string {
  const transliterated = s
    .toLowerCase()
    .trim()
    .replace(/[а-яё]/gu, (char) => CYRILLIC_SLUG_MAP[char] ?? "");
  return transliterated.replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "item";
}

export async function createResource(
  entity: InventoryEntityKey,
  body: Omit<Resource, "id">,
): Promise<Resource> {
  const api = ENTITY_API[entity];
  if (!api.create) throw new Error(`${entity} is read-only and cannot be created`);
  const raw = await api.create(toBackendPayload(entity, body, "create"));
  return toResource(entity, raw);
}

export async function updateResource(
  entity: InventoryEntityKey,
  id: string,
  body: Partial<Resource>,
): Promise<Resource> {
  const apiUpdate = ENTITY_API[entity];
  if (!apiUpdate.update) throw new Error(`${entity} is read-only and cannot be updated`);
  const raw = await apiUpdate.update(id, toBackendPayload(entity, body, "update"));
  return toResource(entity, raw);
}

export async function deleteResource(
  entity: InventoryEntityKey,
  id: string,
): Promise<void> {
  const api = ENTITY_API[entity];
  if (!api.delete) throw new Error(`${entity} is read-only and cannot be deleted`);
  await api.delete(id);
}

/**
 * Contact assignments scoped to a single target entity UUID, via the typed
 * `object_id` contract filter. Rows are normalized through the shared
 * `toResource` projection so callers read the camelCase
 * `contactId`/`roleId`/`objectId` shape like every other inventory record.
 */
export async function listContactAssignmentsByObject(objectId: string): Promise<Resource[]> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/inventory/contact-assignments", {
      params: { query: { object_id: objectId } },
    }),
  );
  return asBackendInventoryEnvelope(raw).items.map((r) => toResource("Contact Assignments", r));
}

/**
 * Create a contact assignment against the typed contract. `object_type` is the
 * canonical NetBox `app_label.model` token (resolved to a content-type UUID
 * server-side); `object_id` is the target entity UUID. Distinct from the generic
 * inventory `createResource` path, whose Contact Assignments branch predates this
 * contract shape.
 */
export async function createContactAssignment(payload: {
  objectType: string;
  objectId: string;
  contactId: string;
  roleId: string;
  priority?: string;
}): Promise<Resource> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/inventory/contact-assignments", {
      body: {
        object_type: payload.objectType,
        object_id: payload.objectId,
        contact_id: payload.contactId,
        role_id: payload.roleId,
        ...(payload.priority ? { priority: payload.priority } : {}),
      },
    }),
  );
  return toResource("Contact Assignments", asBackendInventoryRecord(raw));
}

/** Outcome of a client fan-out bulk update: ids that succeeded and the ones
 * that failed with their reason. */
export interface BulkUpdateResult {
  updated: string[];
  failed: { id: string; reason: string }[];
}

/** A bulk-edit patch. Set fields carry a value; cleared FK/textarea fields
 * carry `null`, which `toBackendPayload`/`compactPayload` preserve (they only
 * strip `undefined`) so the backend receives an explicit clear. */
export type BulkUpdatePatch = Partial<Record<keyof Resource, string | null>>;

/** Applies the same `patch` to every id via individual `updateResource` calls
 * (client fan-out — there is no backend /bulk endpoint).
 *
 * Native Owners and VM types use PATCH and send only changed fields, without a
 * read/merge step. Other inventory update endpoints use full-object PUTs (the backend rejects a
 * partial body with 422 `missing field 'name'`), so we cannot send the patch
 * alone. For each id we read the current record, merge the patch over it, and
 * PUT the complete object — mirroring what the single-edit form does. `null`
 * patch values survive the merge and `toBackendPayload` to clear nullable
 * FK/textarea columns; untouched fields keep their current values. */
export async function bulkUpdateResources(
  entity: InventoryEntityKey,
  ids: string[],
  patch: BulkUpdatePatch,
): Promise<BulkUpdateResult> {
  const results = await Promise.allSettled(
    ids.map(async (id) => {
      if (entity === "Owners" || entity === "Owner Groups") {
        const { name, description, groupId } = patch;
        if (name === null) throw new Error("Name cannot be cleared");
        // Scalar bulk edits cannot replace identity collections.
        return updateResource(entity, id, { name, description: description === null ? "" : description, groupId });
      }
      if (entity === "Virtual Machine Types") {
        if (patch.name === null || patch.slug === null) throw new Error("Name and slug cannot be cleared");
        const changed = Object.fromEntries(Object.entries(patch).map(([key, value]) => [key, value === null ? "" : value]));
        return updateResource(entity, id, changed);
      }
      const current = await getResource(entity, id);
      if (entity === "Virtual Machines") {
        if (patch.name === null) throw new Error("Name cannot be cleared");
        const changed = Object.fromEntries(Object.entries(patch).map(([key, value]) => [key, value === null ? "" : value]));
        const merged = { ...current, ...changed };
        // Bulk scalar edits cannot replace the masked custom-field read view.
        delete merged.customFields;
        for (const key of ["platformId", "virtualMachineTypeId", "vcpus", "memory"] as const) {
          if (!(key in patch)) delete merged[key];
        }
        return updateResource(entity, id, merged);
      }
      const merged = { ...current, ...patch } as Partial<Resource>;
      return updateResource(entity, id, merged);
    }),
  );
  const updated: string[] = [];
  const failed: { id: string; reason: string }[] = [];
  results.forEach((r, idx) => {
    const id = ids[idx] as string;
    if (r.status === "fulfilled") {
      updated.push(id);
    } else {
      const reason = r.reason instanceof Error ? r.reason.message : String(r.reason);
      failed.push({ id, reason });
    }
  });
  return { updated, failed };
}

// ── Device types (lookup for connection-profile scope resolution) ──

// ── Templates (backend: /api/v1/inventory/templates) ─────

type BackendInventoryTemplate = components["schemas"]["InventoryTemplateResponse"];
type BackendTemplateList = components["schemas"]["InventoryTemplateListResponse"];
type BackendTemplateRun = components["schemas"]["InventoryTemplateRunResponse"];
type BackendTemplateRunList = components["schemas"]["InventoryTemplateRunListResponse"];

function isObject(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function toTemplateVariable(raw: unknown): TemplateVariable | null {
  if (!isObject(raw) || typeof raw.key !== "string" || typeof raw.label !== "string") return null;
  if (typeof raw.value_type !== "string") return null;

  return {
    key: raw.key,
    label: raw.label,
    description: typeof raw.description === "string" ? raw.description : undefined,
    value_type: raw.value_type as TemplateVariable["value_type"],
    required: typeof raw.required === "boolean" ? raw.required : false,
    default_value: raw.default_value,
    validation_rules: isObject(raw.validation_rules) ? raw.validation_rules : {},
    sort_order: typeof raw.sort_order === "number" ? raw.sort_order : undefined,
  };
}

function toTemplateStep(raw: unknown): TemplateStep | null {
  if (!isObject(raw) || typeof raw.step_key !== "string" || typeof raw.resource_type !== "string") {
    return null;
  }

  return {
    step_key: raw.step_key,
    title: typeof raw.title === "string" ? raw.title : raw.step_key,
    description: typeof raw.description === "string" ? raw.description : undefined,
    resource_type: raw.resource_type as TemplateStep["resource_type"],
    operation: "create",
    depends_on_step_keys: Array.isArray(raw.depends_on_step_keys)
      ? raw.depends_on_step_keys.filter((value): value is string => typeof value === "string")
      : [],
    payload_template: isObject(raw.payload_template) ? raw.payload_template : {},
    repeat: isObject(raw.repeat) ? raw.repeat : {},
    on_conflict_policy:
      raw.on_conflict_policy === "skip" || raw.on_conflict_policy === "update_selected_fields"
        ? raw.on_conflict_policy
        : "fail",
    sort_order: typeof raw.sort_order === "number" ? raw.sort_order : undefined,
  };
}

function toInventoryTemplate(raw: BackendInventoryTemplate): InventoryTemplate {
  return {
    id: raw.id,
    is_system: typeof raw.is_system === "boolean" ? raw.is_system : undefined,
    name: raw.name,
    slug: raw.slug ?? raw.id,
    status: (raw.status as InventoryTemplate["status"]) ?? "draft",
    description: raw.description ?? "",
    variables: Array.isArray(raw.variables)
      ? raw.variables
          .map(toTemplateVariable)
          .filter((value): value is TemplateVariable => value !== null)
      : [],
    steps: Array.isArray(raw.steps)
      ? raw.steps
          .map(toTemplateStep)
          .filter((value): value is TemplateStep => value !== null)
      : [],
    updatedAt: raw.updated_at ?? "",
  };
}

function toTemplatePayload(body: Partial<InventoryTemplate>): Record<string, unknown> {
  return {
    name: body.name,
    slug: body.slug,
    description: body.description,
    status: body.status,
    variables: body.variables ?? [],
    steps: body.steps ?? [],
  };
}

export interface TemplateListQuery extends ListQuery {
  status?: string;
  order?: "asc" | "desc";
}

export async function listTemplates(q?: TemplateListQuery): Promise<Paginated<InventoryTemplate>> {
  const raw: BackendTemplateList = unwrap(
    await apiClient.GET("/api/v1/inventory/templates", {
      params: {
        query: {
          search: q?.search,
          status: q?.status,
          sort: q?.sort,
          order: q?.order,
          limit: q?.limit,
          offset: q?.offset,
        },
      },
    }),
  );
  return {
    items: (raw.items ?? []).map(toInventoryTemplate),
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

/** GET /api/v1/inventory/templates/{id} — single template (for the editor). */
export async function getTemplate(id: string): Promise<InventoryTemplate> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/inventory/templates/{id}", {
      params: { path: { id } },
    }),
  );
  return toInventoryTemplate(raw);
}

export async function createTemplate(
  body: Omit<InventoryTemplate, "id" | "updatedAt">,
): Promise<InventoryTemplate> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/inventory/templates", {
      body: toTemplatePayload(body) as components["schemas"]["InventoryTemplatePayload"],
    }),
  );
  return toInventoryTemplate(raw);
}

export async function updateTemplate(
  id: string,
  body: Partial<InventoryTemplate>,
): Promise<InventoryTemplate> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/inventory/templates/{id}", {
      params: { path: { id } },
      body: toTemplatePayload(body) as components["schemas"]["InventoryTemplatePayload"],
    }),
  );
  return toInventoryTemplate(raw);
}

export async function deleteTemplate(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/inventory/templates/{id}", {
    params: { path: { id } },
  });
}

/** POST /api/v1/inventory/templates/{id}/runs — returns the run record id, which
 *  the UI surfaces as the job identifier. */
export async function runTemplate(
  id: string,
  variables?: Record<string, unknown>,
): Promise<TemplateRunResponse> {
  const raw: BackendTemplateRun = unwrap(
    await apiClient.POST("/api/v1/inventory/templates/{id}/runs", {
      params: { path: { id } },
      body: { variables: variables ?? {} },
    }),
  );
  const summary = raw.summary as
    | { total_steps: number; succeeded_steps: number; failed_steps: number; skipped_steps: number }
    | null
    | undefined;
  return {
    id: raw.id,
    templateId: raw.template_id,
    status: raw.status,
    startedAt: raw.started_at ?? undefined,
    finishedAt: raw.finished_at ?? undefined,
    summary: summary ?? undefined,
  };
}

type BackendTemplateRunStep = components["schemas"]["InventoryTemplateRunStepResponse"];

export interface TemplateRunListQuery extends ListQuery {
  status?: string;
  template_id?: string;
}

function toTemplateRunResponse(raw: BackendTemplateRun, templateName?: string): TemplateRunResponse {
  const summary = raw.summary as
    | { total_steps: number; succeeded_steps: number; failed_steps: number; skipped_steps: number }
    | null
    | undefined;
  return {
    id: raw.id,
    templateId: raw.template_id,
    status: raw.status,
    startedAt: raw.started_at ?? undefined,
    finishedAt: raw.finished_at ?? undefined,
    summary: summary ?? undefined,
    templateName,
  };
}

export async function listTemplateRuns(
  query: TemplateRunListQuery = {},
): Promise<Paginated<TemplateRunResponse>> {
  const raw: BackendTemplateRunList = unwrap(
    await apiClient.GET("/api/v1/inventory/template-runs", {
      params: { query },
    }),
  );
  return {
    items: raw.items.map((run) => toTemplateRunResponse(run)),
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

/** GET /api/v1/inventory/templates/{id}/runs — lists runs for one template. */
export async function listTemplateRunsForTemplate(
  templateId: string,
  limit = 10,
): Promise<TemplateRunResponse[]> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/inventory/templates/{id}/runs", {
      params: { path: { id: templateId }, query: { limit, offset: 0 } },
    }),
  ) as BackendTemplateRun[];
  return raw.map((r) => toTemplateRunResponse(r));
}

/** A single resolved step row returned by the dry-run preview endpoint. */
export interface TemplatePreviewStep {
  step_key: string;
  title: string;
  resource_type: TemplateStep["resource_type"];
  payload: Record<string, unknown>;
}

export interface TemplatePreviewResult {
  templateId: string;
  steps: TemplatePreviewStep[];
  variables: Record<string, unknown>;
}

/**
 * POST /api/v1/inventory/templates/{id}/preview-run — server-side dry run that
 * resolves placeholders (`{{var}}`, `{{steps.<key>.id}}`) against the supplied
 * variables and returns the payloads that *would* be created, without writing
 * anything. The run wizard previews the persisted template before execution.
 */
export async function previewTemplateRun(
  id: string,
  variables?: Record<string, unknown>,
): Promise<TemplatePreviewResult> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/inventory/templates/{id}/preview-run", {
      params: { path: { id } },
      body: { variables: variables ?? {} },
    }),
  );
  const rawSteps = Array.isArray(raw.steps) ? raw.steps : [];
  const steps: TemplatePreviewStep[] = rawSteps
    .map((entry): TemplatePreviewStep | null => {
      if (!isObject(entry) || typeof entry.step_key !== "string") return null;
      return {
        step_key: entry.step_key,
        title: typeof entry.title === "string" ? entry.title : entry.step_key,
        resource_type: entry.resource_type as TemplateStep["resource_type"],
        payload: isObject(entry.payload) ? entry.payload : {},
      };
    })
    .filter((value): value is TemplatePreviewStep => value !== null);
  return {
    templateId: raw.template_id,
    steps,
    variables: isObject(raw.variables) ? raw.variables : {},
  };
}

/** GET /api/v1/inventory/template-runs/{id}/steps */
export async function listRunSteps(runId: string): Promise<TemplateRunStep[]> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/inventory/template-runs/{id}/steps", {
      params: { path: { id: runId } },
    }),
  ) as BackendTemplateRunStep[];
  return raw.map((s) => ({
    id: s.id,
    runId: s.run_id,
    stepKey: s.step_key,
    status: s.status,
    createdResourceIds: Array.isArray(s.created_resource_ids)
      ? (s.created_resource_ids as string[])
      : [],
    resolvedPayload: (s.resolved_payload as Record<string, unknown>) ?? {},
    errorCode: s.error_code ?? undefined,
    errorMessage: s.error_message ?? undefined,
    durationMs: s.duration_ms ?? undefined,
    createdAt: s.created_at,
  }));
}
