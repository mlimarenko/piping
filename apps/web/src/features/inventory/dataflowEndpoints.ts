import type { InventoryEntityKey } from "@/shared/api/types";
import type { DataflowAddressMode, DataflowEndpoint, DataflowEndpointResponse } from "./dataflows-api";

// Selector metadata only. Membership and address intent belong to the resolver.
export const DATAFLOW_ENDPOINTS = {
  "dcim.device": { entity: "Devices", kind: "host" },
  "virtualization.virtualmachine": { entity: "Virtual Machines", kind: "host" },
  "ipam.service": { entity: "Services", kind: "service" },
  "ipam.ipaddress": { entity: "IP Addresses", kind: "address" },
  "ipam.prefix": { entity: "Prefixes", kind: "address" },
  "ipam.iprange": { entity: "IP Ranges", kind: "range" },
  "dcim.region": { entity: "Regions", kind: "group" },
  "dcim.site": { entity: "Sites", kind: "group" },
  "dcim.sitegroup": { entity: "Site Groups", kind: "group" },
  "dcim.devicerole": { entity: "Device Roles", kind: "group" },
  "tenancy.tenant": { entity: "Tenants", kind: "group" },
  "tenancy.tenantgroup": { entity: "Tenant Groups", kind: "group" },
  "dcim.platform": { entity: "Platforms", kind: "group" },
  "dcim.manufacturer": { entity: "Manufacturers", kind: "group" },
  "dcim.devicetype": { entity: "Device Types", kind: "group" },
  "dcim.rack": { entity: "Racks", kind: "group" },
  "dcim.location": { entity: "Locations", kind: "group" },
  "virtualization.cluster": { entity: "Clusters", kind: "group" },
  "virtualization.clustergroup": { entity: "Cluster Groups", kind: "group" },
  "virtualization.clustertype": { entity: "Cluster Types", kind: "group" },
  "extras.tag": { entity: "Tags", kind: "tag" },
  "inventory.addressgroup": { entity: null, kind: "addressgroup" },
  all: { entity: null, kind: "all" },
  internet: { entity: null, kind: "internet" },
} satisfies Record<DataflowEndpoint["object_type"], { entity: InventoryEntityKey | null; kind: "host" | "service" | "address" | "range" | "group" | "tag" | "addressgroup" | "all" | "internet" }>;

export const DATAFLOW_ENDPOINT_TYPES = Object.keys(DATAFLOW_ENDPOINTS) as DataflowEndpoint["object_type"][];

export function dataflowEndpointInput(endpoint: DataflowEndpointResponse | DataflowEndpoint): DataflowEndpoint {
  return {
    object_type: endpoint.object_type,
    object_id: endpoint.object_id,
    address_mode: endpoint.address_mode ?? "default",
    ipaddresses: endpoint.ipaddresses ?? [],
    ...(endpoint.object_type === "extras.tag" ? { tag_object_type: endpoint.tag_object_type } : {}),
  };
}

// Labels are presentation, not identity. Different address selectors and tag scopes stay distinct.
export function dataflowEndpointKey(endpoint: Pick<DataflowEndpoint, "object_type" | "object_id" | "address_mode" | "ipaddresses" | "tag_object_type">): string {
  return JSON.stringify([
    endpoint.object_type,
    endpoint.object_id ?? null,
    endpoint.address_mode ?? "default",
    endpoint.tag_object_type ?? null,
    [...new Set(endpoint.ipaddresses ?? [])].sort((left, right) => left.localeCompare(right)),
  ]);
}

/** Modes the resolver accepts for this selector; the UI must not offer a wider set. */
export function dataflowAddressModesFor(endpoint: Pick<DataflowEndpoint, "object_type" | "tag_object_type">): DataflowAddressMode[] {
  const kind = DATAFLOW_ENDPOINTS[endpoint.object_type].kind;
  if (kind === "range" || kind === "addressgroup" || kind === "internet" || kind === "all") return ["default"];
  if (kind === "tag") return endpoint.tag_object_type === "dcim.device" ? ["default", "all", "oob"] : ["default", "all"];
  if (endpoint.object_type === "dcim.device") return ["default", "selected", "all", "oob"];
  return ["default", "selected", "all"];
}

export function dataflowEndpointSupportsMembershipPreview(endpoint: Pick<DataflowEndpoint, "object_type">): boolean {
  return ["host", "service", "group", "range", "tag", "addressgroup"].includes(DATAFLOW_ENDPOINTS[endpoint.object_type].kind);
}

/** Mirrors the mutation boundary: returns an i18n key for the first invalid field. */
export function dataflowEndpointError(endpoint: DataflowEndpoint): string | undefined {
  const kind = DATAFLOW_ENDPOINTS[endpoint.object_type].kind;
  if (endpoint.object_type === "extras.tag" && !endpoint.tag_object_type) return "dataflows.tagScopeError";
  if (endpoint.object_type === "extras.tag" && endpoint.address_mode === "selected") return "dataflows.tagModeError";
  if (endpoint.tag_object_type && endpoint.object_type !== "extras.tag") return "dataflows.tagScopeError";
  if ((kind === "range" || kind === "addressgroup") && (endpoint.address_mode ?? "default") !== "default") return "dataflows.fixedAddressModeError";
  if (endpoint.address_mode === "oob" && endpoint.object_type !== "dcim.device" && !(endpoint.object_type === "extras.tag" && endpoint.tag_object_type === "dcim.device")) return "dataflows.oobModeError";
  if (endpoint.address_mode === "selected") {
    if (!endpoint.ipaddresses?.length) return "dataflows.addressError";
    if (endpoint.ipaddresses.length > 32) return "dataflows.addressLimitError";
  }
  if (!["all", "internet"].includes(kind) && !endpoint.object_id) return "dataflows.endpointError";
  return undefined;
}
