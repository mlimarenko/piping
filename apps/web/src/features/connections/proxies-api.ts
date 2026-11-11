import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";
import type { ListQuery, Paginated, ProxyNode, ProxyNodeAssignment } from "@/shared/api/types";

type CreateProxyNodeRequest = components["schemas"]["CreateProxyNodeRequest"];
type UpdateProxyNodeRequest = components["schemas"]["UpdateProxyNodeRequest"];
type CreateProxyNodeAssignmentRequest = components["schemas"]["CreateProxyNodeAssignmentRequest"];
type UpdateProxyNodeAssignmentRequest = components["schemas"]["UpdateProxyNodeAssignmentRequest"];
export type ProxyNodeBootstrap = components["schemas"]["ProxyNodeBootstrapResponse"];
export type CreatedProxyNodeResponse = components["schemas"]["CreatedProxyNodeResponse"];

export interface ProxyNodeAssignmentListQuery extends ListQuery {
  proxy_node_id?: string;
  scope_type?: string;
  scopeId?: string;
  enabled?: string;
}

// ── Sort + multi-value filter query shape ─────────────────
//
// Mirrors the inventory/connections table-fetch contract: an allowlisted `sort`
// key + `order` (default asc) and comma-separated multi-value `IN` filters.

/** Backend filter param -> selected values (joined with `,`). */
export interface ProxyTableSort {
  sort?: string;
  order?: "asc" | "desc";
}

type LooseProxyQuery = Record<string, string | number | boolean | undefined>;

/** Attach `sort` (+ `order`, default asc) to a loose proxy list query. */
function withProxySort(params: LooseProxyQuery, sort?: ProxyTableSort): LooseProxyQuery {
  if (sort?.sort) {
    params.sort = sort.sort;
    params.order = sort.order ?? "asc";
  }
  return params;
}

export async function listProxyNodes(
  q?: ListQuery & ProxyTableSort & { enabled?: string },
): Promise<Paginated<ProxyNode>> {
  const query = withProxySort(
    { limit: q?.limit, offset: q?.offset, search: q?.search, enabled: q?.enabled },
    q,
  );
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: "/api/v1/admin/proxy-nodes",
      init: { params: { query: LooseProxyQuery } },
    ) => Promise<{ data?: { items?: ProxyNode[]; total: number; limit: number; offset: number }; error?: unknown }>)(
      "/api/v1/admin/proxy-nodes",
      { params: { query } },
    ),
  );
  return {
    items: raw.items ?? [],
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

export async function createProxyNode(
  body: CreateProxyNodeRequest,
): Promise<CreatedProxyNodeResponse> {
  return unwrap(
    await (apiClient.POST as unknown as (
      path: "/api/v1/admin/proxy-nodes",
      init: { body: CreateProxyNodeRequest },
    ) => Promise<{ data?: CreatedProxyNodeResponse; error?: unknown }>)(
      "/api/v1/admin/proxy-nodes",
      { body },
    ),
  );
}

export async function updateProxyNode(
  id: string,
  body: UpdateProxyNodeRequest,
): Promise<ProxyNode> {
  return unwrap(
    await apiClient.PUT("/api/v1/admin/proxy-nodes/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteProxyNode(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/proxy-nodes/{id}", {
    params: { path: { id } },
  });
}

export async function rotateProxyNodeSecret(id: string): Promise<CreatedProxyNodeResponse> {
  return unwrap(
    await (apiClient.POST as unknown as (
      path: "/api/v1/admin/proxy-nodes/{id}/rotate-secret",
      init: { params: { path: { id: string } } },
    ) => Promise<{ data?: CreatedProxyNodeResponse; error?: unknown }>)(
      "/api/v1/admin/proxy-nodes/{id}/rotate-secret",
      { params: { path: { id } } },
    ),
  );
}

export async function listProxyNodeAssignments(
  q?: ProxyNodeAssignmentListQuery & ProxyTableSort,
): Promise<Paginated<ProxyNodeAssignment>> {
  // `proxy_node_id` and `scope_type` accept comma-separated multi-value `IN`
  // lists; the caller joins selected filter values before passing them here.
  const query = withProxySort(
    {
      proxy_node_id: q?.proxy_node_id,
      scope_type: q?.scope_type,
      scope_id: q?.scopeId,
      enabled: q?.enabled,
      limit: q?.limit,
      offset: q?.offset,
    },
    q,
  );
  const raw = unwrap(
    await (apiClient.GET as unknown as (
      p: "/api/v1/admin/proxy-node-assignments",
      init: { params: { query: LooseProxyQuery } },
    ) => Promise<{ data?: { items?: ProxyNodeAssignment[]; total: number; limit: number; offset: number }; error?: unknown }>)(
      "/api/v1/admin/proxy-node-assignments",
      { params: { query } },
    ),
  );
  return {
    items: raw.items ?? [],
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

export async function createProxyNodeAssignment(
  body: CreateProxyNodeAssignmentRequest,
): Promise<ProxyNodeAssignment> {
  return unwrap(
    await apiClient.POST("/api/v1/admin/proxy-node-assignments", { body }),
  );
}

export async function updateProxyNodeAssignment(
  id: string,
  body: UpdateProxyNodeAssignmentRequest,
): Promise<ProxyNodeAssignment> {
  return unwrap(
    await apiClient.PUT("/api/v1/admin/proxy-node-assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteProxyNodeAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/proxy-node-assignments/{id}", {
    params: { path: { id } },
  });
}
