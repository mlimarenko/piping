/**
 * Sole builder/parser for the connections-workspace host deep-link contract:
 *
 *   /connections?host=<connectionTargetId>
 *
 * Opening this URL selects the target and opens its inspector in the workspace
 * (in any view). Both the inspectors (link source) and ConnectionsWorkspace
 * (link target) go through these helpers so the grammar lives in one place. The
 * id is the ConnectionTarget id — what the workspace keys hosts on — NOT the
 * underlying device/VM UUID.
 */

export const CONNECTIONS_PATH = "/connections";
export const CONNECTIONS_HOST_PARAM = "host";

/** Relative href to the connections workspace focused on a host. */
export function connectionsHostHref(targetId: string): string {
  return `${CONNECTIONS_PATH}?${CONNECTIONS_HOST_PARAM}=${encodeURIComponent(targetId)}`;
}

/** Absolute, shareable URL focusing a host in the connections workspace. */
export function connectionsHostUrl(targetId: string): string {
  return `${window.location.origin}${connectionsHostHref(targetId)}`;
}
