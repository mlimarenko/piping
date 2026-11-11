import { apiClient, unwrap } from "@/shared/api/client";
import { trackProductEvent } from "@/shared/lib/telemetry";
import type { Uuid } from "@/shared/lib/ids";
import type { components } from "@/shared/api/schema.gen";
import type {
  ConnectionTarget,
  ListQuery,
  Paginated,
  ActiveSession,
  ConnectionSessionStatus,
  OpenSessionRequest,
  OpenSessionResponse,
  SessionProtocol,
  ConnectionLaunchOptions,
  ConnectionLaunchProtocol,
  ResourceStatus,
} from "@/shared/api/types";
import { extractPersistedIcon } from "@/shared/entity-icon-utils";
import {
  loadSessionViewSettings,
  sessionViewLaunchOptions,
} from "@/features/connections/sessionViewPreferences";

// ── Backend DTOs ─────────────────────────────────────────

type BackendConnectionTarget = components["schemas"]["ConnectionTargetListRowResponse"];

type BackendActiveSession = components["schemas"]["ConnectionSessionResponse"];
type BackendOpenSessionResponse = components["schemas"]["CreateConnectionSessionResponse"];
type BackendResumeSessionResponse = components["schemas"]["ResumeConnectionSessionResponse"];
export type ConnectionSessionHealth = components["schemas"]["LiveConnectionHealthResponse"];
export type ConnectionSessionHealthLeg = components["schemas"]["LiveConnectionLegHealthResponse"];

function toConnectionTarget(raw: BackendConnectionTarget): ConnectionTarget {
  const roleIcon = extractPersistedIcon(raw.role_custom_fields);
  // The id / *_id fields are id columns from the typed backend contract; mint
  // them as `Uuid` for the UI FK boundary. This is a read boundary, so use
  // `as Uuid` rather than a runtime assert — a per-row throw would white-screen
  // the whole target list on one malformed row and adds no compile-time value.
  return {
    id: raw.id as Uuid,
    name: raw.name,
    targetKind: raw.target_kind,
    // Default to false so non-video targets (or an older/partial payload) never
    // surface the video affordances.
    isCctv: raw.is_cctv ?? false,
    family: raw.family ?? null,
    status: toResourceStatus(raw.status),
    site: raw.site_name ?? undefined,
    tenant: raw.tenant_name ?? undefined,
    role: raw.role_name ?? undefined,
    roleIconName: roleIcon.name,
    roleIconDataUrl: roleIcon.dataUrl,
    platform: raw.platform_name ?? undefined,
    deviceType: raw.device_type_name ?? undefined,
    primaryIp: raw.address ?? undefined,
    // Default to true so targets aren't flagged when the field is absent (older backend / partial payload).
    addressBound: raw.address_bound ?? true,
    region: raw.region_name ?? undefined,
    cluster: raw.cluster_name ?? undefined,
    siteId: (raw.site_id ?? undefined) as Uuid | undefined,
    tenantId: (raw.tenant_id ?? undefined) as Uuid | undefined,
    regionId: (raw.region_id ?? undefined) as Uuid | undefined,
    roleId: (raw.role_id ?? undefined) as Uuid | undefined,
    platformId: (raw.platform_id ?? undefined) as Uuid | undefined,
    deviceTypeId: (raw.device_type_id ?? undefined) as Uuid | undefined,
    clusterId: (raw.cluster_id ?? undefined) as Uuid | undefined,
    deviceId: (raw.device_id ?? undefined) as Uuid | undefined,
    vmId: (raw.vm_id ?? undefined) as Uuid | undefined,
  };
}

function toResourceStatus(raw: string | null | undefined): ResourceStatus | undefined {
  if (
    raw === "active" ||
    raw === "planned" ||
    raw === "staged" ||
    raw === "decommissioning" ||
    raw === "offline"
  ) {
    return raw;
  }
  return undefined;
}

// ── Sort + multi-value filter query helper ────────────────
//
// Mirrors the inventory table-fetch contract (`buildInventoryTableParams`): the
// backend list routes accept an allowlisted `sort` key + `order` (default asc)
// and comma-separated multi-value `IN` filters. Kept local to the connections
// feature so the feature stays decoupled from inventory.

/** Backend param name -> selected values (joined with `,`). */
export interface ConnectionTableQuery {
  limit: number;
  offset: number;
  search?: string;
  sort?: string;
  order?: "asc" | "desc";
  filters?: Record<string, string[]>;
}

type LooseListQuery = Record<string, string | number | boolean | undefined>;

/** Build the flat query object sent to a sort/filter-aware connections list
 *  endpoint. Drops empty filter value lists and only attaches `order` when a
 *  `sort` key is present. */
export function buildConnectionTableParams(query: ConnectionTableQuery): LooseListQuery {
  const params: LooseListQuery = { limit: query.limit, offset: query.offset };
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

// ── Connection targets (workspace) ────────────────────────

/**
 * Canonical TanStack Query key for the full connection target list.
 *
 * `listConnectionTargets()` always fetches every target (it paginates internally), so the
 * workspace tree and the active-sessions enrichment lookup share one cache
 * entry instead of loading all targets twice under divergent keys. Invalidations
 * after session open/close mutations must target this constant.
 */
export const CONNECTION_TARGETS_QUERY_KEY = ["connections", "targets"] as const;

export async function getConnectionTarget(targetId: string, signal?: AbortSignal): Promise<ConnectionTarget> {
  const target = unwrap(
    await apiClient.GET("/api/v1/connections/targets/{id}", {
      params: { path: { id: targetId } },
      signal,
    }),
  );
  return toConnectionTarget(target);
}

/**
 * Background refresh cadence for the connections workspace target list.
 *
 * Picks up changes other users made without any manual reload. Updates are silent:
 * TanStack structural sharing keeps unchanged rows referentially identical (zero
 * re-render when nothing changed), and the tree's expand/scroll/selection state is
 * keyed by stable ids, so a changed row updates in place without rebuilding.
 *
 * 10s (the conservative end of the desired 5-10s) balances freshness against the
 * full-list payload re-fetched per visible client. A lighter change-detection
 * endpoint would be the prerequisite for going faster.
 */
export const CONNECTION_WORKSPACE_REFRESH_MS = 10_000;

// Single-fetch page size: the workspace tree needs every target, and the backend
// recomputes the UNION read-model per page, so fanning out N paged round-trips
// recomputed everything N times (and ran a COUNT per page). One large request
// computes the projection once and lets the backend skip COUNT entirely. Must
// stay <= the backend cap (`MAX_CONNECTION_TARGET_LIST_LIMIT`). Above this size
// the loop below still pages as a safety net — the signal that the precomputed
// read-model (materialized view) is due.
const CONNECTION_TARGETS_PAGE_SIZE = 50_000;

/**
 * One server page of connection targets — the single owner of the paged
 * `/connections/targets` read. The backend applies `search`, sort and filters
 * before paging, so a picker never needs the whole inventory to find a match.
 * Full-list hydration and the name search below build on this.
 */
export async function listConnectionTargetPage(
  query: ConnectionTableQuery,
): Promise<Paginated<ConnectionTarget>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/connections/targets", {
      params: { query: buildConnectionTableParams(query) },
    }),
  );
  return {
    items: raw.items.map(toConnectionTarget),
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

export async function listConnectionTargets(
  q?: ListQuery & Pick<ConnectionTableQuery, "search" | "sort" | "order" | "filters">,
): Promise<Paginated<ConnectionTarget>> {
  // One request fetches the whole list (page size covers realistic inventories),
  // so the backend computes the expensive UNION read-model once and skips COUNT.
  // The parallel fan-out below only runs as a safety net when an inventory
  // exceeds the single-page size — at which point the precomputed read-model is
  // the real fix, not more concurrent recomputes.
  const first = await listConnectionTargetPage({ ...q, limit: CONNECTION_TARGETS_PAGE_SIZE, offset: 0 });
  const total = first.total;
  const items: ConnectionTarget[] = [...first.items];

  if (first.items.length === CONNECTION_TARGETS_PAGE_SIZE && items.length < total) {
    const offsets: number[] = [];
    for (let offset = CONNECTION_TARGETS_PAGE_SIZE; offset < total; offset += CONNECTION_TARGETS_PAGE_SIZE) {
      offsets.push(offset);
    }
    const pages = await Promise.all(
      offsets.map((offset) =>
        listConnectionTargetPage({ ...q, limit: CONNECTION_TARGETS_PAGE_SIZE, offset }),
      ),
    );
    for (const page of pages) items.push(...page.items);
  }

  return { items, total, limit: items.length, offset: 0 };
}

/**
 * Server-side connection target name/address search for entity pickers. Unlike
 * {@link listConnectionTargets} (which loads the entire target list), this hits
 * the backend `search` filter so a picker over thousands of targets only fetches
 * a bounded page of matches.
 * Returns lightweight `{ id, name }` rows.
 */
export async function searchConnectionTargets(
  search: string,
  limit = 50,
): Promise<{ id: string; name: string }[]> {
  const term = search.trim();
  const page = await listConnectionTargetPage({ limit, offset: 0, search: term || undefined });
  return page.items.map((h) => ({ id: h.id, name: h.name }));
}

// ── Sessions ─────────────────────────────────────────────

function toSessionProtocol(raw: string): SessionProtocol {
  if (raw === "ssh" || raw === "rdp" || raw === "vnc") return raw;
  throw new Error(`Unsupported session protocol: ${raw}`);
}

function toConnectionSessionStatus(raw: string): ConnectionSessionStatus {
  if (
    raw === "pending" ||
    raw === "active" ||
    raw === "disconnected" ||
    raw === "terminated" ||
    raw === "failed"
  ) {
    return raw;
  }
  throw new Error(`Unsupported connection session status: ${raw}`);
}

function toObserveCapability(value: unknown): ActiveSession["observe"] {
  if (!value || typeof value !== "object") return null;
  const data = value as Record<string, unknown>;
  if (typeof data.allowed !== "boolean" || (data.reason != null && typeof data.reason !== "string") ||
      (data.websocket_url != null && typeof data.websocket_url !== "string") ||
      (data.transport != null && data.transport !== "terminal" && data.transport !== "guacamole")) return null;
  return { allowed: data.allowed, reason: data.reason as string | null ?? null, websocket_url: data.websocket_url as string | null ?? null, transport: data.transport as "terminal" | "guacamole" | null ?? null };
}

function toPresence(raw: BackendActiveSession["presence"]): ActiveSession["presence"] {
  if (!raw || typeof raw !== "object") return null;
  const state = ["connecting", "live", "detached", "degraded", "ended", "unknown"].includes(raw.state)
    ? raw.state as NonNullable<ActiveSession["presence"]>["state"] : "unknown";
  return {
    state,
    generation: typeof raw.generation === "string" ? raw.generation : null,
    operator_attached: typeof raw.operator_attached === "boolean" ? raw.operator_attached : null,
    observer_count: typeof raw.observer_count === "number" && Number.isInteger(raw.observer_count) && raw.observer_count >= 0 ? raw.observer_count : null,
    observed_at: typeof raw.observed_at === "string" ? raw.observed_at : null,
    last_operator_activity_at: typeof raw.last_operator_activity_at === "string" ? raw.last_operator_activity_at : null,
    reconnect_deadline: typeof raw.reconnect_deadline === "string" ? raw.reconnect_deadline : null,
    ended_at: typeof raw.ended_at === "string" ? raw.ended_at : null,
  };
}

function toActiveSession(raw: BackendActiveSession): ActiveSession {
  return {
    id: raw.id,
    userId: raw.user_id,
    hostId: raw.target_id,
    protocol: toSessionProtocol(raw.protocol),
    status: toConnectionSessionStatus(raw.status),
    connectedAt: raw.started_at,
    lastActivity: raw.last_activity,
    endedAt: raw.ended_at,
    presence: toPresence(raw.presence),
    observe: toObserveCapability(raw.observe),
    clientIp: raw.client_ip ?? undefined,
    proxyNodeName: raw.proxy_node_name ?? undefined,
  };
}

/** GET /api/v1/connections/sessions — server-paged session catalogue. */
export async function listActiveSessions(
  q?: Pick<ConnectionTableQuery, "limit" | "offset" | "sort" | "order" | "filters" | "search"> & { view?: "active" | "history" },
): Promise<Paginated<ActiveSession>> {
  const params = buildConnectionTableParams({
    limit: q?.limit ?? 0,
    offset: q?.offset ?? 0,
    search: q?.search,
    sort: q?.sort,
    order: q?.order,
    filters: q?.filters,
  });
  params.view = q?.view ?? "active";
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: "/api/v1/connections/sessions",
      init: { params: { query: LooseListQuery } },
    ) => Promise<{ data?: { items?: BackendActiveSession[]; total: number; limit: number; offset: number }; error?: unknown }>)(
      "/api/v1/connections/sessions",
      { params: { query: params } },
    ),
  );
  const items = (raw.items ?? []).map(toActiveSession);
  return { items, total: raw.total, limit: raw.limit, offset: raw.offset };
}

/**
 * Drop empty/whitespace-only credential fields so the inline `params.credentials`
 * object only carries values the user actually provided. Returns `undefined`
 * when nothing is left, so an empty object is never sent.
 */
function compactCredentials(
  credentials: OpenSessionRequest["credentials"],
): Record<string, string> | undefined {
  if (!credentials) return undefined;
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(credentials)) {
    const trimmed = typeof value === "string" ? value.trim() : "";
    if (trimmed) out[key] = trimmed;
  }
  return Object.keys(out).length ? out : undefined;
}

/** POST /api/v1/connections/sessions — open a new session. */
export async function openSession(body: OpenSessionRequest): Promise<OpenSessionResponse> {
  // Ad-hoc credentials ride on the request body's `params.credentials` (flat
  // launch-params shape). With no `entryId` the backend uses them directly;
  // with one it overrides the selected account's missing fields.
  const credentials = compactCredentials(body.credentials);
  const params: Record<string, unknown> = { ...(body.params ?? {}) };
  if (credentials) params.credentials = credentials;

  // A local quality preset overrides the connection config's colour depth and
  // lossless setting. Callers that pass explicit options win over the stored
  // preference; the preference only fills what the caller did not specify.
  const preference = loadSessionViewSettings(body.hostId, body.protocol);
  const viewOptions = sessionViewLaunchOptions(body.protocol, preference.quality);
  if (viewOptions) {
    const explicitOptions =
      typeof params.options === "object" && params.options !== null
        ? (params.options as Record<string, unknown>)
        : {};
    params.options = { ...viewOptions, ...explicitOptions };
  }

  const raw: BackendOpenSessionResponse = unwrap(
    await apiClient.POST("/api/v1/connections/sessions", {
      body: {
        target_id: body.hostId,
        protocol: body.protocol,
        ...(body.entryId !== undefined ? { metadata: { entryId: body.entryId } } : {}),
        ...(Object.keys(params).length ? { params } : {}),
      },
    }),
  );
  trackProductEvent("connection_session_started");
  return {
    session: toActiveSession(raw.session),
    proxy: {
      connectionId: raw.proxy.connection_id,
      websocketUrl: raw.proxy.websocket_url,
      token: raw.proxy.token,
      tokenTtlSeconds: raw.proxy.token_ttl_seconds,
    },
  };
}

/** POST /api/v1/connections/sessions/{id}/resume — issue fresh WS details for an active session. */
export async function resumeSession(id: string): Promise<OpenSessionResponse> {
  const raw: BackendResumeSessionResponse = unwrap(
    await apiClient.POST("/api/v1/connections/sessions/{id}/resume", {
      params: { path: { id } },
    }),
  );
  return {
    session: toActiveSession(raw.session),
    proxy: {
      connectionId: raw.proxy.connection_id,
      websocketUrl: raw.proxy.websocket_url,
      token: raw.proxy.token,
      tokenTtlSeconds: raw.proxy.token_ttl_seconds,
    },
  };
}

/** GET /api/v1/connections/sessions/{id}/health — live route health snapshot. */
export async function getSessionHealth(id: string): Promise<ConnectionSessionHealth> {
  return unwrap(
    await apiClient.GET("/api/v1/connections/sessions/{id}/health", {
      params: { path: { id } },
    }),
  );
}

/** POST /api/v1/connections/sessions/{id}/disconnect — graceful. */
export async function closeSession(id: string): Promise<void> {
  await apiClient.POST("/api/v1/connections/sessions/{id}/disconnect", {
    params: { path: { id } },
  });
}

/** POST /api/v1/connections/sessions/{id}/terminate — forceful. */
export async function terminateSession(id: string): Promise<void> {
  await apiClient.POST("/api/v1/connections/sessions/{id}/terminate", {
    params: { path: { id } },
  });
}

/** Shape returned by `GET /api/v1/connections/sessions/{id}/client-credentials`. */
export interface SessionClientCredentials {
  session_id: string;
  protocol: string;
  credentials: Record<string, string>;
}

type BackendLaunchAccount = components["schemas"]["ConnectionLaunchAccountResponse"];
type BackendLaunchProtocol = components["schemas"]["ConnectionLaunchProtocolResponse"];

function toLaunchAccount(raw: BackendLaunchAccount): ConnectionLaunchOptions["protocols"][number]["accounts"][number] {
  return {
    entryId: raw.entry_id,
    kind: raw.kind === "personal" ? "personal" : "shared",
    label: raw.label,
    color: raw.color,
    icon: raw.icon,
    username: raw.username,
    uri: raw.uri,
    source: {
      credentialSetName: raw.source.credential_set_name,
      recordName: raw.source.record_name,
      scopeType: raw.source.scope_type,
      scopeId: raw.source.scope_id,
    },
    isDefault: raw.is_default,
    requiresInput: raw.requires_input,
  };
}

const LAUNCH_PROTOCOLS = ["ssh", "rdp", "vnc", "app", "cctv"] as const;
type FrontendLaunchProtocol = (typeof LAUNCH_PROTOCOLS)[number];

function isKnownLaunchProtocol(value: string): value is FrontendLaunchProtocol {
  return (LAUNCH_PROTOCOLS as readonly string[]).includes(value);
}

/**
 * Map one wire launch-protocol bucket, or `null` for a protocol the frontend
 * does not model. Returning `null` (dropped by the caller) is deliberate: an
 * unknown protocol must NOT masquerade as `ssh` — that is exactly the bug that
 * turned a `cctv` (formerly `video`) bucket into a phantom SSH connect button.
 */
function toLaunchProtocol(raw: BackendLaunchProtocol): ConnectionLaunchProtocol | null {
  if (!isKnownLaunchProtocol(raw.protocol)) return null;
  const render = (raw.render === "buttons" || raw.render === "split")
    ? raw.render
    : "auto" as const;
  return {
    protocol: raw.protocol,
    render,
    accounts: raw.accounts.filter((account) => typeof account.entry_id === "string" && account.entry_id.trim().length > 0).map(toLaunchAccount),
  };
}

/** GET /api/v1/connections/targets/{id}/launch-options */
export async function getConnectionLaunchOptions(
  hostId: string,
  signal?: AbortSignal,
): Promise<ConnectionLaunchOptions> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/connections/targets/{id}/launch-options", {
      params: { path: { id: hostId } },
      signal,
    }),
  );
  if (raw.host_id !== hostId) throw new Error("Launch options target does not match request");
  return {
    hostId: raw.host_id,
    protocols: raw.protocols.map(toLaunchProtocol).filter(
      (protocol): protocol is ConnectionLaunchProtocol => protocol !== null,
    ),
  };
}

/** GET /api/v1/connections/sessions/{id}/client-credentials */
export async function getSessionClientCredentials(
  sessionId: string,
): Promise<SessionClientCredentials> {
  return unwrap(
    await apiClient.GET("/api/v1/connections/sessions/{id}/client-credentials", {
      params: { path: { id: sessionId } },
    }),
  ) as unknown as SessionClientCredentials;
}
