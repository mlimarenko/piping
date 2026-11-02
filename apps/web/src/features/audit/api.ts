import { apiClient, unwrap } from "@/shared/api/client";
import type { components, operations } from "@/shared/api/schema.gen";
import type { AuditEntry, AuditLookupOption, AuditObjectRef, AuditQuery, Paginated } from "@/shared/api/types";

type BackendAuditEvent = components["schemas"]["AuditEventResponse"];
type BackendAuditEnvelope = components["schemas"]["AuditEventListResponse"];
type BackendAuditTargetEnvelope = components["schemas"]["AuditTargetLookupResponse"];
type BackendAuditObjectRef = {
  kind?: string | null;
  id?: string | null;
  label?: string | null;
  secondary?: string | null;
};
type HydratedBackendAuditEvent = BackendAuditEvent & {
  actor?: BackendAuditObjectRef | null;
  actor_via?: BackendAuditObjectRef | null;
  target?: BackendAuditObjectRef | null;
  resource?: BackendAuditObjectRef | null;
};

function toAuditDetails(metadata: BackendAuditEvent["metadata"]): Record<string, unknown> | undefined {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) return undefined;
  return metadata as Record<string, unknown>;
}

function toAuditObjectRef(raw?: BackendAuditObjectRef | null): AuditObjectRef | undefined {
  if (!raw || !raw.kind || !raw.label) return undefined;
  return {
    kind: raw.kind,
    id: raw.id ?? undefined,
    label: raw.label,
    secondary: raw.secondary ?? undefined,
  };
}

function toAuditEntry(raw: BackendAuditEvent): AuditEntry {
  const hydrated = raw as HydratedBackendAuditEvent;
  const actorRef = toAuditObjectRef(hydrated.actor);
  const actorVia = toAuditObjectRef(hydrated.actor_via);
  const targetRef = toAuditObjectRef(hydrated.target);
  const resourceRef = toAuditObjectRef(hydrated.resource);
  return {
    id: raw.id,
    timestamp: raw.created_at,
    actor: actorRef?.label ?? raw.actor_id ?? "",
    actorKind: raw.actor_kind,
    action: raw.event_type,
    resourceType: resourceRef?.kind ?? targetRef?.kind ?? raw.target_type ?? "",
    resourceId: resourceRef?.id ?? targetRef?.id ?? raw.target_id ?? "",
    summary: raw.event_type,
    actorRef,
    actorVia,
    targetRef,
    resourceRef,
    details: toAuditDetails(raw.metadata),
  };
}

export interface AuditLookupQuery {
  targetType?: string;
  search?: string;
  limit?: number;
}

/**
 * Sort/multi-filter input for the audit feed table. Every dimension the backend
 * treats as a comma-separated `IN` filter is modelled here as a value array;
 * the corresponding scalar fields from {@link AuditQuery} are omitted so a
 * caller cannot send both shapes for the same param. `mcpTool` stays a free-text
 * field (the backend splits it on `,` too, so a comma-separated value still maps
 * to an `IN`). `sort`/`order` map to the backend's allowlisted sort contract
 * (`created_at, event_type, actor, target_type, result`).
 */
export interface AuditTableQuery
  extends Omit<
    AuditQuery,
    "action" | "resourceType" | "result" | "actor" | "targetId" | "mcpToolGroup" | "mcpProtocol" | "mcpRisk"
  > {
  /** Backend `event_type` IN filter (joined with `,`). */
  eventTypes?: string[];
  /** Backend `actor_kind` (channel) IN filter (joined with `,`). */
  actorKinds?: string[];
  /** Backend `actor_id` IN filter (joined with `,`). */
  actorIds?: string[];
  /** Backend `target_type` IN filter (joined with `,`). */
  targetTypes?: string[];
  /** Backend `target_id` IN filter (joined with `,`). */
  targetIds?: string[];
  /** Backend `result` IN filter (joined with `,`). */
  results?: string[];
  /** Backend `mcp_tool_group` IN filter (joined with `,`). */
  mcpToolGroups?: string[];
  /** Backend `mcp_protocol` IN filter (joined with `,`). */
  mcpProtocols?: string[];
  /** Backend `mcp_risk` IN filter (joined with `,`). */
  mcpRisks?: string[];
  sort?: string;
  order?: "asc" | "desc";
}

/** The flat `list_audit_events` query object. Sourced from the generated schema
 *  operation so the typed `apiClient.GET` accepts it without a cast. */
type AuditEventParams = NonNullable<operations["list_audit_events"]["parameters"]["query"]>;

/** Build the flat `/audit/events` query from the table input. Empty filter
 *  arrays are dropped (no param sent); multi-value arrays are comma-joined into
 *  the backend's `IN` form. Exported for unit testing the param contract — the
 *  new backend filtering is not reachable from the local stack. */
export function buildAuditEventParams(q?: AuditTableQuery): AuditEventParams {
  const join = (values?: string[]): string | undefined =>
    values && values.length > 0 ? values.join(",") : undefined;
  return {
    q: q?.query,
    event_type: join(q?.eventTypes),
    actor_kind: join(q?.actorKinds),
    actor_id: join(q?.actorIds),
    target_type: join(q?.targetTypes),
    target_id: join(q?.targetIds),
    result: join(q?.results),
    mcp_tool: q?.mcpTool,
    mcp_tool_group: join(q?.mcpToolGroups),
    mcp_protocol: join(q?.mcpProtocols),
    mcp_risk: join(q?.mcpRisks),
    created_from: q?.from,
    created_to: q?.to,
    limit: q?.limit,
    offset: q?.offset,
    ...(q?.sort ? { sort: q.sort, order: q.order ?? "asc" } : {}),
  };
}

/** GET /api/v1/audit/events — canonical backend audit feed; returns `{items,total,limit,offset}`. */
export async function listAuditLogs(q?: AuditTableQuery): Promise<Paginated<AuditEntry>> {
  const raw: BackendAuditEnvelope = unwrap(
    await apiClient.GET("/api/v1/audit/events", {
      params: { query: buildAuditEventParams(q) },
    }),
  );
  return {
    items: (raw.items ?? []).map(toAuditEntry),
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

/** GET /api/v1/audit/targets — server-side hydrated lookup for audit filter pickers. */
export async function searchAuditTargets(q?: AuditLookupQuery): Promise<AuditLookupOption[]> {
  const raw: BackendAuditTargetEnvelope = unwrap(
    await apiClient.GET("/api/v1/audit/targets", {
      params: {
        query: {
          target_type: q?.targetType,
          search: q?.search,
          limit: q?.limit,
        },
      },
    }),
  );
  return raw.items ?? [];
}
