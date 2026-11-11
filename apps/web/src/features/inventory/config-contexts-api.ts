import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";
import type { ConfigContext, ListQuery, Paginated } from "@/shared/api/types";

function isJsonObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Parse the editor's JSON text into the object the wire contract requires. */
function parseContextData(raw: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("Config context data must be valid JSON");
  }
  if (!isJsonObject(parsed)) {
    throw new Error("Config context data must be a JSON object");
  }
  return parsed;
}

function toConfigContext(raw: components["schemas"]["ConfigContext"]): ConfigContext {
  return {
    id: raw.id,
    name: raw.name,
    description: raw.description ?? "",
    weight: raw.weight,
    active: raw.is_active,
    data: JSON.stringify(raw.data, null, 2),
    createdAt: raw.created_at ?? undefined,
    updatedAt: raw.updated_at ?? undefined,
  };
}

function toBackendPayload(
  body: Partial<ConfigContext>,
): components["schemas"]["ConfigContextPayload"] {
  return {
    name: body.name ?? "",
    description: body.description,
    weight: body.weight,
    is_active: body.active,
    data: body.data === undefined ? {} : parseContextData(body.data),
  };
}

export interface ConfigContextListQuery extends ListQuery {
  name?: string;
  isActive?: string;
  sort?: string;
  order?: "asc" | "desc";
}

export async function listContexts(q?: ConfigContextListQuery): Promise<Paginated<ConfigContext>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/config-contexts", {
      params: {
        query: {
          limit: q?.limit,
          offset: q?.offset,
          name: q?.name,
          is_active: q?.isActive,
          sort: q?.sort,
          order: q?.order,
        },
      },
    }),
  );
  return {
    items: (raw.items ?? []).map(toConfigContext),
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

export async function createContext(body: Omit<ConfigContext, "id">): Promise<ConfigContext> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/config-contexts", { body: toBackendPayload(body) }),
  );
  return toConfigContext(raw);
}

export async function updateContext(
  id: string,
  body: Partial<ConfigContext>,
): Promise<ConfigContext> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/config-contexts/{id}", {
      params: { path: { id } },
      body: toBackendPayload(body),
    }),
  );
  return toConfigContext(raw);
}

export async function deleteContext(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/config-contexts/{id}", {
    params: { path: { id } },
  });
}

/* ── Scope bindings ── */

/**
 * A config-context scope binding: what a context attaches to. The response
 * carries only the raw `scope_type` token + `scope_id` (no resolved name); the
 * UI resolves the display name client-side via the per-scope-type option list.
 */
export type ConfigContextBinding = components["schemas"]["ConfigContextBinding"];

export interface ConfigContextBindingListQuery extends ListQuery {
  scopeType?: string;
  sort?: string;
  order?: "asc" | "desc";
}

/** List the scope bindings for one config context. */
export async function listBindings(
  contextId: string,
  q?: ConfigContextBindingListQuery,
): Promise<Paginated<ConfigContextBinding>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/config-context-bindings", {
      params: {
        query: {
          context_id: contextId,
          scope_type: q?.scopeType,
          limit: q?.limit,
          offset: q?.offset,
          sort: q?.sort,
          order: q?.order,
        },
      },
    }),
  );
  return {
    items: raw.items ?? [],
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

/** Attach a config context to a scope (region/site/tenant/cluster/role/etc.). */
export async function createBinding(
  body: components["schemas"]["ConfigContextBindingPayload"],
): Promise<ConfigContextBinding> {
  return unwrap(
    await apiClient.POST("/api/v1/config-context-bindings", { body }),
  );
}

/** Remove a single scope binding by id. */
export async function deleteBinding(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/config-context-bindings/{id}", {
    params: { path: { id } },
  });
}

/* ── Rendered config context (device / VM) ── */

/**
 * The merged config context applied to a device or VM: inherited config
 * contexts plus the entity's `local_context_data` as the highest-priority
 * layer (local wins), with the contributing `sources` in resolution order.
 */
export type RenderedConfigContext = components["schemas"]["RenderedConfigContextResponse"];

/** Device/VM kinds that expose a rendered config context endpoint. */
export type ConfigContextEntityKind = "device" | "virtual-machine";

/** Fetch the rendered (merged) config context for a device or VM. */
export async function getRenderedConfigContext(
  kind: ConfigContextEntityKind,
  id: string,
  signal?: AbortSignal,
): Promise<RenderedConfigContext> {
  if (kind === "device") {
    return unwrap(
      await apiClient.GET("/api/v1/inventory/devices/{id}/config-context", {
        params: { path: { id } },
        signal,
      }),
    );
  }
  return unwrap(
    await apiClient.GET("/api/v1/inventory/virtual_machines/{id}/config-context", {
      params: { path: { id } },
      signal,
    }),
  );
}
