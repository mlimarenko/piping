import type { InventoryEntityKey } from "@/shared/api/types";
import type { DataflowEndpoint } from "./dataflows-api";

// Selector metadata only. Membership and address intent belong to the resolver.
export const DATAFLOW_ENDPOINTS = {
  "dcim.device": { entity: "Devices", kind: "host" },
  "virtualization.virtualmachine": { entity: "Virtual Machines", kind: "host" },
  "ipam.service": { entity: "Services", kind: "service" },
  "ipam.ipaddress": { entity: "IP Addresses", kind: "address" },
  "ipam.prefix": { entity: "Prefixes", kind: "address" },
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
  all: { entity: null, kind: "all" },
  internet: { entity: null, kind: "internet" },
} satisfies Record<DataflowEndpoint["object_type"], { entity: InventoryEntityKey | null; kind: "host" | "service" | "address" | "group" | "all" | "internet" }>;

export const DATAFLOW_ENDPOINT_TYPES = Object.keys(DATAFLOW_ENDPOINTS) as DataflowEndpoint["object_type"][];

export function dataflowEndpointInput(endpoint: DataflowEndpoint): DataflowEndpoint {
  return { object_type: endpoint.object_type, object_id: endpoint.object_id, address_mode: endpoint.address_mode ?? "default", ipaddresses: endpoint.ipaddresses ?? [] };
}

// Labels are presentation, not identity. Different address selectors stay distinct.
export function dataflowEndpointKey(endpoint: DataflowEndpoint): string {
  return JSON.stringify([endpoint.object_type, endpoint.object_id ?? null, endpoint.address_mode ?? "default", [...new Set(endpoint.ipaddresses ?? [])].sort((left, right) => left.localeCompare(right))]);
}
