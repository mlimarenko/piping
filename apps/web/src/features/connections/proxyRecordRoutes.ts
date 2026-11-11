export const PROXY_NODES_ROUTE = "/connections/proxies";

export function proxyCollectionHref(collection: "nodes" | "assignments", params = new URLSearchParams()): string {
  const next = new URLSearchParams(params);
  next.set("collection", collection);
  return `${PROXY_NODES_ROUTE}?${next}`;
}

export const PROXY_NODE_NEW_ROUTE = `${PROXY_NODES_ROUTE}/new`;

export const PROXY_ASSIGNMENTS_ROUTE = `${PROXY_NODES_ROUTE}/assignments`;

export const PROXY_ASSIGNMENT_NEW_ROUTE = `${PROXY_ASSIGNMENTS_ROUTE}/new`;

export function proxyNodeHref(id: string): string {
  return `${PROXY_NODES_ROUTE}/${encodeURIComponent(id)}`;
}

export function proxyNodeEditHref(id: string): string {
  return `${proxyNodeHref(id)}/edit`;
}

export function proxyAssignmentEditHref(id: string): string {
  return `${PROXY_ASSIGNMENTS_ROUTE}/${encodeURIComponent(id)}/edit`;
}
