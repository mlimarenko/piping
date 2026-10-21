/**
 * MSW request handlers for demo mode.
 *
 * These reproduce the CURRENT backend response envelopes consumed by the
 * connections workspace + inventory tree + launch-options, captured
 * from the live seeded backend (http://127.0.0.1:9080) so the mocked shapes are
 * guaranteed correct against current `main`. Data is intentionally plausible
 * (Amsterdam Edge / Frankfurt DC sites, Linux servers, a Windows RDP jump host,
 * a VNC lab box, a router) so the embedded product demo looks like a working
 * install.
 *
 * Scope: every endpoint the demoed screens read. Mutations return a benign
 * echo/no-op so the UI never errors, but demo data is not persisted.
 */
import { http, HttpResponse } from "msw";
import type { components } from "@/shared/api/schema.gen";
// Bundle the REAL OpenAPI contract into the demo build so the API reference
// (Swagger UI) renders the actual product surface. `?raw` ships the file verbatim
// as a string; Swagger UI parses the YAML client-side once it has a valid
// `openapi: 3.x` document with the right content-type. The path resolves up out
// of apps/web into the sibling apps/api contracts dir (inside the pnpm workspace
// root, so Vite's fs.allow covers it).
import openapiYaml from "../../../api/contracts/piping.openapi.yaml?raw";
import { cctvClipBytes, cctvFrameBytes } from "./cctv-media";
import { demoLanguageFromLocation } from "./language";
import {
  findById,
  insert,
  isCollection,
  listEnvelope,
  remove,
  rows,
  update,
} from "./store";

const NOW = Date.parse("2026-06-02T11:20:00Z");
const iso = (minutesAgo: number) => new Date(NOW - minutesAgo * 60_000).toISOString();

/** Parse limit/offset/search from a request URL for store list queries. */
function listOpts(url: URL): { limit?: number; offset?: number; search?: string } {
  const num = (k: string): number | undefined => {
    const v = url.searchParams.get(k);
    if (v === null) return undefined;
    const n = Number(v);
    return Number.isFinite(n) ? n : undefined;
  };
  const search = url.searchParams.get("search") ?? undefined;
  return { limit: num("limit"), offset: num("offset"), search: search || undefined };
}

/** Shape a list response for a store key using the canonical inventory envelope. */
function listResponse(key: string, url: URL): Response {
  return HttpResponse.json(listEnvelope(key, listOpts(url)));
}

function auditTargetsResponse(request: Request): Response {
  const url = new URL(request.url);
  const targetType = url.searchParams.get("target_type") || undefined;
  const q = (url.searchParams.get("search") ?? "").trim().toLowerCase();
  const limit = Number(url.searchParams.get("limit") ?? 30);
  const max = Number.isFinite(limit) ? Math.min(Math.max(limit, 1), 50) : 30;
  const items: Array<{
    target_type: string;
    kind: string;
    id: string;
    label: string;
    secondary?: string;
  }> = [];
  const matches = (...values: unknown[]) => {
    if (!q) return true;
    return values.some((value) => typeof value === "string" && value.toLowerCase().includes(q));
  };
  const include = (type: string) =>
    !targetType ||
    targetType === "all" ||
    targetType === type ||
    (targetType === "connection_target" && (type === "device" || type === "virtual_machine"));

  const str = (v: unknown): string => (typeof v === "string" ? v : "");

  if (include("user")) {
    for (const user of rows("admin/access-control/users")) {
      const username = str(user.username);
      const email = str(user.email);
      if (matches(username, email, user.id, user.status)) {
        items.push({
          target_type: "user",
          kind: "user",
          id: user.id,
          label: username || email,
          secondary: [email, user.status].filter(Boolean).join(" · "),
        });
      }
    }
  }

  if (include("user_api_token")) {
    for (const token of rows("user/settings/api-tokens")) {
      const name = str(token.name);
      if (matches(name, token.token_prefix, token.status, token.id)) {
        items.push({
          target_type: "user_api_token",
          kind: "api_token",
          id: token.id,
          label: name,
          secondary: [token.token_prefix, token.status].filter(Boolean).join(" · "),
        });
      }
    }
  }

  if (include("connection_config:record")) {
    for (const record of rows("connection-config/records")) {
      const name = str(record.name);
      if (matches(name, record.protocol, record.id)) {
        items.push({
          target_type: "connection_config:record",
          kind: "connection_config_record",
          id: record.id,
          label: name,
          secondary: str(record.protocol),
        });
      }
    }
  }

  if (include("connection_session")) {
    for (const session of ACTIVE_SESSIONS) {
      const host = HOSTS.find((item) => item.id === session.target_id);
      const protocol = String(session.protocol ?? "").toUpperCase();
      if (matches(protocol, session.status, session.id, host?.name)) {
        items.push({
          target_type: "connection_session",
          kind: "connection_session",
          id: session.id,
          label: host ? `${protocol} · ${host.name}` : protocol,
          secondary: String(session.status ?? ""),
        });
      }
    }
  }

  if (include("device") || include("virtual_machine")) {
    for (const host of HOSTS) {
      const type = host.target_kind === "virtual_machine" ? "virtual_machine" : "device";
      if (!include(type)) continue;
      if (matches(host.name, host.address, host.status, host.id)) {
        items.push({
          target_type: targetType === "connection_target" ? "connection_target" : type,
          kind: type,
          id: host.id,
          label: host.name,
          secondary: [host.status, host.address].filter(Boolean).join(" · "),
        });
      }
    }
  }

  return HttpResponse.json({ items: items.slice(0, max), limit: max });
}

async function readBody(request: Request): Promise<Record<string, unknown>> {
  try {
    const body = (await request.clone().json()) as unknown;
    if (body && typeof body === "object" && !Array.isArray(body)) {
      return body as Record<string, unknown>;
    }
  } catch {
    // Empty / non-JSON body — treat as no fields.
  }
  return {};
}

const envelope = <T>(items: T[]) => ({
  items,
  total: items.length,
  limit: 1000,
  offset: 0,
});

function demoPrefixTreeResponse(request: Request): Response {
  const url = new URL(request.url);
  const search = (url.searchParams.get("search") ?? "").trim().toLowerCase();
  const family = url.searchParams.get("family");
  const status = url.searchParams.get("status");
  const rootPrefix = url.searchParams.get("root_prefix");
  const limit = Math.min(Math.max(Number(url.searchParams.get("limit") ?? 300), 1), 500);
  const nodes = [
    {
      id: "px-0000-0000-0000-000000000001",
      node_id: "prefix:px-0000-0000-0000-000000000001",
      prefix: "192.0.2.0/24",
      parent_id: null,
      parent_prefix: null,
      root_id: null,
      root_prefix: rootPrefix,
      depth: 0,
      child_count: 2,
      status: "container",
      family: 4,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 5,
      usable_addresses: 254,
      utilization_percent: 2.0,
      breadcrumbs: ["192.0.2.0/24"],
    },
    {
      id: "px-0000-0000-0000-000000000011",
      node_id: "prefix:px-0000-0000-0000-000000000011",
      prefix: "192.0.2.0/25",
      parent_id: "px-0000-0000-0000-000000000001",
      parent_prefix: "192.0.2.0/24",
      root_id: "px-0000-0000-0000-000000000001",
      root_prefix: "192.0.2.0/24",
      depth: 1,
      child_count: 0,
      status: "active",
      family: 4,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: true,
      utilized_addresses: 120,
      usable_addresses: 126,
      utilization_percent: 100,
      breadcrumbs: ["192.0.2.0/24", "192.0.2.0/25"],
    },
    {
      id: "px-0000-0000-0000-000000000012",
      node_id: "prefix:px-0000-0000-0000-000000000012",
      prefix: "192.0.2.128/25",
      parent_id: "px-0000-0000-0000-000000000001",
      parent_prefix: "192.0.2.0/24",
      root_id: "px-0000-0000-0000-000000000001",
      root_prefix: "192.0.2.0/24",
      depth: 1,
      child_count: 0,
      status: "reserved",
      family: 4,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 1,
      usable_addresses: 126,
      utilization_percent: 0.8,
      breadcrumbs: ["192.0.2.0/24", "192.0.2.128/25"],
    },
    {
      id: "px-0000-0000-0000-000000000002",
      node_id: "prefix:px-0000-0000-0000-000000000002",
      prefix: "198.51.100.0/24",
      parent_id: null,
      parent_prefix: null,
      root_id: null,
      root_prefix: rootPrefix,
      depth: 0,
      child_count: 0,
      status: "active",
      family: 4,
      site_id: SITE_FRA.id,
      site_name: SITE_FRA.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 4,
      usable_addresses: 254,
      utilization_percent: 1.6,
      breadcrumbs: ["198.51.100.0/24"],
    },
    {
      id: "px-0000-0000-0000-000000000006",
      node_id: "prefix:px-0000-0000-0000-000000000006",
      prefix: "2001:db8:20::/64",
      parent_id: null,
      parent_prefix: null,
      root_id: null,
      root_prefix: rootPrefix,
      depth: 0,
      child_count: 0,
      status: "active",
      family: 6,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 0,
      usable_addresses: null,
      utilization_percent: null,
      breadcrumbs: ["2001:db8:20::/64"],
    },
  ];
  const filtered = nodes.filter((node) => {
    if (family && String(node.family) !== family) return false;
    if (status && !status.split(",").includes(node.status)) return false;
    if (rootPrefix && !node.prefix.startsWith(rootPrefix.split("/")[0] ?? rootPrefix)) return false;
    if (!search) return true;
    return [node.prefix, node.status, node.site_name, node.tenant_name]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(search));
  });
  return HttpResponse.json({
    nodes: filtered.slice(0, limit),
    total: filtered.length,
    limit,
    applied_filters: {
      search: search || null,
      root_id: null,
      root_prefix: rootPrefix,
      family: family ? Number(family) : null,
      site_id: [],
      vrf_id: [],
      tenant_id: [],
      status: status ? status.split(",") : [],
      limit,
    },
  });
}

function demoPrefixPlanningPreview(request: Request): Response {
  const url = new URL(request.url);
  const prefixLength = Number(url.searchParams.get("prefix_length") ?? 26);
  return HttpResponse.json({
    parent_id: "px-0000-0000-0000-000000000001",
    parent_prefix: "192.0.2.0/24",
    requested_prefix_length: prefixLength,
    status: prefixLength > 32 ? "invalid" : "available",
    candidate_prefix: prefixLength > 32 ? null : "192.0.2.64/26",
    reason: prefixLength > 32 ? "invalid_prefix_length" : null,
    conflicts: [],
    searched_candidates: 2,
  });
}

const CABLING_DEMO_ENDPOINTS = [
  {
    endpoint: { kind: "interface", id: "cb-iface-0000-0000-0000-000000000001", label: "edge-router-01 · ge-0/0/1" },
    parent: { entity: "Devices", id: "host-0000-0000-0000-0000-000000000005", label: "edge-router-01" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "cabled",
    trace_state: "complete",
    cable: { id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001", status: "connected", color: "2f80ed" },
    related_links: [
      { entity: "Devices", id: "host-0000-0000-0000-0000-000000000005", label: "edge-router-01" },
      { entity: "Cables", id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001" },
    ],
  },
  {
    endpoint: { kind: "interface", id: "cb-iface-0000-0000-0000-000000000002", label: "web-prod-01 · eth0" },
    parent: { entity: "Devices", id: "host-0000-0000-0000-0000-000000000001", label: "web-prod-01" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "cabled",
    trace_state: "complete",
    cable: { id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001", status: "connected", color: "2f80ed" },
    related_links: [
      { entity: "Devices", id: "host-0000-0000-0000-0000-000000000001", label: "web-prod-01" },
      { entity: "Cables", id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001" },
    ],
  },
  {
    endpoint: { kind: "front-port", id: "cb-front-0000-0000-0000-000000000001", label: "patch-panel-01 · F01" },
    parent: { entity: "Devices", id: "device-patch-0000-0000-0000-0000000001", label: "patch-panel-01" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "marked-connected",
    trace_state: "incomplete",
    cable: null,
    related_links: [{ entity: "Devices", id: "device-patch-0000-0000-0000-0000000001", label: "patch-panel-01" }],
  },
  {
    endpoint: { kind: "power-outlet", id: "cb-pout-0000-0000-0000-000000000001", label: "pdu-a · outlet 08" },
    parent: { entity: "Devices", id: "device-pdu-0000-0000-0000-00000000001", label: "pdu-a" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "available",
    trace_state: "unsupported",
    cable: null,
    related_links: [{ entity: "Devices", id: "device-pdu-0000-0000-0000-00000000001", label: "pdu-a" }],
  },
] as const;

function demoCablingEndpoints(request: Request): Response {
  const url = new URL(request.url);
  const query = (url.searchParams.get("query") ?? "").trim().toLowerCase();
  const endpointKinds = new Set((url.searchParams.get("endpoint_kind") ?? "").split(",").filter(Boolean));
  const linkStates = new Set((url.searchParams.get("link_state") ?? "").split(",").filter(Boolean));
  const traceStates = new Set((url.searchParams.get("trace_state") ?? "").split(",").filter(Boolean));
  const issuesOnly = url.searchParams.get("issues_only") === "true";
  const limit = Math.min(Math.max(Number(url.searchParams.get("limit") ?? 50), 1), 100);
  const filtered = CABLING_DEMO_ENDPOINTS.filter((item) => {
    if (query && ![item.endpoint.label, item.parent?.label, item.cable?.label].some((value) => value?.toLowerCase().includes(query))) {
      return false;
    }
    if (endpointKinds.size > 0 && !endpointKinds.has(item.endpoint.kind)) return false;
    if (linkStates.size > 0 && !linkStates.has(item.link_state)) return false;
    if (traceStates.size > 0 && !traceStates.has(item.trace_state)) return false;
    if (issuesOnly && item.trace_state === "complete") return false;
    return true;
  });
  return HttpResponse.json({ items: filtered.slice(0, limit), total: filtered.length, limit, offset: 0 });
}

function demoCablingTrace(request: Request): Response {
  const url = new URL(request.url);
  const endpointId = url.searchParams.get("endpoint_id");
  const start = CABLING_DEMO_ENDPOINTS.find((item) => item.endpoint.id === endpointId) ?? CABLING_DEMO_ENDPOINTS[0];
  const peer = start.cable
    ? CABLING_DEMO_ENDPOINTS.find((item) => item.cable?.id === start.cable?.id && item.endpoint.id !== start.endpoint.id)
    : undefined;
  const state = start.trace_state;
  const issues = state === "complete" ? [] : [
    {
      code: state === "unsupported" ? "unsupported-endpoint" : "marked-connected",
      endpoint: start.endpoint,
      message: state === "unsupported"
        ? "Demo endpoint kind is indexed but not traversable."
        : "Endpoint is marked connected without a cable reference.",
    },
  ];
  return HttpResponse.json({
    start: start.endpoint,
    state,
    summary: { hop_count: peer ? 1 : 0, cable_count: start.cable ? 1 : 0, issue_count: issues.length },
    segments: peer
      ? [{ index: 0, traversal: "cable", from: start.endpoint, to: peer.endpoint, cable: start.cable, note: null }]
      : [],
    issues,
    related_links: start.related_links,
    generated_at: iso(0),
  });
}

// ── Sites / tenants / roles ──────────────────────────────────────────────────
const SITE_AMS = {
  id: "site-ams-0000-0000-0000-000000000001",
  name: "Amsterdam Edge",
};
const SITE_FRA = {
  id: "site-fra-0000-0000-0000-000000000002",
  name: "Frankfurt DC",
};
const TENANT_CORE = { id: "ten-core-0000-0000-0000-000000000001", name: "Core Infrastructure" };
const TENANT_CORP = { id: "ten-corp-0000-0000-0000-000000000002", name: "Corporate IT" };

const ROLE_SERVER = {
  id: "role-srv-0000-0000-0000-000000000001",
  name: "Linux Server",
  custom_fields: { piping_icon: { name: "server" } },
};
const ROLE_JUMP = {
  id: "role-jump-0000-0000-0000-00000000002",
  name: "Jump Host",
  custom_fields: { piping_icon: { name: "monitor" } },
};
const ROLE_LAB = {
  id: "role-lab-0000-0000-0000-000000000003",
  name: "Lab Workstation",
  custom_fields: { piping_icon: { name: "laptop" } },
};
const ROLE_ROUTER = {
  id: "role-rtr-0000-0000-0000-000000000004",
  name: "Router",
  custom_fields: { piping_icon: { name: "router" } },
};
const ROLE_CAMERA = {
  id: "role-cam-0000-0000-0000-000000000005",
  name: "CCTV Recorder",
  custom_fields: { piping_icon: { name: "cctv" } },
};

const DEVICE_TYPE = { id: "dt-server-0000-0000-0000-00000000001", name: "Generic Server" };

// ── Connection targets (Connections workspace + target tree) ─────────────────
// target_kind identifies the backing inventory source: device or virtual_machine.
interface DemoHost {
  id: string;
  name: string;
  address: string;
  target_kind: "device" | "virtual_machine";
  status: string;
  device_id: string | null;
  device_name: string | null;
  vm_id: string | null;
  vm_name: string | null;
  tenant_id: string | null;
  tenant_name: string | null;
  site_id: string | null;
  site_name: string | null;
  region_id: string | null;
  region_name: string | null;
  role_id: string;
  role_name: string;
  role_custom_fields: Record<string, unknown> | null;
  platform_id: string | null;
  platform_name: string | null;
  device_type_id: string | null;
  device_type_name: string | null;
  cluster_id: string | null;
  cluster_name: string | null;
  // Drives the CCTV badge in the tree/cards and the CCTV console entry point.
  // The read model sets it from the device family, not from the protocol.
  is_cctv: boolean;
  family: string | null;
}

function device(
  id: string,
  name: string,
  address: string,
  site: { id: string; name: string },
  tenant: { id: string; name: string },
  role: { id: string; name: string; custom_fields: Record<string, unknown> },
): DemoHost {
  return {
    id,
    name,
    address,
    target_kind: "device",
    status: "active",
    device_id: id,
    device_name: name,
    vm_id: null,
    vm_name: null,
    tenant_id: tenant.id,
    tenant_name: tenant.name,
    site_id: site.id,
    site_name: site.name,
    region_id: null,
    region_name: null,
    role_id: role.id,
    role_name: role.name,
    role_custom_fields: role.custom_fields,
    platform_id: null,
    platform_name: null,
    device_type_id: DEVICE_TYPE.id,
    device_type_name: DEVICE_TYPE.name,
    cluster_id: null,
    cluster_name: null,
    is_cctv: false,
    family: null,
  };
}

function vm(
  id: string,
  name: string,
  address: string,
  site: { id: string; name: string },
  tenant: { id: string; name: string },
  role: { id: string; name: string; custom_fields: Record<string, unknown> },
): DemoHost {
  return {
    ...device(id, name, address, site, tenant, role),
    target_kind: "virtual_machine",
    device_id: null,
    device_name: null,
    vm_id: id,
    vm_name: name,
  };
}

// A Linux host carries the non-video `linux` family: it is not cctv, yet the
// neutral device-users plane manages its local accounts, so the workspace shows
// the Accounts entry beside its SSH button.
const H_WEB: DemoHost = {
  ...device("host-0000-0000-0000-0000-000000000001", "web-prod-01", "192.0.2.11", SITE_AMS, TENANT_CORE, ROLE_SERVER),
  family: "linux",
};
const H_DB = device("host-0000-0000-0000-0000-000000000002", "db-primary-01", "192.0.2.21", SITE_AMS, TENANT_CORE, ROLE_SERVER);
const H_APP = { ...vm("host-0000-0000-0000-0000-000000000003", "app-node-02", "192.0.2.32", SITE_AMS, TENANT_CORE, ROLE_SERVER), site_id: null, site_name: null };
const H_JUMP = device("host-0000-0000-0000-0000-000000000004", "win-rdp-jump", "192.0.2.41", SITE_AMS, TENANT_CORP, ROLE_JUMP);
const H_ROUTER = device("host-0000-0000-0000-0000-000000000005", "edge-router-01", "192.0.2.1", SITE_AMS, TENANT_CORE, ROLE_ROUTER);

const H_FRA_WEB = device("host-0000-0000-0000-0000-000000000011", "fra-web-01", "198.51.100.11", SITE_FRA, TENANT_CORE, ROLE_SERVER);
const H_FRA_DB = device("host-0000-0000-0000-0000-000000000012", "fra-db-01", "198.51.100.21", SITE_FRA, TENANT_CORE, ROLE_SERVER);
const H_FRA_LAB = vm("host-0000-0000-0000-0000-000000000013", "vnc-lab-01", "198.51.100.51", { id: "site-lab-0000-0000-0000-000000000003", name: "Remote Lab" }, TENANT_CORP, ROLE_LAB);
const H_FRA_JUMP = device("host-0000-0000-0000-0000-000000000014", "fra-rdp-jump", "198.51.100.41", SITE_FRA, TENANT_CORP, ROLE_JUMP);

// Two CCTV devices from different families, because the capability matrix is
// per-family: the Sofia/XMeye recorder exposes everything (PTZ, archive, export,
// on-device accounts), while a generic ONVIF camera is view-only by construction.
// Demoing both is what makes that distinction visible instead of implied.
const H_NVR: DemoHost = {
  ...device("host-0000-0000-0000-0000-000000000021", "nvr-ams-01", "192.0.2.90", SITE_AMS, TENANT_CORP, ROLE_CAMERA),
  is_cctv: true,
  family: "dvrip_sofia",
};
const H_CAM_ONVIF: DemoHost = {
  ...device("host-0000-0000-0000-0000-000000000022", "cam-fra-entrance", "198.51.100.90", SITE_FRA, TENANT_CORP, ROLE_CAMERA),
  is_cctv: true,
  family: "onvif",
};

const HOSTS: DemoHost[] = [
  H_WEB, H_DB, H_APP, H_JUMP, H_ROUTER, H_NVR,
  H_FRA_WEB, H_FRA_DB, H_FRA_LAB, H_FRA_JUMP, H_CAM_ONVIF,
];

// Opt-in public density fixture; the ordinary walkthrough stays small.
if (new URLSearchParams(globalThis.location?.search).get("dense-map") === "1") {
  HOSTS.push(...Array.from({ length: 495 }, (_, offset) => {
    const index = offset + 5;
    return (index < 250 ? device : vm)(`dense-map-${index}`, `ams-${index < 250 ? "device" : "vm"}-${String(index).padStart(3, "0")}`, `192.0.${Math.floor(index / 250) + 2}.${index % 250 + 1}`, SITE_AMS, TENANT_CORE, ROLE_SERVER);
  }));
  const additionalSites = [
    { id: "dense-site-campus", name: "Amsterdam Campus", latitude: "52.3676", longitude: "4.9041" },
    { id: "dense-site-origin", name: "Equator Station", latitude: "0", longitude: "0" },
  ];
  for (const site of additionalSites) {
    insert("inventory/sites", { ...site, slug: site.id });
    HOSTS.push(device(`target-${site.id}`, `${site.name} gateway`, "203.0.113.10", site, TENANT_CORE, ROLE_SERVER));
  }
}

// ── Connection launch options ────────────────────────────────────────────────
// Shape captured from GET /api/v1/connections/targets/{id}/launch-options.
// Accounts carry clean labels + usernames so ConnectButtons renders e.g.
// "root", "RDP", "VNC", "WEB" — never the bracketed "[...]" auto-label fallback.
interface LaunchAccount {
  entry_id: string;
  kind: "shared" | "personal";
  label: string;
  color?: string;
  username?: string;
  uri?: string;
  source: {
    scope_type?: string;
    scope_id?: string;
    record_name?: string;
    credential_set_name?: string;
  };
  is_default: boolean;
  requires_input: boolean;
}
interface LaunchProtocol {
  protocol: "ssh" | "rdp" | "vnc" | "app" | "cctv";
  render: "auto" | "buttons" | "split";
  accounts: LaunchAccount[];
}

const emptyProto = (protocol: LaunchProtocol["protocol"]): LaunchProtocol => ({
  protocol,
  render: "auto",
  accounts: [],
});

function sshAccount(label: string, username: string, isDefault = false): LaunchProtocol {
  return {
    protocol: "ssh",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:ssh:${username}`,
        kind: "shared",
        label,
        color: "#22c55e",
        username,
        source: { scope_type: "role", record_name: `Linux Server · ${label}` },
        is_default: isDefault,
        requires_input: false,
      },
    ],
  };
}

function rdpAccount(label: string, username?: string): LaunchProtocol {
  return {
    protocol: "rdp",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:rdp:0`,
        kind: "shared",
        label,
        color: "#00BCD4",
        ...(username ? { username } : {}),
        source: { scope_type: "site", record_name: `Jump Host · ${label}` },
        is_default: false,
        requires_input: !username,
      },
    ],
  };
}

function vncAccount(label: string): LaunchProtocol {
  return {
    protocol: "vnc",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:vnc:0`,
        kind: "shared",
        label,
        color: "#f59e0b",
        source: { scope_type: "tenant", record_name: `Lab Workstation · ${label}` },
        is_default: false,
        requires_input: true,
      },
    ],
  };
}

// `requires_input` must stay false: a truthy value opens the ad-hoc credentials
// dialog instead of the console, and CCTV never mints a backend session.
function cctvAccount(label: string, recordName: string): LaunchProtocol {
  return {
    protocol: "cctv",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:cctv:0`,
        kind: "shared",
        label,
        color: "#8b5cf6",
        source: { scope_type: "role", record_name: recordName },
        is_default: false,
        requires_input: false,
      },
    ],
  };
}

function appAccount(label: string, uri: string): LaunchProtocol {
  return {
    protocol: "app",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:app:0`,
        kind: "shared",
        label,
        color: "#4CAF50",
        uri,
        source: { record_name: `Router · ${label}` },
        is_default: false,
        requires_input: false,
      },
    ],
  };
}

// Per-host launch options. Linux servers expose SSH (sometimes root + deploy),
// the jump hosts expose RDP, the lab box VNC, the router SSH + a WEB app link.
const LAUNCH_OPTIONS: Record<string, LaunchProtocol[]> = {
  [H_WEB.id]: [sshAccount("root", "root", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_DB.id]: [
    { protocol: "ssh", render: "auto", accounts: [
      sshAccount("root", "root", true).accounts[0]!,
      { entry_id: "config:connconf:ssh:deploy", kind: "shared", label: "deploy", color: "#3b82f6", username: "deploy", source: { scope_type: "role", record_name: "Linux Server · deploy" }, is_default: false, requires_input: false },
    ] },
    emptyProto("rdp"), emptyProto("vnc"), emptyProto("app"),
  ],
  [H_APP.id]: [sshAccount("deploy", "deploy", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_JUMP.id]: [emptyProto("ssh"), rdpAccount("RDP", "Administrator"), emptyProto("vnc"), emptyProto("app")],
  [H_ROUTER.id]: [
    sshAccount("admin", "admin", true),
    emptyProto("rdp"),
    emptyProto("vnc"),
    appAccount("WEB", "https://192.0.2.1"),
  ],
  [H_FRA_WEB.id]: [sshAccount("root", "root", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_FRA_DB.id]: [sshAccount("root", "root", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_FRA_LAB.id]: [emptyProto("ssh"), emptyProto("rdp"), vncAccount("VNC"), emptyProto("app")],
  [H_FRA_JUMP.id]: [emptyProto("ssh"), rdpAccount("RDP", "Administrator"), emptyProto("vnc"), emptyProto("app")],
  [H_NVR.id]: [
    emptyProto("ssh"), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app"),
    cctvAccount("CCTV", "CCTV Recorder · admin"),
  ],
  [H_CAM_ONVIF.id]: [
    emptyProto("ssh"), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app"),
    cctvAccount("CCTV", "CCTV Camera · viewer"),
  ],
};

const emptyLaunch = [emptyProto("ssh"), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")];

// ── Active sessions ──────────────────────────────────────────────────────────
const ACTIVE_SESSIONS = [
  {
    id: "sess-0000-0000-0000-000000000001",
    target_id: H_WEB.id,
    user_id: "u-admin",
    protocol: "ssh",
    status: "active",
    proxy_id: "px-0000-0000-0000-000000000001",
    proxy_node_id: null,
    proxy_node_name: null,
    proxy_node_slug: null,
    routing_source: "embedded_default",
    client_ip: "203.0.113.10",
    last_activity: iso(1),
    session_metadata: { entryId: "config:connconf:ssh:root" },
    started_at: iso(14),
  },
];

function demoSessionHealth(sessionId: string): Response {
  return HttpResponse.json({
    session_id: sessionId,
    connection_type: "ssh",
    route_mode: "proxied",
    state: "degraded",
    reason_code: "proxy_target_latency",
    worst_leg_id: "proxy_target",
    proxy_node_name: "pxm-proxy01",
    updated_at: iso(0),
    stale_after_ms: 30_000,
    legs: [
      {
        id: "client_backend",
        state: "ok",
        latency_ms: 18,
        rx_bps: 2_048,
        tx_bps: 1_024,
        rx_bytes: 182_272,
        tx_bytes: 91_136,
        quality_label: "good",
        quality_score: 96,
        message: null,
      },
      {
        id: "backend_proxy",
        state: "ok",
        latency_ms: 42,
        rx_bps: 8_192,
        tx_bps: 4_096,
        rx_bytes: 724_992,
        tx_bytes: 348_160,
        quality_label: "good",
        quality_score: 91,
        message: null,
      },
      {
        id: "proxy_target",
        state: "degraded",
        latency_ms: 167,
        rx_bps: 512,
        tx_bps: 320,
        rx_bytes: 44_032,
        tx_bytes: 23_552,
        quality_label: "warning",
        quality_score: 61,
        message: "Target network latency is elevated",
      },
    ],
  });
}

// Inventory generic entities (Resources / Access Control / Settings screens) now
// live in the stateful store (`./store.ts`) and are served by the generic CRUD
// handlers below so create/edit/delete reflect in-session.

// Full capability grid (every Action × ResourceType from the OpenAPI enums) so
// the demo user is a full administrator. Without this the recent RBAC
// capability-gating hides almost every nav section/route/control (the SPA gates
// on `auth/me.capabilities`), leaving the demo looking empty/locked.
const DEMO_ACTIONS = [
  "CREATE", "READ", "UPDATE", "DELETE", "CONNECT", "OBSERVE", "EXECUTE",
  "MANAGE_ROLES", "MANAGE_POLICIES", "VIEW_AUDIT", "SECRET",
  // CCTV carries purpose-built actions instead of the generic CRUD verbs; the
  // access-control editor and the token scope picker render from this list.
  // `DEVICE_USERS` is the neutral on-device account action (video recorders and
  // Linux hosts alike), governed on the connection target.
  "CCTV_VIEW", "CCTV_RECORDINGS", "CCTV_EXPORT", "DEVICE_USERS", "CCTV_SETTINGS",
  "CCTV_PTZ",
] as const;
const DEMO_RESOURCE_TYPES = [
  "CONNECTION_TARGET", "CONNECTION_SESSION", "CONNECTION_FILTER_PRESET", "USER",
  "GROUP", "ROLE", "POLICY", "INVENTORY", "CONFIG_CONTEXT", "SECRET_VARIABLE",
  "AUDIT_EVENT", "KEYCLOAK", "WEBHOOK", "ASSISTANT",
  "ASSISTANT_CONVERSATION", "PLAYBOOK", "PLAYBOOK_VERSION", "PLAYBOOK_ASSIGNMENT",
  "PLAYBOOK_TARGET_SET", "PLAYBOOK_RUN_GROUP", "PLAYBOOK_RUN", "PLAYBOOK_SCHEDULE",
  "PLAYBOOK_ARTIFACT", "PLAYBOOK_SERVICE_PRINCIPAL",
  "PLAYBOOK_EXECUTION_ENVIRONMENT", "PLAYBOOK_PROJECT", "PLAYBOOK_JOB_TEMPLATE",
  "LICENSE", "SETTINGS", "BRANDING", "PROXY_NODE", "MONITORING", "BACKUP", "CCTV",
] as const;
const DEMO_CAPABILITIES = DEMO_RESOURCE_TYPES.flatMap((resource_type) =>
  DEMO_ACTIONS.map((action) => ({ action, resource_type })),
);

const SESSION_USER = {
  user_id: "u-admin",
  username: "admin",
  email: "admin@piping.demo",
  status: "active",
  auth_source: "local",
  expires_at: iso(-60 * 8),
  roles: ["Platform Admin"],
  capabilities: DEMO_CAPABILITIES,
};

const DEMO_PUBLIC_FILTER_PRESETS: Array<{
  id: string;
  name: string;
  filters: Record<string, string[]>;
  created_by: string;
  created_at: string;
  updated_at: string;
}> = [
  {
    id: "preset-public-active-0000-0000-000000000001",
    name: "Active infrastructure",
    filters: { status: ["active"] },
    created_by: "00000000-0000-0000-0000-000000000001",
    created_at: iso(80),
    updated_at: iso(80),
  },
];

// Instance branding — a single mutable record (not a list, so it lives here
// rather than in the store registry). Edits in the Appearance screen persist
// in-session and reset on reload like everything else.
const DEMO_BRANDING: {
  app_name: string;
  favicon_data_url: string | null;
  logo_data_url: string | null;
  theme: Record<string, unknown>;
  updated_at: string;
} = {
  app_name: "Piping",
  favicon_data_url: null,
  logo_data_url: null,
  theme: { mode: "default" },
  updated_at: iso(60 * 24),
};
const userPreferences = new Map<string, unknown>();
const instanceSettings = new Map<string, unknown>();

function effectiveSetting(key: string) {
  // Demo embed contract: the embedding portal names the demo language in
  // `?lang=`. That query is the single source of truth, so the mocked settings
  // API answers `app.language` with it — the app's own preference load then
  // resolves to the embedded language instead of an unset/stale default and
  // cannot override the explicit demo input.
  if (key === "app.language") {
    const demoLanguage = demoLanguageFromLocation();
    if (demoLanguage) {
      const value = { lng: demoLanguage };
      return {
        key,
        value,
        source: "instance",
        user_value: null,
        instance_value: value,
        updated_at: iso(0),
      };
    }
  }
  const hasUser = userPreferences.has(key);
  const hasInstance = instanceSettings.has(key);
  const userValue = hasUser ? userPreferences.get(key) : undefined;
  const instanceValue = hasInstance ? instanceSettings.get(key) : undefined;
  return {
    key,
    value: hasUser ? userValue : hasInstance ? instanceValue : null,
    source: hasUser ? "user" : hasInstance ? "instance" : "unset",
    user_value: hasUser ? userValue : null,
    instance_value: hasInstance ? instanceValue : null,
    updated_at: hasUser || hasInstance ? iso(0) : null,
  };
}

// Service Discovery preview, derived from the seeded HOSTS. Every host becomes a
// node-exporter scrape target/group so the SD screen renders a populated preview
// and non-zero target/group stat cards.
function buildSdPreview() {
  const hosts = HOSTS.map((h) => ({
    host_address: h.address,
    host_id: h.id,
    host_name: h.name,
    inspector_metrics: [],
    kind: h.target_kind,
    metric_sources: [],
    metrics: [],
    role_name: h.role_name,
    scrapes: [],
    site_name: h.site_name,
    status: h.status,
    tenant_name: h.tenant_name,
  }));
  const groups = hosts.map((h) => ({
    labels: {
      __address__: `${h.host_address}:9100`,
      job: "node",
      instance: h.host_name,
      site: h.site_name ?? "",
      role: h.role_name ?? "",
    },
    targets: [`${h.host_address}:9100`],
  }));
  return { groups, groups_count: groups.length, hosts, targets_count: groups.length };
}

// SD label-output toggles (SdLabelSettingsResponse). Mutable in-session.
const DEMO_SD_LABELS = {
  groups: {
    contacts: false,
    description: false,
    hardware: true,
    identifiers: false,
    location: true,
    network: true,
    role: true,
    status: true,
    tenant: true,
    device_statuses: ["active"],
    site_statuses: ["active"],
  } as Record<string, unknown>,
};

export const handlers = [
  // Auth — keep AppShell authenticated and the login screen consistent.
  http.get("/api/v1/auth/me", () => HttpResponse.json(SESSION_USER)),
  http.post("/api/v1/auth/login", () => HttpResponse.json(SESSION_USER)),
  http.post("/api/v1/auth/logout", () => new HttpResponse(null, { status: 204 })),
  http.post("/api/v1/auth/session-refresh", () => HttpResponse.json({ status: "ok" })),
  http.get("/api/v1/auth/providers", () =>
    HttpResponse.json({ local_enabled: true, oidc_enabled: false, ldap_enabled: false, ad_enabled: false }),
  ),

  // Assistant — bootstrap only. The demo runs the assistant shell read-only
  // (no session_shell / chat_messages) so it never falls through to the portal
  // backend, whose public-docs `/assistant/bootstrap` returns a different shape
  // (no `capabilities`/`access`), which previously crashed the embedded demo.
  http.get("/api/v1/assistant/bootstrap", () =>
    HttpResponse.json({
      feature_status: "ready",
      access: {
        can_execute: false,
        can_open: true,
        can_update_preferences: false,
        can_view_conversations: false,
      },
      capabilities: {
        chat_messages: false,
        chat_streaming: false,
        mcp_tools: false,
        portal_context_packets: false,
        session_shell: false,
      },
      config: {
        api_key_configured: true,
        archive_retention_days: 30,
        base_url_configured: true,
        instance_enabled: true,
        max_sessions_per_user: 5,
        model: "demo-assistant",
        portal_context_enabled: false,
        provider: "Demo",
      },
    }),
  ),

  // Canonical settings.
  http.get("/api/v1/settings/effective", ({ request }) => {
    const url = new URL(request.url);
    const keys = (url.searchParams.get("keys") ?? "")
      .split(",")
      .map((key) => key.trim())
      .filter(Boolean);
    const allKeys = keys.length
      ? keys
      : [...new Set([...userPreferences.keys(), ...instanceSettings.keys()])].sort((a, b) =>
          a.localeCompare(b),
        );
    return HttpResponse.json({ settings: allKeys.map(effectiveSetting) });
  }),
  http.get("/api/v1/settings/effective/:key", ({ params }) =>
    HttpResponse.json(effectiveSetting(String(params.key))),
  ),
  http.put("/api/v1/settings/user/:key", async ({ params, request }) => {
    const body = await request.json().catch(() => undefined);
    const value =
      body && typeof body === "object" && "value" in body
        ? (body as { value: unknown }).value
        : null;
    userPreferences.set(String(params.key), value);
    return new HttpResponse(null, { status: 204 });
  }),
  http.delete("/api/v1/settings/user/:key", ({ params }) => {
    userPreferences.delete(String(params.key));
    return new HttpResponse(null, { status: 204 });
  }),
  http.get("/api/v1/admin/settings/defaults", () =>
    HttpResponse.json({
      settings: [...instanceSettings.entries()].map(([key, value]) => ({
        key,
        value,
        updated_at: iso(0),
        updated_by_user_id: "00000000-0000-0000-0000-000000000001",
      })),
    }),
  ),
  http.put("/api/v1/admin/settings/defaults/:key", async ({ params, request }) => {
    const body = await request.json().catch(() => undefined);
    const value =
      body && typeof body === "object" && "value" in body
        ? (body as { value: unknown }).value
        : null;
    const key = String(params.key);
    instanceSettings.set(key, value);
    return HttpResponse.json({
      key,
      value,
      updated_at: iso(0),
      updated_by_user_id: "00000000-0000-0000-0000-000000000001",
    });
  }),
  http.delete("/api/v1/admin/settings/defaults/:key", ({ params }) => {
    instanceSettings.delete(String(params.key));
    return new HttpResponse(null, { status: 204 });
  }),

  // Legacy user preference compatibility — override-only.
  http.get("/api/v1/user/settings/preferences/:key", ({ params }) =>
    HttpResponse.json({
      key: String(params.key),
      value: userPreferences.has(String(params.key))
        ? userPreferences.get(String(params.key))
        : null,
    }),
  ),
  http.put("/api/v1/user/settings/preferences/:key", async ({ params, request }) => {
    const body = await request.json().catch(() => undefined);
    const value =
      body && typeof body === "object" && "value" in body
        ? (body as { value: unknown }).value
        : null;
    userPreferences.set(String(params.key), value);
    return new HttpResponse(null, { status: 204 });
  }),

  // Connection targets (Connections workspace tree).
  http.get("/api/v1/connections/targets", () => HttpResponse.json(envelope(HOSTS))),
  http.get("/api/v1/connections/targets/:id", ({ params }) => {
    const host = HOSTS.find((target) => target.id === String(params.id));
    if (!host) return HttpResponse.json({ code: "NOT_FOUND", message: "Connection target not found" }, { status: 404 });
    const target: components["schemas"]["ConnectionTargetListRowResponse"] = { ...host, address_bound: true };
    return HttpResponse.json(target);
  }),
  http.get("/api/v1/connections/targets/:id/launch-options", ({ params }) => {
    const id = String(params.id);
    return HttpResponse.json({ host_id: id, protocols: LAUNCH_OPTIONS[id] ?? emptyLaunch });
  }),
  http.get("/api/v1/connections/targets/:id/effective-config", () =>
    HttpResponse.json({ data: {}, sources: [] }),
  ),
  // Assignment/inheritance resolution belongs to the backend, not a second
  // implementation in MSW. A successful list envelope crashes this consumer.
  http.get("/api/v1/connection-config/effective/:deviceId", () =>
    HttpResponse.json({ code: "NOT_IMPLEMENTED", message: "Effective configuration requires a backend-backed instance." }, { status: 501 }),
  ),

  // ── CCTV console ───────────────────────────────────────────────────────────
  // In a real install these 400 until device credentials are configured; the
  // demo answers them fully so the console reads as a working install.
  //
  // `/capabilities` is load-bearing: the console derives EVERY tab from it, so
  // without this handler the generic CRUD fallback returns a list envelope, the
  // permissions object comes back undefined, and the console renders an empty
  // tab strip. It must also stay ahead of `...crudHandlers()` below.
  //
  // The two demo devices deliberately differ, because the capability matrix is
  // per family: the Sofia recorder exposes everything, while the ONVIF camera is
  // view-only (its driver refuses user management, disk and recording status).
  http.get("/api/v1/cctv/:targetId/capabilities", ({ params }) => {
    const onvif = params.targetId === H_CAM_ONVIF.id;
    return HttpResponse.json({
      info: true,
      channels: true,
      snapshot: true,
      // Live MJPEG is a server-side transcode with no equivalent in a mocked
      // demo; reporting it unsupported puts the console on its stills path,
      // which is a real product mode rather than a demo-only shim.
      streaming: false,
      users: !onvif,
      user_update: !onvif,
      authority_catalog: !onvif,
      disk: !onvif,
      recording: !onvif,
      recording_search: !onvif,
      export: !onvif,
      ptz: !onvif,
      permissions: {
        view: true,
        recordings: !onvif,
        export: !onvif,
        users: !onvif,
        settings: !onvif,
        ptz: !onvif,
      },
    });
  }),
  http.get("/api/v1/cctv/:targetId/channels", ({ params }) =>
    HttpResponse.json({
      channels:
        params.targetId === H_CAM_ONVIF.id
          ? [{ index: 1, name: "Entrance", enabled: true, ptz_support: "unsupported", recording: false }]
          : [
              { index: 1, name: "Lobby", enabled: true, ptz_support: "supported", recording: true },
              { index: 2, name: "Parking P1", enabled: true, ptz_support: "supported", recording: true },
              { index: 3, name: "Warehouse A", enabled: true, ptz_support: "unsupported", recording: true },
              { index: 4, name: "Gate / Entrance", enabled: true, ptz_support: "supported", recording: true },
            ],
    }),
  ),
  // Binary endpoints answer with generated stand-in media (see ./cctv-media).
  http.get("/api/v1/cctv/:targetId/snapshot", ({ request }) => {
    const channel = Number(new URL(request.url).searchParams.get("channel") ?? 1);
    const bytes = cctvFrameBytes(Number.isFinite(channel) && channel > 0 ? channel : 1);
    return new HttpResponse(bytes, {
      headers: { "Content-Type": "image/jpeg", "Content-Length": String(bytes.byteLength) },
    });
  }),
  http.get("/api/v1/cctv/:targetId/playback", () => {
    const bytes = cctvClipBytes();
    return new HttpResponse(bytes, {
      headers: {
        "Content-Type": "video/mp4",
        "Content-Length": String(bytes.byteLength),
        "Content-Disposition": "inline",
      },
    });
  }),
  http.get("/api/v1/cctv/:targetId/export", () => {
    const bytes = cctvClipBytes();
    return new HttpResponse(bytes, {
      headers: {
        "Content-Type": "video/mp4",
        "Content-Length": String(bytes.byteLength),
        "Content-Disposition": 'attachment; filename="clip.mp4"',
      },
    });
  }),
  // PTZ answers 204 in production, but the demo must return a body: the client
  // unwraps the envelope and treats an undefined body as an error.
  http.post("/api/v1/cctv/:targetId/ptz", () => HttpResponse.json({ ok: true })),
  http.get("/api/v1/cctv/:targetId/authorities", () =>
    HttpResponse.json({
      authorities: [
        "all", "live", "playback", "config", "ptz", "export", "search", "shutdown",
      ],
    }),
  ),
  http.get("/api/v1/cctv/:targetId/info", () =>
    HttpResponse.json({
      model: "NVR-16",
      firmware: "V5.2.1 build 240118",
      serial: "NVR16-2400-000482",
      channels: 4,
      device_time: iso(1),
    }),
  ),
  http.get("/api/v1/device-fleet/:targetId/users", () =>
    HttpResponse.json({
      users: [
        { name: "admin", group: "Administrators", authorities: ["all", "playback", "config", "ptz"] },
        { name: "operator", group: "Operators", authorities: ["playback", "live"] },
        { name: "viewer", group: "Guests", authorities: ["live"] },
      ],
    }),
  ),
  http.get("/api/v1/cctv/:targetId/disk", () =>
    HttpResponse.json({
      disks: [
        { name: "HDD1", total_bytes: 4_000_000_000_000, used_bytes: 3_760_000_000_000, state: "ok", is_error: false },
        { name: "HDD2", total_bytes: 4_000_000_000_000, used_bytes: 1_240_000_000_000, state: "ok", is_error: false },
      ],
    }),
  ),
  http.get("/api/v1/cctv/:targetId/recording", () =>
    HttpResponse.json({
      active: true,
      oldest: iso(30 * 24 * 60),
      newest: iso(1),
      spans: [
        { start: iso(30 * 24 * 60), end: iso(18 * 24 * 60) },
        { start: iso(18 * 24 * 60), end: iso(6 * 24 * 60) },
        { start: iso(6 * 24 * 60), end: iso(1) },
      ],
    }),
  ),
  // Per-channel archive search backing the timeline. Continuous recording with
  // two gaps, so the timeline shows something more honest than a solid bar.
  http.get("/api/v1/cctv/:targetId/recordings", ({ request }) => {
    const url = new URL(request.url);
    const channel = Number(url.searchParams.get("channel") ?? 1) || 1;
    const dayStart = Date.parse(url.searchParams.get("start") ?? "") || NOW - 24 * 60 * 60_000;
    const at = (h: number, m = 0) => new Date(dayStart + (h * 60 + m) * 60_000).toISOString();
    return HttpResponse.json({
      spans: [
        { start: at(0), end: at(7, 12), channel },
        { start: at(7, 26), end: at(14, 3), channel },
        { start: at(14, 20), end: at(23, 59), channel },
      ],
    });
  }),
  // On-device account writes. Create/delete are preview-then-confirm; editing an
  // existing account is a direct PUT with no dry run, matching the product.
  http.post("/api/v1/device-fleet/:targetId/users:preview", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as {
      op?: string;
      users?: { name?: string }[];
      usernames?: string[];
    };
    const existing = new Set(["admin", "operator", "viewer"]);
    const perItem =
      body.op === "delete"
        ? (body.usernames ?? []).map((name) => ({
            name,
            action: existing.has(name) ? "delete" : "missing",
            reason: existing.has(name) ? null : "no such account on the device",
          }))
        : (body.users ?? []).map((user) => ({
            name: user.name ?? "",
            action: existing.has(user.name ?? "") ? "skip" : "create",
            reason: existing.has(user.name ?? "") ? "account already exists" : null,
          }));
    return HttpResponse.json({ per_item: perItem, writes_enabled: true });
  }),
  http.post("/api/v1/device-fleet/:targetId/users:create", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { users?: { name?: string }[] };
    return HttpResponse.json({
      results: (body.users ?? []).map((user) => ({ name: user.name ?? "", ok: true, error: null })),
    });
  }),
  http.post("/api/v1/device-fleet/:targetId/users:delete", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { usernames?: string[] };
    return HttpResponse.json({
      results: (body.usernames ?? []).map((name) => ({ name, ok: true, error: null })),
    });
  }),
  http.put("/api/v1/device-fleet/:targetId/users", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { users?: { name?: string }[] };
    return HttpResponse.json({
      results: (body.users ?? []).map((user) => ({ name: user.name ?? "", ok: true, error: null })),
    });
  }),

  http.get("/api/v1/connections/filter-presets", () =>
    HttpResponse.json({ items: DEMO_PUBLIC_FILTER_PRESETS }),
  ),
  http.post("/api/v1/connections/filter-presets", async ({ request }) => {
    const body = await readBody(request);
    const now = iso(0);
    const preset = {
      id: `preset-public-${Date.now().toString(16)}`,
      name: typeof body.name === "string" ? body.name : "Saved filter",
      filters:
        body.filters && typeof body.filters === "object" && !Array.isArray(body.filters)
          ? (body.filters as Record<string, string[]>)
          : {},
      created_by: "00000000-0000-0000-0000-000000000001",
      created_at: now,
      updated_at: now,
    };
    DEMO_PUBLIC_FILTER_PRESETS.unshift(preset);
    return HttpResponse.json(preset, { status: 201 });
  }),
  http.put("/api/v1/connections/filter-presets/:id", async ({ params, request }) => {
    const body = await readBody(request);
    const preset = DEMO_PUBLIC_FILTER_PRESETS.find((item) => item.id === params.id);
    if (!preset) return new HttpResponse(null, { status: 404 });
    if (typeof body.name === "string") preset.name = body.name;
    if (body.filters && typeof body.filters === "object" && !Array.isArray(body.filters)) {
      preset.filters = body.filters as Record<string, string[]>;
    }
    preset.updated_at = iso(0);
    return HttpResponse.json(preset);
  }),
  http.delete("/api/v1/connections/filter-presets/:id", ({ params }) => {
    const index = DEMO_PUBLIC_FILTER_PRESETS.findIndex((item) => item.id === params.id);
    if (index >= 0) DEMO_PUBLIC_FILTER_PRESETS.splice(index, 1);
    return new HttpResponse(null, { status: index >= 0 ? 204 : 404 });
  }),

  // Audit target lookup (server-side search for audit filter pickers).
  http.get("/api/v1/audit/targets", ({ request }) => auditTargetsResponse(request)),
  http.get("*/api/v1/audit/targets", ({ request }) => auditTargetsResponse(request)),

  // Instance branding (Appearance / Administration). Backend always answers a
  // BrandingResponse; the store-less default keeps the demo app name + theme.
  http.get("/api/v1/instance/branding", () => HttpResponse.json(DEMO_BRANDING)),
  http.put("/api/v1/instance/branding", async ({ request }) => {
    const body = await readBody(request);
    Object.assign(DEMO_BRANDING, body, { updated_at: iso(0) });
    return HttpResponse.json(DEMO_BRANDING);
  }),

  // API reference (Swagger UI) — serve the REAL bundled OpenAPI contract as YAML
  // with the proper content-type so Swagger renders it (a valid `openapi: 3.1.0`
  // document with a version field). Registered before the generic CRUD/catch-all
  // handlers so the 2-segment `openapi/<file>` path isn't swallowed as a list.
  // Previously this fell through to the catch-all which returned a JSON envelope,
  // so Swagger reported "does not specify a valid version field".
  http.get("/api/v1/openapi/piping.openapi.yaml", () => openapiResponse()),
  http.get("*/api/v1/openapi/piping.openapi.yaml", () => openapiResponse()),

  // Connections / sessions.
  http.get("/api/v1/connections/sessions", () =>
    HttpResponse.json({ items: ACTIVE_SESSIONS, total: ACTIVE_SESSIONS.length, limit: 1000, offset: 0 }),
  ),
  http.post("/api/v1/connections/sessions", async ({ request }) => {
    // Read the requested host + protocol so the demo can (a) report the right
    // protocol back and (b) hand SSH a per-host relay URL the demo WS shim
    // (ssh-socket-shim.ts) intercepts to stream that host's canned shell transcript.
    let targetId = H_WEB.id;
    let protocol = "ssh";
    try {
      const body = (await request.clone().json()) as { target_id?: string; protocol?: string };
      if (typeof body.target_id === "string") targetId = body.target_id;
      if (typeof body.protocol === "string") protocol = body.protocol;
    } catch {
      // Body unreadable — fall back to defaults; the pane still opens.
    }
    const host = HOSTS.find((h) => h.id === targetId);
    const hostName = host?.name ?? "linux-host";
    const sessionId = crypto.randomUUID();
    // SSH gets a host-scoped relay path the WS handler matches on; RDP/VNC are
    // rendered as static mock desktops in the viewer (no live socket needed) so
    // any URL is fine for them.
    const websocketUrl =
      protocol === "ssh"
        ? `wss://demo.invalid/relay/ssh/${encodeURIComponent(hostName)}`
        : `wss://demo.invalid/relay/${protocol}/${encodeURIComponent(hostName)}`;
    return HttpResponse.json({
      session: {
        ...ACTIVE_SESSIONS[0],
        id: sessionId,
        target_id: targetId,
        protocol,
        status: "active",
        started_at: iso(0),
        last_activity: iso(0),
      },
      proxy: {
        connection_id: sessionId,
        websocket_url: websocketUrl,
        token: "demo-token",
        token_ttl_seconds: 60,
      },
    });
  }),
  http.get("/api/v1/connections/sessions/:id/health", ({ params }) =>
    demoSessionHealth(String(params.id)),
  ),
  http.get("*/api/v1/connections/sessions/:id/health", ({ params }) =>
    demoSessionHealth(String(params.id)),
  ),
  http.post("/api/v1/connections/sessions/:id/disconnect", () => new HttpResponse(null, { status: 204 })),
  http.post("/api/v1/connections/sessions/:id/terminate", () => new HttpResponse(null, { status: 204 })),

  // Observability — the target inspector reads `data.target.metrics`; return a
  // well-formed envelope with no configured metrics so the section renders an
  // empty state rather than throwing on an undefined `target`.
  http.get("/api/v1/monitoring/targets/:id", ({ params }) =>
    HttpResponse.json({
      target: { id: String(params.id), metrics: [], inspector_metrics: [] },
      scrape_configs: [],
    }),
  ),
  http.get("/api/v1/monitoring/latest", () => HttpResponse.json({ items: [] })),
  // Service Discovery preview — built from the seeded hosts so the SD screen's
  // discovery preview + the "targets / groups" stat cards show real numbers
  // (ServiceDiscoveryPreviewResponse: groups[], groups_count, hosts[], targets_count).
  http.get("/api/v1/monitoring/sd/preview", () => HttpResponse.json(buildSdPreview())),
  http.get("*/api/v1/monitoring/sd/preview", () => HttpResponse.json(buildSdPreview())),
  // SD label settings — the "labels in output" toggle group. Mutable in-session.
  http.get("/api/v1/monitoring/sd/label-settings", () => HttpResponse.json(DEMO_SD_LABELS)),
  http.put("/api/v1/monitoring/sd/label-settings", async ({ request }) => {
    const body = await readBody(request);
    if (body.groups && typeof body.groups === "object") {
      Object.assign(DEMO_SD_LABELS.groups, body.groups as Record<string, unknown>);
    }
    return HttpResponse.json(DEMO_SD_LABELS);
  }),

  // License state (EntitlementState) — a healthy activated license so the
  // Administration → License screen renders seats/validity instead of empty.
  http.get("/api/v1/license/state", () =>
    HttpResponse.json({
      status: "active",
      seatLimit: 50,
      seatsUsed: 12,
      validFrom: iso(60 * 24 * 200),
      validUntil: iso(-60 * 24 * 165),
      version: 1,
    }),
  ),

  // Inventory content types — the custom-fields screen consumes this as a bare
  // ARRAY (not the list envelope); returning the generic empty envelope made it
  // "not iterable" and crashed the page. Return the polymorphic object types.
  http.get("/api/v1/inventory/content-types", () =>
    HttpResponse.json([
      { id: "ct-device", app_label: "dcim", model: "device", display: "device" },
      { id: "ct-vm", app_label: "virtualization", model: "virtualmachine", display: "virtual machine" },
      { id: "ct-site", app_label: "dcim", model: "site", display: "site" },
      { id: "ct-rack", app_label: "dcim", model: "rack", display: "rack" },
      { id: "ct-interface", app_label: "dcim", model: "interface", display: "interface" },
    ]),
  ),
  http.get("/api/v1/inventory/prefixes/tree", ({ request }) =>
    demoPrefixTreeResponse(request),
  ),
  http.get("/api/v1/inventory/prefixes/:id/next-available-prefix/preview", ({ request }) =>
    demoPrefixPlanningPreview(request),
  ),
  http.get("/api/v1/inventory/cabling/endpoints", ({ request }) =>
    demoCablingEndpoints(request),
  ),
  http.get("/api/v1/inventory/cabling/trace", ({ request }) =>
    demoCablingTrace(request),
  ),

  // Permission vocabulary — the single source-of-truth dictionary the policy
  // editor (and token scope picker) render from. Representative full catalog:
  // every Action × every ResourceType, all enforced and PAT-grantable.
  http.get("/api/v1/access-control/vocabulary", () =>
    HttpResponse.json({
      actions: [...DEMO_ACTIONS],
      resources: DEMO_RESOURCE_TYPES.map((resource_type) => ({
        resource_type,
        status: "enforced",
        pat_grantable_actions: [...DEMO_ACTIONS],
        // Only CONNECTION_TARGET is attribute-scopable today; the policy editor
        // offers exactly these keys for an `attributes` constraint.
        constraint_attribute_keys:
          resource_type === "CONNECTION_TARGET"
            ? ["role_id", "site_id", "tenant_id", "platform_id", "cluster_id", "status", "protocol"]
            : [],
      })),
    }),
  ),

  // Project file read-back: the generic 4-segment CRUD handler treats
  // `/playbooks/projects/{id}/files` as a detail read and answers
  // `{ id, name, status }`, which the editor then fails to `.map`. The demo
  // seeds no projects, so the honest read-back is an empty file set.
  http.get("/api/v1/playbooks/projects/:id/files", () => HttpResponse.json({ files: [] })),
  // Whole-set replace. The response body is a PlaybookProject; the demo has no
  // project row to mutate, so echo a well-formed manual project for `{id}`.
  http.post("/api/v1/playbooks/projects/:id/files", ({ params }) =>
    HttpResponse.json({
      id: params.id,
      name: "",
      description: null,
      scm_type: "manual",
      scm_url: null,
      scm_branch: null,
      scm_ref: null,
      playbook_base_path: null,
      credential_id: null,
      default_execution_environment_id: null,
      sync_policy: "manual",
      sync_status: "never_synced",
      last_sync_revision: null,
      last_sync_error: null,
      last_synced_at: null,
      metadata: {},
      created_at: iso(0),
      updated_at: iso(0),
    }),
  ),

  // ── Generic stateful CRUD over the in-memory store ─────────────────────────
  // Registered AFTER all specific handlers above (so sessions/branding win)
  // and BEFORE the catch-all. Navigating any covered area shows seeded data and
  // create/edit/delete mutate the store (reflected on the next refetch). A page
  // reload re-imports the store module → re-seeds → demo resets to a clean state.
  ...crudHandlers(),

  // Catch-all: any other GET still returns a valid envelope (stateful where the
  // path maps to a store key, empty otherwise); any write is a benign success so
  // the demo never errors on screens beyond the explicitly modelled ones.
  // Both the relative (`/api/v1/*`, same-origin) and absolute (`*/api/v1/*`)
  // forms are registered so nothing slips through to the network (404).
  http.get("/api/v1/*", catchAllGet),
  http.get("*/api/v1/*", catchAllGet),
  http.post("/api/v1/*", catchAllWrite),
  http.post("*/api/v1/*", catchAllWrite),
  http.put("/api/v1/*", catchAllWrite),
  http.put("*/api/v1/*", catchAllWrite),
  http.patch("/api/v1/*", catchAllWrite),
  http.patch("*/api/v1/*", catchAllWrite),
  http.delete("/api/v1/*", () => new HttpResponse(null, { status: 204 })),
  http.delete("*/api/v1/*", () => new HttpResponse(null, { status: 204 })),
];

// ─────────────────────────────────────────────────────────────────────────────
// Swagger spec response.
// ─────────────────────────────────────────────────────────────────────────────
function openapiResponse(): Response {
  return new HttpResponse(openapiYaml, {
    status: 200,
    headers: { "Content-Type": "application/yaml; charset=utf-8" },
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic CRUD handler generation.
//
// Every store key under `/api/v1/<key>` gets a collection route (GET list,
// POST create) and an item route (GET detail, PUT/PATCH update, DELETE). The
// path after `/api/v1/` IS the store key, so the handlers extract it directly
// from the URL. We register one parameterised handler per *area prefix* so MSW's
// path matcher can isolate `{id}` cleanly even for multi-segment keys like
// `admin/access-control/users`.
// ─────────────────────────────────────────────────────────────────────────────

/** Path prefixes (under /api/v1/) whose sub-resources are store-backed. */
const CRUD_AREAS = [
  "inventory",
  "admin/access-control",
  "admin",
  "connection-config",
  "config-contexts",
  "config-context-bindings",
  "connections/templates",
  "webhooks",
  "user/settings",
  "audit",
  "monitoring",
  "playbooks",
] as const;

/** Reserved single-segment endpoints under an area that are NOT plain CRUD. */
const RESERVED_SEGMENTS = new Set(["bulk", "export", "import", "templates"]);

/** Path segments after `/api/v1/`, e.g. ["inventory","sites","abc"]. */
function apiSegments(request: Request): string[] {
  const path = new URL(request.url).pathname.replace(/^.*\/api\/v1\//, "");
  return path.split("/").filter(Boolean);
}

function crudHandlers() {
  // Broad path handlers keyed off the URL: the store key IS the path remainder,
  // so we derive collection key + id from the segments. Specific handlers above
  // (sessions/branding/swagger) are registered earlier and win; the
  // catch-all below catches anything these don't.
  return [
    // GET: list (2-3 area segments) or detail (trailing id).
    http.get("/api/v1/:a/:b", ({ request }) => {
      const s = apiSegments(request);
      return crudList(s.join("/"), request);
    }),
    http.get("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudListOrDetail(s.slice(0, 2).join("/"), s[2], s.join("/"), request);
    }),
    http.get("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudListOrDetail(s.slice(0, 3).join("/"), s[3], s.join("/"), request);
    }),
    // POST: create on collection.
    http.post("/api/v1/:a/:b", ({ request }) => crudCreate(apiSegments(request).join("/"), request)),
    http.post("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudCreate(s.join("/"), request, s.slice(0, 2).join("/"));
    }),
    http.post("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudCreate(s.join("/"), request, s.slice(0, 3).join("/"));
    }),
    // PUT/PATCH: update item.
    http.put("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 2).join("/"), s[2], request);
    }),
    http.patch("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 2).join("/"), s[2], request);
    }),
    http.put("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 3).join("/"), s[3], request);
    }),
    http.patch("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 3).join("/"), s[3], request);
    }),
    // DELETE: remove item.
    http.delete("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudDelete(s.slice(0, 2).join("/"), s[2]);
    }),
    http.delete("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudDelete(s.slice(0, 3).join("/"), s[3]);
    }),
  ];
}

function inCrudArea(key: string): boolean {
  return CRUD_AREAS.some((a) => key === a || key.startsWith(`${a}/`));
}

/** Pass non-CRUD paths through to the catch-all by returning `passthrough`-like. */
function notHandled(): Response {
  // Returning an empty valid envelope keeps any unexpected screen from breaking.
  return HttpResponse.json({ items: [], total: 0, limit: 1000, offset: 0 });
}

function crudList(key: string, request: Request): Response {
  if (!inCrudArea(key)) return notHandled();
  return listResponse(key, new URL(request.url));
}

function crudListOrDetail(
  collectionKey: string,
  idOrSub: string | undefined,
  fullKey: string,
  request: Request,
): Response {
  if (!idOrSub) return notHandled();
  // If the full path is itself a registered collection (e.g.
  // `admin/access-control/users`, `user/settings/api-tokens`), this is a LIST
  // request — NOT a detail read of the 2-segment parent area. Without this the
  // trailing segment ("users") is mistaken for an :id, the handler returns a
  // single object instead of `{ items: [...] }`, and the views crash on
  // `data.items` being undefined ("can't access property map, e.items is undefined").
  if (isCollection(fullKey)) return listResponse(fullKey, new URL(request.url));
  // `/api/v1/inventory/templates/{id}` → detail; reserved sub-collections
  // (e.g. `.../bulk`) are not plain detail reads.
  if (RESERVED_SEGMENTS.has(idOrSub)) {
    // Could be a nested collection (e.g. .../runs) — try it as a store key.
    if (inCrudArea(fullKey)) return listResponse(fullKey, new URL(request.url));
    return notHandled();
  }
  if (!inCrudArea(collectionKey)) return notHandled();
  const found = findById(collectionKey, idOrSub);
  return HttpResponse.json(found ?? { id: idOrSub, name: idOrSub, status: "active" });
}

async function crudCreate(key: string, request: Request, fallbackKey?: string): Promise<Response> {
  // Sub-action POSTs (enable/disable/revoke/default/...) just succeed.
  const lastSeg = key.split("/").pop() ?? "";
  if (NON_CRUD_POST_SEGMENTS.has(lastSeg)) {
    return new HttpResponse(null, { status: 204 });
  }
  const targetKey = inCrudArea(key) ? key : fallbackKey && inCrudArea(fallbackKey) ? fallbackKey : null;
  if (!targetKey) return new HttpResponse(null, { status: 204 });
  const body = await readBody(request);
  return HttpResponse.json(insert(targetKey, normalizeCreate(targetKey, body)), { status: 201 });
}

async function crudUpdate(key: string, id: string | undefined, request: Request): Promise<Response> {
  if (!id || NON_CRUD_POST_SEGMENTS.has(id)) return new HttpResponse(null, { status: 204 });
  if (!inCrudArea(key)) return new HttpResponse(null, { status: 204 });
  const body = await readBody(request);
  return HttpResponse.json(update(key, id, key === "monitoring/exporter-assignments" ? normalizeCreate(key, body) : body));
}

function crudDelete(key: string, id: string | undefined): Response {
  if (id && inCrudArea(key)) remove(key, id);
  return new HttpResponse(null, { status: 204 });
}

/** POST tail segments that are actions, not resource creates. */
const NON_CRUD_POST_SEGMENTS = new Set([
  "enable",
  "disable",
  "revoke",
  "default",
  "test-connection",
  "test-fire",
  "secret",
  "password",
  "disconnect",
  "terminate",
  "preview-run",
  "invalidate",
]);

/**
 * Light per-area shaping so a freshly created row carries the fields its list
 * table reads (e.g. access-control rows need snake_case `auth_source`). Most
 * areas need nothing beyond the raw body + generated id.
 */
function normalizeCreate(key: string, body: Record<string, unknown>): Record<string, unknown> {
  if (key === "monitoring/exporter-assignments") {
    const profile = typeof body.exporter_id === "string" ? findById("monitoring/exporters", body.exporter_id) : undefined;
    return {
      ...body,
      exporter_slug: profile?.slug ?? body.exporter_slug,
      exporter_name: profile?.name ?? body.exporter_name ?? body.exporter_slug,
    };
  }
  if (key === "admin/access-control/users") {
    return {
      username: body.username ?? "new-user",
      email: body.email ?? "",
      status: body.status ?? "active",
      auth_source: body.auth_source ?? "local",
      external_id: body.external_id ?? null,
    };
  }
  if (key === "connection-config/records") {
    // `ConnConfRecordResponse.description` is a plain `string`: the column is
    // NOT NULL DEFAULT '' and the insert binds `unwrap_or_default()`, so the
    // real API cannot answer null. The UI sends null for an empty description,
    // and echoing that back made `draftFromRecord` produce a draft whose
    // `description.trim()` threw before the update mutation ever ran - Save
    // Changes issued no PUT and reported nothing.
    return {
      ...body,
      enabled: body.enabled ?? true,
      description: typeof body.description === "string" ? body.description : "",
    };
  }
  if (key === "connection-config/assignments") {
    return { ...body, enabled: body.enabled ?? true };
  }
  if (key === "admin/access-control/role-assignments") {
    return {
      role_id: body.role_id ?? "",
      user_id: body.user_id ?? null,
      group_id: body.group_id ?? null,
    };
  }
  return body;
}

// ─────────────────────────────────────────────────────────────────────────────
// Stateful catch-all. Any GET whose path maps to a registered store area returns
// that area's live (mutated) rows; everything else returns a valid empty
// envelope. Writes always succeed so no screen breaks.
// ─────────────────────────────────────────────────────────────────────────────
function catchAllGet({ request }: { request: Request }): Response {
  const url = new URL(request.url);
  const key = url.pathname.replace(/^.*\/api\/v1\//, "");
  if (key && inCrudArea(key)) return listResponse(key, url);
  return HttpResponse.json({ items: [], total: 0, limit: 1000, offset: 0 });
}

function catchAllWrite(): Response {
  return new HttpResponse(null, { status: 204 });
}
