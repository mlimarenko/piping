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
  const actorRef = toAuditObjectRef(raw.actor);
  const targetRef = toAuditObjectRef(raw.target);
  return {
    id: raw.id,
    timestamp: raw.created_at,
    actor: actorRef?.label ?? raw.actor_id ?? "",
    actorKind: raw.actor_kind,
    action: raw.event_type,
    resourceType: raw.target_type ?? "",
    resourceId: raw.target_id ?? "",
    context: { ...raw.context, correlation: raw.context.correlation ?? {}, display: raw.context.display ?? {} },
    actorRef,
    targetRef,
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
 * caller cannot send both shapes for the same param. `sort`/`order` map to the backend's allowlisted sort contract
 * (`created_at, event_type, actor, target_type, outcome`).
 */
export interface AuditTableQuery
  extends Omit<
    AuditQuery,
    "action" | "resourceType" | "actor" | "targetId"
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
  outcomes?: string[];
  sort?: string;
  order?: "asc" | "desc";
}

/** The flat `list_audit_events` query object. Sourced from the generated schema
 *  operation so the typed `apiClient.GET` accepts it without a cast. */
type AuditEventParams = NonNullable<operations["list_audit_events"]["parameters"]["query"]>;

/** Build the flat `/audit/events` query from the table input. Empty filter
 *  arrays are dropped (no param sent); multi-value arrays are comma-joined into
 *  the backend's `IN` form. */
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
    outcome: join(q?.outcomes),
    state_domain: q?.stateDomain,
    state_value: q?.stateValue,
    request_id: q?.requestId,
    session_id: q?.sessionId,
    run_id: q?.runId,
    run_group_id: q?.runGroupId,
    created_from: q?.from,
    created_to: q?.to,
    limit: q?.limit,
    offset: q?.offset,
    ...(q?.sort ? { sort: q.sort, order: q.order ?? "asc" } : {}),
  };
}

export async function getAuditLog(id: string): Promise<AuditEntry> {
  const raw = unwrap(await apiClient.GET("/api/v1/audit/events/{id}", {
    params: { path: { id } },
  }));
  return toAuditEntry(raw);
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

/** GET /api/v1/audit/targets — recorded target identities. */
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
