import type {
  InventoryEntityKey,
  InventoryTemplate,
  TemplateStep,
  TemplateStepConflictPolicy,
  TemplateStepResourceType,
  TemplateVariable,
  TemplateVariableValueType,
} from "@/shared/api/types";
import referenceRegistry from "../../../../../../api/contracts/inventory-template-references.json";
import { analyzeTemplateStepOrder } from "./templateEditorOrder";

export type TemplateFieldValueKind = "string" | "number" | "boolean";
export type TemplateReferenceResource = keyof typeof referenceRegistry.targets;
export interface TemplateSelection {
  resource: TemplateReferenceResource;
  id: string;
}

export const isTemplateReferenceId = (value: string): boolean =>
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);

export function isTemplateSelection(value: unknown): value is TemplateSelection {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  return Object.keys(candidate).length === 2
    && typeof candidate.resource === "string" && Object.hasOwn(referenceRegistry.targets, candidate.resource)
    && typeof candidate.id === "string" && isTemplateReferenceId(candidate.id);
}

export interface TemplatePayloadFieldDefinition {
  key: string;
  label: string;
  valueKind: TemplateFieldValueKind;
  required?: boolean;
  placeholder?: string;
  description?: string;
  /**
   * Resource type this foreign-key field points at. When set, the authoring
   * editor can auto-bind the field to a prior step of that type (or a generated
   * run variable). Frontend-only metadata — not part of the API DTO.
   */
  references?: TemplateReferenceResource;
  /**
   * Manual text resolves server-side by slug/name. Typed picks project to the
   * UUID destination defined by the shared field registry.
   */
  referencesByName?: TemplateReferenceResource;
}

export interface TemplateResourceDefinition {
  resourceType: TemplateStepResourceType;
  label: string;
  description: string;
  defaultStepKey: string;
  defaultTitle: string;
  payloadFields: TemplatePayloadFieldDefinition[];
}

export const TEMPLATE_VARIABLE_VALUE_TYPES: Array<{
  value: TemplateVariableValueType;
  label: string;
}> = [
  { value: "string", label: "String" },
  { value: "integer", label: "Integer" },
  { value: "number", label: "Number" },
  { value: "boolean", label: "Boolean" },
  { value: "json", label: "JSON" },
  { value: "resource_ref", label: "Resource reference" },
];

export const TEMPLATE_CONFLICT_POLICIES: Array<{
  value: TemplateStepConflictPolicy;
  label: string;
}> = [
  { value: "fail", label: "Fail" },
  { value: "skip", label: "Skip" },
  { value: "update_selected_fields", label: "Update selected fields" },
];

const TEMPLATE_RESOURCE_PRESENTATION: TemplateResourceDefinition[] = [
  {
    resourceType: "tenant",
    label: "Tenant",
    description: "Create a tenancy tenant.",
    defaultStepKey: "tenant",
    defaultTitle: "Create tenant",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{tenant_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{tenant_slug}}" },
      { key: "group_id", label: "Group ID", valueKind: "string", placeholder: "{{tenant_group_id}}" },    ],
  },
  {
    resourceType: "site",
    label: "Site",
    description: "Create a DCIM site.",
    defaultStepKey: "site",
    defaultTitle: "Create site",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{site_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{site_slug}}" },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}"  },
      { key: "tenant", label: "Tenant", valueKind: "string", placeholder: "{{tenant_name}}" , description: "Existing tenant, resolved by slug or name." },
      { key: "region_id", label: "Region ID", valueKind: "string", placeholder: "{{region_id}}"  },
      { key: "region", label: "Region", valueKind: "string", placeholder: "{{region_name}}" , description: "Existing region, resolved by slug or name." },
      { key: "site_group_id", label: "Site group ID", valueKind: "string", placeholder: "{{site_group_id}}"  },
      { key: "site_group", label: "Site group", valueKind: "string", placeholder: "{{site_group_name}}" , description: "Existing site group, resolved by slug or name." },
      { key: "facility", label: "Facility", valueKind: "string", placeholder: "DC-1" },
      { key: "time_zone", label: "Time zone", valueKind: "string", placeholder: "Europe/Moscow" },
      { key: "physical_address", label: "Physical address", valueKind: "string" },
      { key: "shipping_address", label: "Shipping address", valueKind: "string" },
      { key: "latitude", label: "Latitude", valueKind: "number", placeholder: "{{latitude}}" },
      { key: "longitude", label: "Longitude", valueKind: "number", placeholder: "{{longitude}}" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "comments", label: "Comments", valueKind: "string" },
    ],
  },
  {
    resourceType: "device",
    label: "Device",
    description: "Create a DCIM device.",
    defaultStepKey: "device",
    defaultTitle: "Create device",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{device_name}}" },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "site_id", label: "Site ID", valueKind: "string", required: true, placeholder: "{{steps.site.id}}"  },
      { key: "site", label: "Site", valueKind: "string", placeholder: "{{site_name}}" , description: "Existing site, resolved by slug or name." },
      { key: "role_id", label: "Role ID", valueKind: "string", required: true, placeholder: "{{device_role_id}}"  },
      { key: "role", label: "Role", valueKind: "string", placeholder: "{{device_role}}" , description: "Existing device role, resolved by slug or name." },
      { key: "device_type_id", label: "Device type ID", valueKind: "string", required: true, placeholder: "{{device_type_id}}"  },
      { key: "device_type", label: "Device type", valueKind: "string", placeholder: "{{device_type}}" , description: "Existing device type, resolved by slug or model." },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}"  },
      { key: "tenant", label: "Tenant", valueKind: "string", placeholder: "{{tenant_name}}" , description: "Existing tenant, resolved by slug or name." },
    ],
  },
  {
    resourceType: "virtual_machine",
    label: "Virtual machine",
    description: "Create a virtualization virtual machine.",
    defaultStepKey: "virtual_machine",
    defaultTitle: "Create virtual machine",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{vm_name}}" },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "cluster_id", label: "Cluster ID", valueKind: "string", placeholder: "{{cluster_id}}"  },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}"  },
      { key: "role_id", label: "Role ID", valueKind: "string", placeholder: "{{vm_role_id}}"  },
      { key: "site_id", label: "Site ID", valueKind: "string", placeholder: "{{steps.site.id}}"  },
    ],
  },
  {
    resourceType: "device_interface",
    label: "Device interface",
    description: "Create a device interface.",
    defaultStepKey: "device_interface",
    defaultTitle: "Create device interface",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "mgmt0" },
      { key: "device_id", label: "Device ID", valueKind: "string", required: true, placeholder: "{{steps.device.id}}"  },
      { key: "type", label: "Type", valueKind: "string", placeholder: "virtual" },
      { key: "mgmt_only", label: "Management only", valueKind: "boolean", placeholder: "true" },
    ],
  },
  {
    resourceType: "vm_interface",
    label: "VM interface",
    description: "Create a virtual machine interface.",
    defaultStepKey: "vm_interface",
    defaultTitle: "Create VM interface",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "eth0" },
      { key: "virtual_machine_id", label: "Virtual machine ID", valueKind: "string", required: true, placeholder: "{{steps.virtual_machine.id}}"  },
    ],
  },
  {
    resourceType: "ip_address",
    label: "IP address",
    description: "Create an IPAM address.",
    defaultStepKey: "ip_address",
    defaultTitle: "Create IP address",
    payloadFields: [
      { key: "address", label: "Address", valueKind: "string", required: true, placeholder: "{{ip_cidr}}" },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "dns_name", label: "DNS name", valueKind: "string", placeholder: "{{hostname}}.example.internal" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "comments", label: "Comments", valueKind: "string" },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}"  },
      { key: "interface_id", label: "Interface ID", valueKind: "string", placeholder: "{{steps.device_interface.id}}"  },
      { key: "vm_interface_id", label: "VM interface ID", valueKind: "string", placeholder: "{{steps.vm_interface.id}}"  },
      { key: "set_primary", label: "Set primary IP", valueKind: "boolean", placeholder: "true" },
    ],
  },
  {
    resourceType: "tenant_group",
    label: "Tenant group",
    description: "Create a tenancy tenant group.",
    defaultStepKey: "tenant_group",
    defaultTitle: "Create tenant group",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{tenant_group_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{tenant_group_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "parent_id", label: "Parent ID", valueKind: "string", placeholder: "{{steps.tenant_group.id}}" , description: "UUID or {{steps.<key>.id}} reference to parent tenant group." },
    ],
  },
  {
    resourceType: "region",
    label: "Region",
    description: "Create a DCIM region.",
    defaultStepKey: "region",
    defaultTitle: "Create region",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{region_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{region_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "parent_id", label: "Parent ID", valueKind: "string", placeholder: "{{steps.region.id}}" , description: "UUID or {{steps.<key>.id}} reference to parent region." },
    ],
  },
  {
    resourceType: "site_group",
    label: "Site group",
    description: "Create a DCIM site group.",
    defaultStepKey: "site_group",
    defaultTitle: "Create site group",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{site_group_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{site_group_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "parent_id", label: "Parent ID", valueKind: "string", placeholder: "{{steps.site_group.id}}" , description: "UUID or {{steps.<key>.id}} reference to parent site group." },
    ],
  },
  {
    resourceType: "location",
    label: "Location",
    description: "Create a DCIM location within a site.",
    defaultStepKey: "location",
    defaultTitle: "Create location",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{location_name}}" },
      { key: "site_id", label: "Site ID", valueKind: "string", required: true, placeholder: "{{steps.site.id}}" , description: "UUID or {{steps.<key>.id}} reference to the parent site." },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{location_slug}}" },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "facility", label: "Facility", valueKind: "string", placeholder: "building-a" },
      { key: "parent_id", label: "Parent ID", valueKind: "string", placeholder: "{{steps.location.id}}" , description: "UUID or {{steps.<key>.id}} reference to parent location." },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}" , description: "UUID or {{steps.<key>.id}} reference to a tenant." },
    ],
  },
  {
    resourceType: "prefix",
    label: "Prefix",
    description: "Create an IPAM prefix.",
    defaultStepKey: "prefix",
    defaultTitle: "Create prefix",
    payloadFields: [
      { key: "prefix", label: "Prefix", valueKind: "string", required: true, placeholder: "{{shop_subnet}}" },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "site_id", label: "Site ID", valueKind: "string", placeholder: "{{steps.site.id}}"  },
      { key: "site", label: "Site", valueKind: "string", placeholder: "{{site_name}}" , description: "Existing site, resolved by slug or name." },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}"  },
      { key: "tenant", label: "Tenant", valueKind: "string", placeholder: "{{tenant_name}}" , description: "Existing tenant, resolved by slug or name." },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "comments", label: "Comments", valueKind: "string" },
    ],
  },
  {
    resourceType: "rack",
    label: "Rack",
    description: "Create a DCIM rack.",
    defaultStepKey: "rack",
    defaultTitle: "Create rack",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{rack_name}}" },
      { key: "site_id", label: "Site ID", valueKind: "string", required: true, placeholder: "{{steps.site.id}}" , description: "UUID or {{steps.<key>.id}} reference to the parent site." },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "width", label: "Width (in)", valueKind: "number", placeholder: "19" },
      { key: "u_height", label: "Height (U)", valueKind: "number", placeholder: "42" },
      { key: "desc_units", label: "Descending units", valueKind: "boolean", placeholder: "false" },
      { key: "facility_id", label: "Facility ID", valueKind: "string", placeholder: "R-101" },
      { key: "serial", label: "Serial", valueKind: "string", placeholder: "SN-000001" },
      { key: "asset_tag", label: "Asset tag", valueKind: "string", placeholder: "AT-000001" },
      { key: "form_factor", label: "Form factor", valueKind: "string", placeholder: "2-post-frame" },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}" , description: "UUID or {{steps.<key>.id}} reference to a tenant." },
      { key: "role_id", label: "Role ID", valueKind: "string", placeholder: "{{steps.rack_role.id}}" , description: "UUID or {{steps.<key>.id}} reference to a rack role." },
    ],
  },
  {
    resourceType: "rack_role",
    label: "Rack role",
    description: "Create a DCIM rack role.",
    defaultStepKey: "rack_role",
    defaultTitle: "Create rack role",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{rack_role_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{rack_role_slug}}" },
      { key: "color", label: "Color", valueKind: "string", placeholder: "ff0000", description: "Hex color code, e.g. ff0000." },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "rack_type",
    label: "Rack type",
    description: "Create a DCIM rack type.",
    defaultStepKey: "rack_type",
    defaultTitle: "Create rack type",
    payloadFields: [
      { key: "model", label: "Model", valueKind: "string", required: true, placeholder: "{{rack_type_model}}" },
      { key: "manufacturer_id", label: "Manufacturer ID", valueKind: "string", required: true, placeholder: "{{steps.manufacturer.id}}" , description: "UUID or {{steps.<key>.id}} reference to a manufacturer." },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{rack_type_slug}}" },
      { key: "form_factor", label: "Form factor", valueKind: "string", placeholder: "2-post-frame" },
      { key: "width", label: "Width (in)", valueKind: "number", placeholder: "19" },
      { key: "u_height", label: "Height (U)", valueKind: "number", placeholder: "42" },
      { key: "starting_unit", label: "Starting unit", valueKind: "number", placeholder: "1" },
      { key: "desc_units", label: "Descending units", valueKind: "boolean", placeholder: "false" },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "manufacturer",
    label: "Manufacturer",
    description: "Create a DCIM manufacturer.",
    defaultStepKey: "manufacturer",
    defaultTitle: "Create manufacturer",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{manufacturer_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{manufacturer_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "device_role",
    label: "Device role",
    description: "Create a DCIM device role.",
    defaultStepKey: "device_role",
    defaultTitle: "Create device role",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{device_role_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{device_role_slug}}" },
      { key: "color", label: "Color", valueKind: "string", placeholder: "00bcd4", description: "Hex color code, e.g. 00bcd4." },
      { key: "vm_role", label: "VM role", valueKind: "boolean", placeholder: "false" },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "device_type",
    label: "Device type",
    description: "Create a DCIM device type.",
    defaultStepKey: "device_type",
    defaultTitle: "Create device type",
    payloadFields: [
      { key: "model", label: "Model", valueKind: "string", required: true, placeholder: "{{device_type_model}}" },
      { key: "manufacturer_id", label: "Manufacturer ID", valueKind: "string", required: true, placeholder: "{{steps.manufacturer.id}}" , description: "UUID or {{steps.<key>.id}} reference to a manufacturer." },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{device_type_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "contact",
    label: "Contact",
    description: "Create a tenancy contact.",
    defaultStepKey: "contact",
    defaultTitle: "Create contact",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{contact_name}}" },
      { key: "title", label: "Title", valueKind: "string" },
      { key: "phone", label: "Phone", valueKind: "string", placeholder: "{{contact_phone}}" },
      { key: "email", label: "Email", valueKind: "string", placeholder: "{{contact_email}}" },
      { key: "address", label: "Address", valueKind: "string" },
      { key: "link", label: "Link", valueKind: "string" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "comments", label: "Comments", valueKind: "string" },
    ],
  },
  {
    resourceType: "contact_assignment",
    label: "Contact assignment",
    description: "Attach a contact to a site, device, VM, or other inventory entity.",
    defaultStepKey: "contact_assignment",
    defaultTitle: "Assign contact",
    payloadFields: [
      { key: "object_type", label: "Object type", valueKind: "string", required: true, placeholder: "dcim.site" },
      { key: "object_id", label: "Object ID", valueKind: "string", required: true, placeholder: "{{steps.site.id}}" },
      { key: "contact_id", label: "Contact ID", valueKind: "string", required: true, placeholder: "{{contact_id}}"  },
      { key: "role_id", label: "Role ID", valueKind: "string", required: true, placeholder: "{{contact_role_id}}"  },
      { key: "priority", label: "Priority", valueKind: "string", placeholder: "primary" },
    ],
  },
  {
    resourceType: "platform",
    label: "Platform",
    description: "Create a DCIM platform.",
    defaultStepKey: "platform",
    defaultTitle: "Create platform",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{platform_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{platform_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "manufacturer_id", label: "Manufacturer ID", valueKind: "string", placeholder: "{{steps.manufacturer.id}}" , description: "UUID or {{steps.<key>.id}} reference to a manufacturer." },
    ],
  },
  {
    resourceType: "cluster",
    label: "Cluster",
    description: "Create a virtualization cluster.",
    defaultStepKey: "cluster",
    defaultTitle: "Create cluster",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{cluster_name}}" },
      { key: "type_id", label: "Type ID", valueKind: "string", required: true, placeholder: "{{steps.cluster_type.id}}" , description: "UUID or {{steps.<key>.id}} reference to a cluster type." },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "group_id", label: "Group ID", valueKind: "string", placeholder: "{{steps.cluster_group.id}}" , description: "UUID or {{steps.<key>.id}} reference to a cluster group." },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string", placeholder: "{{steps.tenant.id}}" , description: "UUID or {{steps.<key>.id}} reference to a tenant." },
      { key: "site_id", label: "Site ID", valueKind: "string", placeholder: "{{steps.site.id}}" , description: "UUID or {{steps.<key>.id}} reference to a site." },
    ],
  },
  {
    resourceType: "cluster_type",
    label: "Cluster type",
    description: "Create a virtualization cluster type.",
    defaultStepKey: "cluster_type",
    defaultTitle: "Create cluster type",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{cluster_type_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{cluster_type_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "cluster_group",
    label: "Cluster group",
    description: "Create a virtualization cluster group.",
    defaultStepKey: "cluster_group",
    defaultTitle: "Create cluster group",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true, placeholder: "{{cluster_group_name}}" },
      { key: "slug", label: "Slug", valueKind: "string", placeholder: "{{cluster_group_slug}}" },
      { key: "description", label: "Description", valueKind: "string" },
    ],
  },
  {
    resourceType: "tunnel_group",
    label: "Tunnel group",
    description: "Create a VPN tunnel group.",
    defaultStepKey: "tunnel_group",
    defaultTitle: "Create tunnel group",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true },
      { key: "slug", label: "Slug", valueKind: "string" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "comments", label: "Comments", valueKind: "string" },
    ],
  },
  {
    resourceType: "tunnel",
    label: "Tunnel",
    description: "Create a VPN tunnel.",
    defaultStepKey: "tunnel",
    defaultTitle: "Create tunnel",
    payloadFields: [
      { key: "name", label: "Name", valueKind: "string", required: true },
      { key: "status", label: "Status", valueKind: "string", placeholder: "active" },
      { key: "encapsulation", label: "Encapsulation", valueKind: "string", placeholder: "ipsec-tunnel" },
      { key: "description", label: "Description", valueKind: "string" },
      { key: "comments", label: "Comments", valueKind: "string" },
      { key: "group_id", label: "Group ID", valueKind: "string", placeholder: "{{steps.tunnel_group.id}}" },
      { key: "ipsec_profile_id", label: "IPsec profile ID", valueKind: "string" },
      { key: "tunnel_id", label: "Tunnel identifier", valueKind: "number" },
      { key: "tenant_id", label: "Tenant ID", valueKind: "string" },
    ],
  },
  {
    resourceType: "tunnel_termination",
    label: "Tunnel termination",
    description: "Terminate a VPN tunnel on a device or VM interface.",
    defaultStepKey: "tunnel_termination",
    defaultTitle: "Create tunnel termination",
    payloadFields: [
      { key: "role", label: "Role", valueKind: "string", required: true },
      { key: "tunnel_id", label: "Tunnel ID", valueKind: "string", required: true, placeholder: "{{steps.tunnel.id}}" },
      { key: "interface_id", label: "Device interface ID", valueKind: "string", description: "Choose a device interface, a VM interface, or both a content type and object ID." },
      { key: "vm_interface_id", label: "VM interface ID", valueKind: "string" },
      { key: "termination_type_id", label: "Termination content type ID", valueKind: "string" },
      { key: "termination_id", label: "Termination object ID", valueKind: "string" },
      { key: "outside_ip_id", label: "Outside IP address ID", valueKind: "string" },
      { key: "outside_ip", label: "Outside IP address", valueKind: "string", description: "Created if needed when no outside IP address ID is supplied." },
    ],
  },
];

const referenceFields = referenceRegistry.fields as Partial<Record<TemplateStepResourceType, Record<string, { resource: TemplateReferenceResource; idField: string }>>>;
export const TEMPLATE_RESOURCE_DEFINITIONS: TemplateResourceDefinition[] = TEMPLATE_RESOURCE_PRESENTATION.map((definition) => ({
  ...definition,
  payloadFields: definition.payloadFields.map((field) => {
    const reference = referenceFields[definition.resourceType]?.[field.key];
    if (!reference) return field;
    return { ...field, ...(reference.idField === field.key
      ? { references: reference.resource } : { referencesByName: reference.resource }) };
  }),
}));

export const TEMPLATE_RESOURCE_DEFINITION_MAP = Object.fromEntries(
  TEMPLATE_RESOURCE_DEFINITIONS.map((definition) => [definition.resourceType, definition]),
) as Record<TemplateStepResourceType, TemplateResourceDefinition>;

// ── Reference variables ──────────────────────────────────────────────────────
//
// A reference variable points at ONE kind of existing inventory object. The
// consuming step payload field decides whether manual names are accepted.
// Every picked object retains its resource/UUID identity; the backend projects
// it to the consuming field's UUID destination. A `resource_ref` variable may
// also declare the kind in
// `validation_rules.resource`; the step binding stays authoritative because it
// is what the run actually consumes.

/** Inventory entity the run wizard picks from, per reference token. */
export const TEMPLATE_REFERENCE_ENTITIES = Object.fromEntries(
  Object.entries(referenceRegistry.targets).map(([resource, target]) => [resource, target.entity]),
) as Record<TemplateReferenceResource, InventoryEntityKey>;

export interface TemplateReference {
  /** Reference target, including entities that cannot be created by a step. */
  resource: TemplateReferenceResource;
  /** Inventory entity the run wizard lists. */
  entity: InventoryEntityKey;
  /** Identifier the consuming step field expects. */
  value: "id" | "name";
}

const TEMPLATE_REFERENCE_RESOURCES = new Set<string>(
  Object.keys(TEMPLATE_REFERENCE_ENTITIES),
);

/** Resolve a stored declaration to its reference target. */
function templateReferenceResource(value: unknown): TemplateReferenceResource | null {
  if (typeof value !== "string") return null;
  return TEMPLATE_REFERENCE_RESOURCES.has(value) ? (value as TemplateReferenceResource) : null;
}

const declaredReferenceResource = (variable: TemplateVariable): TemplateReferenceResource | null => {
  const rules = variable.validation_rules;
  if (!rules || typeof rules !== "object" || Array.isArray(rules)) return null;
  return templateReferenceResource(rules.resource);
};

const referenceOf = (resource: TemplateReferenceResource, value: "id" | "name"): TemplateReference => ({
  resource,
  entity: TEMPLATE_REFERENCE_ENTITIES[resource],
  value,
});

const containsVariableToken = (value: unknown, key: string): boolean => {
  if (typeof value === "string") return Array.from(value.matchAll(/\{\{\s*([^{}]+?)\s*\}\}/g)).some((match) => match[1]?.trim().split(/\s+/).includes(key));
  if (Array.isArray(value)) return value.some((item: unknown) => containsVariableToken(item, key));
  if (value && typeof value === "object") return Object.values(value).some((item: unknown) => containsVariableToken(item, key));
  return false;
};
const isWholeVariableToken = (value: unknown, key: string): boolean => typeof value === "string"
  && value.trim().match(/^\{\{\s*([^{}]+?)\s*\}\}$/)?.[1]?.trim() === key;

export const hasTemplateVariableConsumers = (template: InventoryTemplate, variable: TemplateVariable): boolean =>
  template.steps.some((step) => containsVariableToken(step.payload_template, variable.key));

/** Reference the step field bound to `{{variable.key}}` declares, if any. */
function boundReferenceFor(
  template: InventoryTemplate,
  variable: TemplateVariable,
): TemplateReference | null {
  let bound: TemplateReference | null = null;
  for (const step of template.steps) {
    const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[step.resource_type];
    if (!definition) continue;
    for (const [fieldKey, payloadValue] of Object.entries(step.payload_template)) {
      if (!containsVariableToken(payloadValue, variable.key)) continue;
      if (!isWholeVariableToken(payloadValue, variable.key)) return null;
      const field = definition.payloadFields.find((candidate) => candidate.key === fieldKey);
      const reference = field?.referencesByName ? referenceOf(field.referencesByName, "name")
        : field?.references ? referenceOf(field.references, "id") : null;
      // A creation/string consumer must not become an eager object picker.
      // All reference consumers must agree on the target; projection supplies
      // the identifier expected by each name/UUID field.
      if (!reference) return null;
      if (bound && bound.resource !== reference.resource) return null;
      if (!bound || reference.value === "id") bound = reference;
    }
  }
  return bound;
}

/**
 * Effective reference of a variable: the step payload field that consumes
 * `{{key}}`, or — for an unbound `resource_ref` variable — its declared
 * `resource` rule. `null` for anything that is not a reference.
 */
export function templateReferenceFor(
  template: InventoryTemplate,
  variable: TemplateVariable,
): TemplateReference | null {
  if (variable.value_type === "json") return null;
  const bound = boundReferenceFor(template, variable);
  if (bound) return bound;
  // A bound variable with incompatible/free-text consumers is not an unbound
  // declaration. Do not let its rule turn creation inputs into object picks.
  if (hasTemplateVariableConsumers(template, variable)) return null;
  if (variable.value_type !== "resource_ref") return null;
  const declared = declaredReferenceResource(variable);
  return declared ? referenceOf(declared, "id") : null;
}

type TranslatePresentation = (key: string, options?: { defaultValue?: string; [key: string]: unknown }) => string;

// Factory text is recognized at its consuming field, never by a variable key.
// System templates remain editable: text not in these defaults belongs to the author.
const FACTORY_FIELD_TEXT: Record<string, { labels: string[]; help: string[]; helpKey: string }> = {
  "site.name": { labels: ["Site name"], help: ["Site to create.", "Site to create for the server.", "Site to create for the network device.", "Site to create for the virtual machine.", "Site to create for the rack."], helpKey: "createObjectHelp" },
  "device.name": { labels: ["Device name"], help: ["Server inventory name.", "Network device inventory name."], helpKey: "createObjectHelp" },
  "virtual_machine.name": { labels: ["VM name"], help: ["Virtual machine inventory name."], helpKey: "createObjectHelp" },
  "device.role": { labels: ["Device role"], help: ["Existing device role slug or name."], helpKey: "existingObjectHelp" },
  "device.device_type": { labels: ["Device type"], help: ["Existing device type slug or model."], helpKey: "existingObjectHelp" },
  "device_interface.name": { labels: ["Interface name"], help: ["Management interface name."], helpKey: "createObjectHelp" },
  "vm_interface.name": { labels: ["Interface name"], help: ["Virtual NIC name."], helpKey: "createObjectHelp" },
  "ip_address.address": { labels: ["Primary IP"], help: ["Primary address in CIDR notation."], helpKey: "cidrHelp" },
  "rack.name": { labels: ["Rack name"], help: ["Rack inventory name."], helpKey: "createObjectHelp" },
  "rack.u_height": { labels: ["Rack units"], help: ["Rack height in rack units."], helpKey: "rackHeightHelp" },
  "contact.name": { labels: ["Contact name"], help: ["Contact display name."], helpKey: "createObjectHelp" },
  "contact.email": { labels: ["Contact email"], help: ["Contact email address."], helpKey: "contactEmailHelp" },
};

export function templatePayloadFieldLabel(resource: TemplateStepResourceType, field: TemplatePayloadFieldDefinition, t: TranslatePresentation): string {
  const target = field.references ?? field.referencesByName;
  if (target) return t(`inventory.templates.resourceTypes.${target}`, { defaultValue: field.label });
  const camelKey = field.key.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase());
  const label = t(`inventory.resources.fields.${camelKey}`, {
    defaultValue: t(`inventory.templates.payloadFields.${field.key}`, { defaultValue: field.label }),
  });
  return field.key === "name"
    ? t("inventory.templates.run.objectName", { type: t(`inventory.templates.resourceTypes.${resource}`) })
    : label;
}

export function templateVariablePresentation(template: InventoryTemplate, variable: TemplateVariable, t: TranslatePresentation) {
  const consumers: Array<{ resource: TemplateStepResourceType; field: TemplatePayloadFieldDefinition }> = [];
  let ambiguous = false;
  for (const step of template.steps) {
    for (const [key, value] of Object.entries(step.payload_template)) {
      if (!containsVariableToken(value, variable.key)) continue;
      const field = TEMPLATE_RESOURCE_DEFINITION_MAP[step.resource_type]?.payloadFields.find((candidate) => candidate.key === key);
      if (!field || !isWholeVariableToken(value, variable.key)) { ambiguous = true; continue; }
      consumers.push({ resource: step.resource_type, field });
    }
  }
  const owner = consumers[0];
  const canonical = !ambiguous && owner && consumers.every((consumer) => consumer.resource === owner.resource && consumer.field.key === owner.field.key) ? owner : null;
  const reference = templateReferenceFor(template, variable);
  const defaults = canonical ? FACTORY_FIELD_TEXT[`${canonical.resource}.${canonical.field.key}`] : undefined;
  const label = template.is_system && canonical && defaults?.labels.includes(variable.label)
    ? templatePayloadFieldLabel(canonical.resource, canonical.field, t) : variable.label || variable.key;
  const help = template.is_system && defaults?.help.includes(variable.description ?? "")
    ? t(`inventory.templates.run.${defaults.helpKey}`) : variable.description;
  return { label, help, group: reference ? "references" : canonical ? "creates" : "parameters" } as const;
}

/**
 * Persisted reference declarations follow the consuming fields, matching the
 * editor's read-only selector. Unbound declarations remain editable.
 */
function normalizeVariableRules(
  template: InventoryTemplate,
  variable: TemplateVariable,
): Record<string, unknown> {
  const rules = variable.validation_rules ?? {};
  if (variable.value_type !== "resource_ref") return rules;
  const reference = templateReferenceFor(template, variable);
  if (!reference && hasTemplateVariableConsumers(template, variable)) {
    // A mixed creation/reference input cannot use a declared object kind.
    const { resource: _ignored, ...unboundRules } = rules;
    return unboundRules;
  }
  return reference ? { ...rules, resource: reference.resource } : rules;
}

export interface PreviewRow {
  step_key: string;
  title: string;
  resource_type: TemplateStepResourceType;
  payload: Record<string, unknown>;
}

export function slugifyTemplateKey(value: string, fallback: string): string {
  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .replace(/_+/g, "_");
  return normalized || fallback;
}

export function formatTemplateValue(value: unknown): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

export function isRequiredTemplateVariableWithoutDefault(variable: TemplateVariable): boolean {
  return Boolean(variable.required) && !formatTemplateValue(variable.default_value).trim();
}

export function parseConfiguredValue(rawValue: string, kind: TemplateFieldValueKind): unknown {
  const value = rawValue.trim();
  if (!value) return undefined;
  if (isTemplatePlaceholder(value)) return value;
  if (kind === "number") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : value;
  }
  if (kind === "boolean") {
    if (value === "true") return true;
    if (value === "false") return false;
  }
  return value;
}

export function isTemplatePlaceholder(value: string): boolean {
  return /^\{\{\s*[^{}]+\s*\}\}$/.test(value.trim());
}

export function isConfiguredValueCompatible(
  value: unknown,
  kind: TemplateFieldValueKind,
): boolean {
  if (value === undefined) return true;
  if (typeof value === "string") {
    return kind === "string" || isTemplatePlaceholder(value);
  }
  if (kind === "number") return typeof value === "number" && Number.isFinite(value);
  if (kind === "boolean") return typeof value === "boolean";
  return false;
}

export function formatTemplateVariableDefault(variable: TemplateVariable): string {
  return formatTemplateValue(variable.default_value);
}

export function parseTemplateVariableDefault(
  valueType: TemplateVariableValueType,
  rawValue: unknown,
  referenceResource?: TemplateReferenceResource,
  structuredReference = false,
): { value: unknown; error?: string } {
  if (structuredReference && valueType !== "json") {
    if (rawValue === undefined || rawValue === null || (typeof rawValue === "string" && !rawValue.trim())) return { value: undefined };
    try {
      const parsed: unknown = typeof rawValue === "string" ? JSON.parse(rawValue) : rawValue;
      return parsed && typeof parsed === "object"
        ? parseTemplateVariableDefault(valueType, parsed, referenceResource)
        : { value: rawValue, error: "invalid_reference" };
    } catch {
      return { value: rawValue, error: "invalid_reference" };
    }
  }
  if (rawValue && typeof rawValue === "object") {
    if (valueType === "json") return { value: rawValue };
    return isTemplateSelection(rawValue) && (valueType === "resource_ref" || referenceResource)
      && (!referenceResource || rawValue.resource === referenceResource)
      ? { value: rawValue } : { value: rawValue, error: "invalid_reference" };
  }
  const value = formatTemplateValue(rawValue).trim();
  if (!value) return { value: undefined };
  if (valueType === "resource_ref" && /^[{[]/.test(value)) {
    try {
      return parseTemplateVariableDefault(valueType, JSON.parse(value) as unknown, referenceResource);
    } catch {
      return { value, error: "invalid_reference" };
    }
  }
  if (valueType === "json") {
    try {
      return { value: JSON.parse(value) };
    } catch {
      return { value, error: "invalid_json" };
    }
  }
  if (valueType === "boolean") {
    if (value === "true") return { value: true };
    if (value === "false") return { value: false };
    return { value, error: "invalid_boolean" };
  }
  if (valueType === "integer") {
    if (/^-?\d+$/.test(value) && Number.isSafeInteger(Number(value))) return { value: Number(value) };
    return { value, error: "invalid_integer" };
  }
  if (valueType === "number") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return { value: parsed };
    return { value, error: "invalid_number" };
  }
  return { value };
}

export function getTemplateVariableOptions(variable: TemplateVariable): string[] {
  const rules = variable.validation_rules;
  if (!rules || typeof rules !== "object" || Array.isArray(rules)) return [];

  const options = (rules).options ?? (rules).enum;
  if (!Array.isArray(options)) return [];
  return [...new Set(options.filter((item): item is string => typeof item === "string" && item.trim().length > 0))];
}

export function getTemplateVariableFields(variable: TemplateVariable): TemplateVariable[] | null {
  if (variable.value_type !== "json" || !Object.hasOwn(variable.validation_rules ?? {}, "fields")) return null;
  const fields = variable.validation_rules?.fields;
  if (!Array.isArray(fields) || !fields.length) return [];
  const keys = new Set<string>();
  for (const candidate of fields as unknown[]) {
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) return [];
    const field = candidate as Record<string, unknown>;
    const rawRules = field.validation_rules;
    if (rawRules !== undefined && (!rawRules || typeof rawRules !== "object" || Array.isArray(rawRules))) return [];
    const rules = (rawRules ?? {}) as Record<string, unknown>;
    if (typeof field.key !== "string" || !/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(field.key)
      || ["__proto__", "constructor", "prototype"].includes(field.key)
      || keys.has(field.key) || typeof field.label !== "string"
      || (field.description !== undefined && typeof field.description !== "string")
      || !TEMPLATE_VARIABLE_VALUE_TYPES.some(({ value }) => value === field.value_type)
      || (field.required !== undefined && typeof field.required !== "boolean")
      || (field.value_type === "resource_ref" && (typeof rules.resource !== "string" || !Object.hasOwn(referenceRegistry.targets, rules.resource)))
      || Object.hasOwn(rules, "fields")) return [];
    keys.add(field.key);
  }
  return fields as TemplateVariable[];
}

export function parseTemplateObjectRows(rawValue: unknown, fields: readonly TemplateVariable[] = []): { rows: Record<string, unknown>[]; error?: string } {
  if (rawValue === undefined || rawValue === null || rawValue === "") return { rows: [] };
  let parsed: unknown = rawValue;
  if (typeof rawValue === "string") {
    try { parsed = JSON.parse(rawValue); }
    catch { return { rows: [], error: "invalid_collection" }; }
  }
  return Array.isArray(parsed) && parsed.every((row) => row && typeof row === "object" && !Array.isArray(row))
    ? { rows: (parsed as Record<string, unknown>[]).map((row) => ({
      ...Object.fromEntries(fields.filter((field) => field.default_value !== undefined && !Object.hasOwn(row, field.key)).map((field) => [field.key, field.default_value])),
      ...row,
    })) }
    : { rows: [], error: "invalid_collection" };
}

function buildTemplateObjectValues(variable: TemplateVariable, fields: TemplateVariable[], rawValue: unknown) {
  const parsed = parseTemplateObjectRows(rawValue, fields);
  const errors: Record<string, string> = {};
  if (!fields.length || parsed.error || (variable.required && !parsed.rows.length)) {
    errors[variable.key] = !fields.length ? "invalid_fields" : parsed.error ?? "required";
    return { values: [], errors };
  }
  const values = parsed.rows.map((row, index) => {
    const value: Record<string, unknown> = {};
    if (Object.keys(row).some((key) => !fields.some((field) => field.key === key))) errors[variable.key] = "invalid_collection";
    for (const field of fields) {
      const key = `${variable.key}.${index}.${field.key}`;
      const raw = row[field.key];
      if (raw !== null && typeof raw === "object" && field.value_type !== "json") {
        errors[key] = "invalid_reference";
        continue;
      }
      if (field.value_type === "resource_ref" && raw !== undefined && raw !== null && !(typeof raw === "string" && !raw.trim())) {
        if (typeof raw !== "string" || !isTemplateReferenceId(raw)) errors[key] = "invalid_reference";
        else value[field.key] = raw;
        continue;
      }
      const result = buildTemplateVariableValues([field], Object.hasOwn(row, field.key)
        ? { [field.key]: raw === null || typeof raw === "string" ? raw : formatTemplateValue(raw) } : {});
      const error = result.errors[field.key];
      if (error) errors[key] = error;
      else if (result.values[field.key] !== undefined) {
        if (field.validation_rules?.format === "email" && typeof result.values[field.key] === "string" && result.values[field.key] !== ""
          && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(result.values[field.key] as string)) errors[key] = "invalid_email";
        else value[field.key] = result.values[field.key];
      }
    }
    return value;
  });
  return { values, errors };
}

export function buildTemplateVariableValues(
  variables: TemplateVariable[],
  rawValues: Record<string, unknown>,
): { values: Record<string, unknown>; errors: Record<string, string> } {
  const values: Record<string, unknown> = {};
  const errors: Record<string, string> = {};

  for (const variable of variables) {
    const rawValue = rawValues[variable.key];
    const fields = getTemplateVariableFields(variable);
    if (fields !== null) {
      const result = buildTemplateObjectValues(variable, fields, rawValue);
      Object.assign(errors, result.errors);
      if (!Object.keys(result.errors).length) values[variable.key] = result.values;
      continue;
    }
    if (rawValue === undefined || rawValue === null || (typeof rawValue === "string" && !rawValue.trim())) {
      if (variable.required) errors[variable.key] = "required";
      else if (Object.hasOwn(rawValues, variable.key) && rawValue !== undefined) {
        // Explicit clears must survive JSON serialization so the server does not
        // restore a default. Numeric, boolean and JSON controls clear to null.
        values[variable.key] = rawValue === null || !["string", "resource_ref"].includes(variable.value_type) ? null : "";
      }
      continue;
    }
    const options = getTemplateVariableOptions(variable);
    if (options.length && rawValue !== "" && (typeof rawValue !== "string" || !options.includes(rawValue.trim()))) {
      errors[variable.key] = "invalid_option";
      continue;
    }
    if (variable.value_type !== "json" && isTemplateSelection(rawValue)) {
      values[variable.key] = rawValue;
      continue;
    }
    if (typeof rawValue !== "string") {
      errors[variable.key] = "invalid_reference";
      continue;
    }
    const parsed = parseTemplateVariableDefault(variable.value_type, rawValue);
    if (parsed.error) {
      errors[variable.key] = parsed.error;
      continue;
    }

    if ((parsed.value === undefined || rawValue.trim() === "") && variable.required) {
      errors[variable.key] = "required";
      continue;
    }

    if (parsed.value !== undefined && rawValue.trim() !== "") {
      values[variable.key] = parsed.value;
    }
  }

  return { values, errors };
}

export function buildTemplatePreview(
  template: InventoryTemplate,
  variableValues: Record<string, unknown>,
): PreviewRow[] {
  const tokens = new Map<string, unknown>(Object.entries(variableValues));
  const rows: PreviewRow[] = [];

  for (const step of template.steps) {
    const repeat = resolveLocalRepeat(step.repeat, tokens);
    if (!repeat || repeat.count === 0) {
      tokens.set(`steps.${step.step_key}.ids`, []);
      continue;
    }

    const ids: string[] = [];
    for (let iteration = 0; iteration < repeat.count; iteration += 1) {
      const position = repeat.start + iteration;
      const effectiveStepKey = repeat.count === 1 ? step.step_key : `${step.step_key}[${position}]`;
      const iterationTokens = new Map(tokens);
      if (repeat.collection) {
        const { itemToken, items } = repeat.collection;
        for (const key of iterationTokens.keys()) {
          if (key === itemToken || key.startsWith(`${itemToken}.`)) iterationTokens.delete(key);
        }
        insertLocalItemTokens(iterationTokens, itemToken, items[iteration]);
      } else iterationTokens.set(repeat.indexToken, position);
      iterationTokens.set("position", position);
      insertCurrentIterationTokens(iterationTokens, position);

      rows.push({
        step_key: effectiveStepKey,
        title: step.title || step.step_key,
        resource_type: step.resource_type,
        payload: resolveTemplatePayload(step.payload_template, iterationTokens),
      });

      const previewId = `preview:${step.step_key}:${position}`;
      tokens.set(`steps.${step.step_key}.${position}.id`, previewId);
      tokens.set(`steps.${step.step_key}.${position}.ids`, [previewId]);
      if (ids.length === 0) tokens.set(`steps.${step.step_key}.id`, previewId);
      ids.push(previewId);
    }
    tokens.set(`steps.${step.step_key}.ids`, ids);
  }

  return rows;
}

function resolveLocalRepeat(
  repeat: unknown,
  tokens: Map<string, unknown>,
): { count: number; start: number; indexToken: string; collection?: { itemToken: string; items: unknown[] } } | null {
  if (!repeat || typeof repeat !== "object" || Array.isArray(repeat)) {
    return { count: 1, start: 1, indexToken: "position" };
  }
  const record = repeat as Record<string, unknown>;
  if (Object.hasOwn(record, "for_each")) {
    if (["count", "start", "index_token"].some((key) => Object.hasOwn(record, key))) return null;
    const itemToken = record.item_token === undefined ? "item" : record.item_token;
    if (typeof itemToken !== "string" || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(itemToken) || ["position", "steps"].includes(itemToken)) return null;
    if (!Array.isArray(record.for_each) && !(typeof record.for_each === "string" && isTemplatePlaceholder(record.for_each.trim()))) return null;
    const items = resolveTemplateValue(record.for_each, tokens);
    if (!Array.isArray(items) || items.length > 500) return null;
    return { count: items.length, start: 1, indexToken: "position", collection: { itemToken, items: items as unknown[] } };
  }
  if (Object.hasOwn(record, "item_token")) return null;
  const count = record.count === undefined ? 1 : resolveLocalInteger(record.count, tokens);
  const start = record.start === undefined ? 1 : resolveLocalInteger(record.start, tokens);
  if (count === undefined || count < 0 || count > 500 || start === undefined || !Number.isSafeInteger(start + count - 1)) return null;
  const indexToken = typeof record.index_token === "string" && record.index_token.trim()
    ? record.index_token.trim()
    : "position";
  return { count, start, indexToken };
}

function insertLocalItemTokens(tokens: Map<string, unknown>, token: string, value: unknown): void {
  tokens.set(token, value);
  if (value && typeof value === "object" && !Array.isArray(value)) {
    for (const [key, nested] of Object.entries(value)) insertLocalItemTokens(tokens, `${token}.${key}`, nested);
  }
}

function resolveLocalInteger(value: unknown, tokens: Map<string, unknown>): number | undefined {
  if (value === undefined || value === null) return undefined;
  const resolved = typeof value === "string" ? resolveTemplateValue(value, tokens) : value;
  if (typeof resolved === "number" && Number.isSafeInteger(resolved)) return resolved;
  if (typeof resolved === "string" && /^-?\d+$/.test(resolved.trim())) {
    const parsed = Number(resolved.trim());
    return Number.isSafeInteger(parsed) ? parsed : undefined;
  }
  return undefined;
}

function insertCurrentIterationTokens(tokens: Map<string, unknown>, position: number): void {
  const idSuffix = `.${position}.id`;
  const idsSuffix = `.${position}.ids`;
  const aliases: Array<[string, unknown]> = [];
  for (const [key, value] of tokens.entries()) {
    if (!key.startsWith("steps.")) continue;
    if (key.endsWith(idSuffix)) {
      aliases.push([`${key.slice(0, -idSuffix.length)}.current.id`, value]);
    } else if (key.endsWith(idsSuffix)) {
      aliases.push([`${key.slice(0, -idsSuffix.length)}.current.ids`, value]);
    }
  }
  for (const [key, value] of aliases) tokens.set(key, value);
}

function resolveTemplatePayload(
  value: Record<string, unknown>,
  tokens: Map<string, unknown>,
): Record<string, unknown> {
  return resolveTemplateValue(value, tokens) as Record<string, unknown>;
}

function resolveTemplateValue(value: unknown, tokens: Map<string, unknown>): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => resolveTemplateValue(item, tokens));
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, nested]) => [key, resolveTemplateValue(nested, tokens)]),
    );
  }

  if (typeof value !== "string") return value;
  if (!value.includes("{{")) return value;

  const trimmed = value.trim();
  if (isTemplatePlaceholder(trimmed)) {
    const token = trimmed.slice(2, -2).trim();
    const helper = resolveTemplateHelper(token, tokens);
    if (helper !== undefined) return helper;
    return tokens.has(token) ? tokens.get(token) : value;
  }

  let output = value;
  for (const [token, tokenValue] of tokens.entries()) {
    const replacement = typeof tokenValue === "string" ? tokenValue : JSON.stringify(tokenValue);
    output = output.split(`{{${token}}}`).join(replacement);
  }
  return output;
}

function resolveTemplateHelper(token: string, tokens: Map<string, unknown>): unknown {
  const parts = token.split(/\s+/).filter(Boolean);
  if (parts[0] !== "cidr_host" || (parts.length !== 3 && parts.length !== 4)) return undefined;
  const cidr = resolveTokenText(parts[1], tokens);
  const offset = resolveTokenInteger(parts[2], tokens);
  const index = parts[3] ? resolveTokenInteger(parts[3], tokens) : 1;
  if (!cidr || offset === undefined || index === undefined) return undefined;
  return cidrHost(cidr, offset + index - 1);
}

function resolveTokenText(token: string | undefined, tokens: Map<string, unknown>): string | undefined {
  if (!token) return undefined;
  const value = tokens.get(token);
  if (typeof value === "string") return value;
  if (value === undefined && token.includes("/")) return token;
  return undefined;
}

function resolveTokenInteger(token: string | undefined, tokens: Map<string, unknown>): number | undefined {
  if (!token) return undefined;
  const value = tokens.get(token) ?? token;
  if (typeof value === "number" && Number.isFinite(value)) return Math.trunc(value);
  if (typeof value === "string" && /^-?\d+$/.test(value.trim())) return Number.parseInt(value, 10);
  return undefined;
}

function cidrHost(cidr: string, hostOffset: number): string | undefined {
  if (!Number.isInteger(hostOffset) || hostOffset < 0) return undefined;
  const [address, prefixRaw] = cidr.trim().split("/");
  if (!address || !prefixRaw) return undefined;
  const octets = address.split(".").map((part) => Number.parseInt(part, 10));
  const prefix = Number.parseInt(prefixRaw, 10);
  if (octets.length !== 4 || octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
    return undefined;
  }
  if (!Number.isInteger(prefix) || prefix < 0 || prefix > 32) return undefined;
  const base = octets.reduce((acc, octet) => (acc << 8) + octet, 0) >>> 0;
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  const network = base & mask;
  const host = (network + hostOffset) >>> 0;
  if ((host & mask) !== network) return undefined;
  return [
    (host >>> 24) & 255,
    (host >>> 16) & 255,
    (host >>> 8) & 255,
    host & 255,
  ].join(".") + `/${prefix}`;
}

// ── Authoring validation ─────────────────────────────────────────────────────
//
// Centralized, field-addressable validation reused by the authoring editor.
// Each issue carries a stable `code` (translated by the UI), a `section`
// (used to scroll to the right place) and a precise `target` so the editor can
// focus the offending control. The legacy builder dialog inlined this logic;
// extracting it keeps the new editor honest and unit-testable.

export type TemplateIssueSection = "meta" | "variables" | "steps";

export interface TemplateIssue {
  section: TemplateIssueSection;
  /** Stable identifier for focus/scroll, e.g. `var-2-key`, `step-0-name`. */
  target: string;
  /** i18n key under `inventory.templates.editor.issues`. */
  code: string;
  /** Interpolation values for the message. */
  params?: Record<string, string | number>;
}

const VARIABLE_KEY_RE = /^[a-zA-Z_][a-zA-Z0-9_]*$/;
const STEP_KEY_RE = /^[a-z][a-z0-9_]*$/;
const SLUG_RE = /^[a-z0-9-]+$/;

export interface TemplateValidationResult {
  issues: TemplateIssue[];
  /** Normalized template ready to persist (only meaningful when `issues` is empty). */
  normalized: InventoryTemplate;
}

/**
 * Validate and normalize a draft template. Returns every issue found (not just
 * the first) so the editor can badge each section, plus a normalized copy that
 * is safe to send to the API when there are no issues.
 */
export function validateTemplate(template: InventoryTemplate, structuredDefaults: readonly boolean[] = []): TemplateValidationResult {
  const issues: TemplateIssue[] = [];

  if (!template.name.trim()) {
    issues.push({ section: "meta", target: "meta-name", code: "nameRequired" });
  }
  const slug = template.slug.trim();
  if (!slug) {
    issues.push({ section: "meta", target: "meta-slug", code: "slugRequired" });
  } else if (!SLUG_RE.test(slug)) {
    issues.push({ section: "meta", target: "meta-slug", code: "slugFormat" });
  }

  const seenVariables = new Set<string>();
  const normalizedVariables: TemplateVariable[] = template.variables.map((variable, index) => {
    const key = variable.key.trim();
    if (!key || !VARIABLE_KEY_RE.test(key)) {
      issues.push({
        section: "variables",
        target: `var-${index}-key`,
        code: "variableKeyInvalid",
        params: { key: key || "?" },
      });
    } else if (seenVariables.has(key)) {
      issues.push({
        section: "variables",
        target: `var-${index}-key`,
        code: "duplicateVariableKey",
        params: { key },
      });
    } else {
      seenVariables.add(key);
    }

    const parsedDefault = parseTemplateVariableDefault(
      variable.value_type,
      variable.default_value,
      templateReferenceFor(template, variable)?.resource,
      structuredDefaults[index] ?? false,
    );
    if (parsedDefault.error) {
      issues.push({
        section: "variables",
        target: `var-${index}-default`,
        code: parsedDefault.error,
        params: { key: key || "?" },
      });
    }

    return {
      key,
      label: variable.label.trim() || key,
      description: variable.description?.trim() || undefined,
      value_type: variable.value_type,
      required: !!variable.required,
      default_value: parsedDefault.value,
      validation_rules: normalizeVariableRules(template, variable),
      sort_order: index,
    } satisfies TemplateVariable;
  });

  if (template.steps.length === 0) {
    issues.push({ section: "steps", target: "steps-add", code: "noSteps" });
  }

  const normalizedSteps: TemplateStep[] = template.steps.map((step, index) => {
    const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[step.resource_type];
    const stepKey = slugifyTemplateKey(step.step_key, definition.defaultStepKey);

    if (!STEP_KEY_RE.test(stepKey)) {
      issues.push({
        section: "steps",
        target: `step-${index}-key`,
        code: "invalidStepKey",
        params: { key: step.step_key || "?" },
      });
    }

    const payloadTemplate = { ...step.payload_template };
    const repeat = step.repeat && typeof step.repeat === "object" && !Array.isArray(step.repeat)
      ? step.repeat
      : {};
    for (const field of definition.payloadFields) {
      const value = payloadTemplate[field.key];
      if (field.required && (value === undefined || value === null || value === "")) {
        issues.push({
          section: "steps",
          target: `step-${index}-field-${field.key}`,
          code: "requiredStepField",
          params: { field: field.label, step: stepKey },
        });
      } else if (!isConfiguredValueCompatible(value, field.valueKind)) {
        issues.push({
          section: "steps",
          target: `step-${index}-field-${field.key}`,
          code: "invalidStepField",
          params: { field: field.label, step: stepKey },
        });
      }
    }

    return {
      step_key: stepKey,
      title: step.title.trim() || stepKey,
      description: step.description?.trim() || undefined,
      resource_type: step.resource_type,
      operation: "create",
      depends_on_step_keys: step.depends_on_step_keys.map((dep) => slugifyTemplateKey(dep, dep)),
      payload_template: payloadTemplate,
      repeat,
      on_conflict_policy: step.on_conflict_policy,
      sort_order: index,
    } satisfies TemplateStep;
  });

  // Step-key uniqueness + dependency integrity.
  const seenStepKeys = new Set<string>();
  normalizedSteps.forEach((step, index) => {
    if (seenStepKeys.has(step.step_key)) {
      issues.push({
        section: "steps",
        target: `step-${index}-key`,
        code: "duplicateStepKey",
        params: { key: step.step_key },
      });
    }
    seenStepKeys.add(step.step_key);
  });
  analyzeTemplateStepOrder(normalizedSteps).forEach(violation => {
    issues.push({
      section: "steps",
      target: `step-${violation.consumerIndex}-deps`,
      code: violation.reason === "selfReference" ? "selfDependency"
        : violation.reason === "missingProducer" ? "unknownDependency" : "dependencyOrder",
      params: { key: violation.producerKey, step: violation.consumerKey, dependency: violation.producerKey },
    });
  });

  return {
    issues,
    normalized: {
      ...template,
      name: template.name.trim(),
      slug,
      description: template.description.trim(),
      variables: normalizedVariables,
      steps: normalizedSteps,
    },
  };
}

export function createEmptyTemplateVariable(): TemplateVariable {
  return {
    key: "",
    label: "",
    value_type: "string",
    required: false,
    validation_rules: {},
  };
}

function firstTemplateResource() {
  const first = TEMPLATE_RESOURCE_DEFINITIONS[0];
  if (!first) throw new Error("TEMPLATE_RESOURCE_DEFINITIONS must not be empty");
  return first;
}

export function createTemplateStep(
  resourceType: TemplateStepResourceType = firstTemplateResource().resourceType,
): TemplateStep {
  const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[resourceType];
  return {
    step_key: definition.defaultStepKey,
    title: definition.defaultTitle,
    resource_type: definition.resourceType,
    operation: "create",
    depends_on_step_keys: [],
    payload_template: {},
    repeat: {},
    on_conflict_policy: "fail",
  };
}

// ── Reference auto-binding ────────────────────────────────────────────────────
//
// When a step needs a required foreign key (e.g. device.site_id), the author
// should not have to hand-type `{{steps.<key>.id}}`. We resolve each empty
// required FK either to the nearest prior step of the matching resource type,
// or — when no such step exists — to a generated run variable the run wizard
// will prompt for. This is the fix for the original bug where a `site_id` field
// held a stale `{{steps.site.id}}` hint that never matched the real step key.

/** Humanize a snake_case field key into a label, e.g. `role_id` → `Role`. */
function humanizeReferenceFieldKey(key: string): string {
  const base = key.endsWith("_id") ? key.slice(0, -3) : key;
  return base
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

/**
 * Fill currently-empty REQUIRED reference fields on the step at `stepIndex`.
 * Manual (non-empty) values always win. Each filled field either links to the
 * nearest prior step of the referenced resource type, or falls back to a
 * generated required `resource_ref` run variable keyed by the field key.
 *
 * Pure and immutable: returns fresh `steps`/`variables` arrays.
 */
export function autoBindStepReferences(
  template: InventoryTemplate,
  stepIndex: number,
): { steps: TemplateStep[]; variables: TemplateVariable[] } {
  const target = template.steps[stepIndex];
  if (!target) {
    return { steps: template.steps, variables: template.variables };
  }

  const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[target.resource_type];
  const nextPayload = { ...target.payload_template };
  const nextDeps = [...target.depends_on_step_keys];
  const nextVariables = [...template.variables];

  for (const field of definition.payloadFields) {
    if (!field.required || !field.references) continue;

    const existing = nextPayload[field.key];
    const isEmpty = existing === undefined || existing === null || existing === "";
    if (!isEmpty) continue;

    const priorStep = template.steps
      .slice(0, stepIndex)
      .filter((step) => step.resource_type === field.references && step.step_key.trim())
      .at(-1);

    if (priorStep) {
      nextPayload[field.key] = `{{steps.${priorStep.step_key}.id}}`;
      if (!nextDeps.includes(priorStep.step_key)) nextDeps.push(priorStep.step_key);
    } else {
      if (!nextVariables.some((variable) => variable.key === field.key)) {
        nextVariables.push({
          key: field.key,
          label: humanizeReferenceFieldKey(field.key),
          value_type: "resource_ref",
          required: true,
          // Declare the referenced resource so the run wizard can offer the
          // inventory picker for the generated variable.
          validation_rules: { resource: field.references },
        });
      }
      nextPayload[field.key] = `{{${field.key}}}`;
    }
  }

  const nextSteps = template.steps.map((step, index) =>
    index === stepIndex
      ? { ...step, payload_template: nextPayload, depends_on_step_keys: nextDeps }
      : step,
  );

  return { steps: nextSteps, variables: nextVariables };
}

/**
 * Cascade a step-key rename across all OTHER steps: rewrite any payload value
 * that is exactly `{{steps.<oldKey>.id}}` and rename matching `depends_on`
 * entries. Pure/immutable.
 */
export function renameStepReferences(
  steps: TemplateStep[],
  oldKey: string,
  newKey: string,
): TemplateStep[] {
  if (oldKey === newKey) return steps;
  const oldToken = `{{steps.${oldKey}.id}}`;
  const newToken = `{{steps.${newKey}.id}}`;
  return steps.map((step) => {
    if (step.step_key === oldKey) return step;
    const payload = Object.fromEntries(
      Object.entries(step.payload_template).map(([key, value]) => [
        key,
        value === oldToken ? newToken : value,
      ]),
    );
    const deps = step.depends_on_step_keys.map((dep) => (dep === oldKey ? newKey : dep));
    return { ...step, payload_template: payload, depends_on_step_keys: deps };
  });
}

/**
 * Clear references to a removed step from all remaining steps: blank any
 * payload value that is exactly `{{steps.<removedKey>.id}}` so validation
 * re-flags the now-missing required field. Pure/immutable.
 */
export function clearStepReferences(steps: TemplateStep[], removedKey: string): TemplateStep[] {
  const token = `{{steps.${removedKey}.id}}`;
  return steps.map((step) => {
    const payload = Object.fromEntries(
      Object.entries(step.payload_template).map(([key, value]) => [
        key,
        value === token ? "" : value,
      ]),
    );
    return { ...step, payload_template: payload };
  });
}
