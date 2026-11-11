import type { Dataflow } from "@/features/inventory/dataflows-api";
import type { AddressGroup, Application, ApplicationRole, DataflowGroup } from "@/features/inventory/dataflowCatalog-api";

type DemoEndpoint = Dataflow["sources"][number];
const endpoint = (object_type: DemoEndpoint["object_type"], object_id: string | null, label: string, address_mode: DemoEndpoint["address_mode"] = "default"): DemoEndpoint => ({
  object_type, object_id, label, address_mode, ipaddresses: [],
});

const CORE_TAG = "00000000-0000-4000-8000-000000009401";
const CAMPUS_TAG = "00000000-0000-4000-8000-000000009402";

export const DEMO_ROLE_PLATFORM = "role-app-platform-0000-0000-000000000001";
export const DEMO_ROLE_SECURITY = "role-app-security-0000-0000-000000000002";
export const DEMO_APP_WEB = "00000000-0000-4000-8000-000000002101";
export const DEMO_APP_DATA = "00000000-0000-4000-8000-000000002102";
export const DEMO_APP_OPS = "00000000-0000-4000-8000-000000002103";
export const DEMO_APPLICATION_CONTENT_TYPE = "00000000-0000-4000-8000-000000002111";
export const DEMO_GROUP_PROD = "group-prod-0000-0000-0000-000000000001";
export const DEMO_GROUP_EDGE = "group-edge-0000-0000-0000-000000000002";
export const DEMO_GROUP_LEGACY = "group-legacy-0000-0000-0000-000000000003";
export const DEMO_ADDRESS_WEB = "addrgroup-web-0000-0000-000000000001";
export const DEMO_ADDRESS_DB = "addrgroup-db-0000-0000-000000000002";

export const demoApplicationRoles: ApplicationRole[] = [
  { id: DEMO_ROLE_PLATFORM, name: "Platform", description: "Runtime and web platform ownership", comments: "", owner_id: null },
  { id: DEMO_ROLE_SECURITY, name: "Security", description: "Security tooling and controls", comments: "", owner_id: null },
];

export const demoApplications: Application[] = [
  { id: DEMO_APP_WEB, name: "Web platform", role_id: DEMO_ROLE_PLATFORM, description: "Public and internal web traffic", comments: "", owner_id: null, tenant_id: null, tags: [CORE_TAG], custom_field_data: {} },
  { id: DEMO_APP_DATA, name: "Core data", role_id: DEMO_ROLE_PLATFORM, description: "Database and storage access", comments: "", owner_id: null, tenant_id: null, tags: [CORE_TAG], custom_field_data: {} },
  { id: DEMO_APP_OPS, name: "Operations", role_id: DEMO_ROLE_SECURITY, description: "Monitoring and management traffic", comments: "", owner_id: null, tenant_id: null, tags: [], custom_field_data: {} },
];

export const demoDataflowGroups: DataflowGroup[] = [
  { id: DEMO_GROUP_PROD, parent_id: null, application_id: null, name: "Production", status: "active", effective_status: "active", blocked_by_group_id: null, inherited_tags: [], effective_tags: [CORE_TAG], comments: "", owner_id: null, tenant_id: null, tags: [CORE_TAG], custom_field_data: {} },
  { id: DEMO_GROUP_EDGE, parent_id: DEMO_GROUP_PROD, application_id: DEMO_APP_WEB, name: "Edge", status: "active", effective_status: "active", blocked_by_group_id: null, inherited_tags: [CORE_TAG], effective_tags: [CORE_TAG, CAMPUS_TAG], comments: "", owner_id: null, tenant_id: null, tags: [CAMPUS_TAG], custom_field_data: {} },
  { id: DEMO_GROUP_LEGACY, parent_id: null, application_id: null, name: "Paused legacy", status: "disabled", effective_status: "disabled", blocked_by_group_id: null, inherited_tags: [], effective_tags: [], comments: "", owner_id: null, tenant_id: null, tags: [], custom_field_data: {} },
];

const web = endpoint("dcim.device", "00000000-0000-4000-8001-000000000001", "web-prod-01");
const database = endpoint("dcim.device", "00000000-0000-4000-8001-000000000002", "db-primary-01");
const application = endpoint("virtualization.virtualmachine", "00000000-0000-4000-8001-000000000003", "app-node-02");
const router = endpoint("dcim.device", "00000000-0000-4000-8001-000000000005", "edge-router-01");
const servers = endpoint("dcim.devicerole", "role-srv-0000-0000-0000-000000000001", "Linux Server");
const site = endpoint("dcim.site", "site-ams-0000-0000-0000-000000000001", "Amsterdam Edge");
const internet = endpoint("internet", null, "Internet");
const taggedDevices = { ...endpoint("extras.tag", "00000000-0000-4000-8000-000000009401", "Core network"), tag_object_type: "dcim.device" as const };
const databaseRange = endpoint("ipam.iprange", "ip-range-demo-0000-0000-000000000001", "198.51.100.0/24");
const webGroup = endpoint("inventory.addressgroup", DEMO_ADDRESS_WEB, "Web fleet");

export const demoAddressGroups: AddressGroup[] = [
  { id: DEMO_ADDRESS_WEB, name: "Web fleet", members: [web, application].map((member, position) => ({ ...member, position })), comments: "", owner_id: null, tenant_id: null, tags: [CORE_TAG], custom_field_data: {} },
  { id: DEMO_ADDRESS_DB, name: "Database targets", members: [database, databaseRange].map((member, position) => ({ ...member, position })), comments: "", owner_id: null, tenant_id: null, tags: [], custom_field_data: {} },
];

const patterns: Array<{ name: string; sources: DemoEndpoint[]; destinations: DemoEndpoint[]; port: number | null; transport?: Dataflow["transport"]; application: string; group: string | null }> = [
  { name: "Web application", sources: [web], destinations: [application], port: 443, application: DEMO_APP_WEB, group: DEMO_GROUP_EDGE },
  { name: "Application database", sources: [application, web], destinations: [database], port: 5432, application: DEMO_APP_DATA, group: DEMO_GROUP_PROD },
  { name: "Database health check", sources: [database], destinations: [database], port: 5432, application: DEMO_APP_DATA, group: null },
  { name: "Application callback", sources: [application], destinations: [web], port: 8443, application: DEMO_APP_WEB, group: DEMO_GROUP_EDGE },
  { name: "Server metrics", sources: [application], destinations: [servers], port: 9100, application: DEMO_APP_OPS, group: null },
  { name: "Site DNS", sources: [site], destinations: [router], port: 53, transport: "udp", application: DEMO_APP_OPS, group: null },
  { name: "Time synchronization", sources: [servers], destinations: [router], port: 123, transport: "udp", application: DEMO_APP_OPS, group: null },
  { name: "Package downloads", sources: [servers], destinations: [internet], port: 443, application: DEMO_APP_OPS, group: null },
  { name: "Web fleet ingress", sources: [taggedDevices], destinations: [webGroup], port: 443, application: DEMO_APP_WEB, group: DEMO_GROUP_EDGE },
  { name: "Database range access", sources: [application], destinations: [databaseRange], port: 5432, application: DEMO_APP_DATA, group: DEMO_GROUP_PROD },
  { name: "ICMPv6 diagnostics", sources: [servers], destinations: [taggedDevices], port: null, transport: "icmpv6", application: DEMO_APP_OPS, group: null },
  { name: "Legacy replication", sources: [database], destinations: [database], port: 5432, application: DEMO_APP_DATA, group: DEMO_GROUP_LEGACY },
];

/** Synthetic directed traffic, including parallel, reverse, self, tag, range and multi-endpoint flows. */
export function createDemoDataflows(count = 24): Dataflow[] {
  return Array.from({ length: count }, (_, index) => {
    const pattern = patterns[index % patterns.length]!;
    const round = Math.floor(index / patterns.length);
    const status = pattern.group === DEMO_GROUP_LEGACY ? "disabled" : (["active", "draft", "disabled"] as const)[round % 3]!;
    const effectiveStatus = pattern.group === DEMO_GROUP_LEGACY ? "disabled" : status;
    const inheritedTags = pattern.group === DEMO_GROUP_EDGE ? [CORE_TAG] : [];
    const directTags = pattern.group === DEMO_GROUP_EDGE ? [CAMPUS_TAG] : pattern.group === DEMO_GROUP_PROD ? [CORE_TAG] : [];
    const destination = index < 24
      ? (round === 0 ? pattern.destinations : pattern.destinations.map(item => ({ ...item })))
      : [endpoint("dcim.device", `demo-edge-${index}`, `Edge node ${String(index).padStart(3, "0")}`)];
    return {
      id: `demo-dataflow-${index}`, name: `${pattern.name} ${round + 1}`,
      description: "Synthetic demo traffic. This is not a captured production connection or an applied firewall rule.",
      status, effective_status: effectiveStatus, blocked_by_group_id: pattern.group === DEMO_GROUP_LEGACY ? DEMO_GROUP_LEGACY : null,
      sources: pattern.sources.map(item => ({ ...item })), destinations: destination,
      transport: pattern.transport ?? "tcp", destination_ports: pattern.port === null ? null : [pattern.port],
      source_ports: index === 1 ? [1024, 2048] : null,
      icmp_types: pattern.transport === "icmpv6" ? [128] : null,
      allow_cross_vrf: false,
      application_id: pattern.application, group_id: pattern.group,
      comments: "", owner_id: null, tenant_id: null,
      tags: directTags, inherited_tags: inheritedTags, effective_tags: [...new Set([...directTags, ...inheritedTags])].sort(),
      custom_field_data: {},
    };
  });
}
