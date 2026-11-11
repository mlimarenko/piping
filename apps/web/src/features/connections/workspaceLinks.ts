/**
 * Sole builder/parser for the connections-workspace host deep-link contract:
 *
 *   /connections?host=<connectionTargetId>
 *
 * Opening this URL selects the target and opens its inspector in the workspace
 * (in any view), revealing it in cards or expanding its ancestors in the tree.
 * The identity remains in the URL until the inspector is closed.
 * Both the inspectors (link source) and ConnectionsWorkspace
 * (link target) go through these helpers so the grammar lives in one place. The
 * id is the ConnectionTarget id — what the workspace keys hosts on — NOT the
 * underlying device/VM UUID.
 */

export const CONNECTIONS_PATH = "/connections";
export const CONNECTIONS_HOST_PARAM = "host";
export const CONNECTIONS_GROUP_PARAM = "group";

import { ALL_GROUPS, type TreeGroupKey } from "./components/connections/treeConfig";

export interface ConnectionsGroupIdentity {
  type: TreeGroupKey;
  path: string;
  anchor: string;
  entityId: string | null;
}

export function parseConnectionsGroupIdentity(value: string | null): ConnectionsGroupIdentity | null {
  if (!value || value.length > 4096) return null;
  try {
    const raw: unknown = JSON.parse(value);
    if (!Array.isArray(raw) || raw.length !== 4) return null;
    const values: unknown[] = raw;
    const type = values[0];
    const path = values[1];
    const anchor = values[2];
    const entityId = values[3];
    if (typeof type !== "string" || !ALL_GROUPS.includes(type as TreeGroupKey) ||
      typeof path !== "string" || !path || typeof anchor !== "string" || !anchor ||
      !(entityId === null || (typeof entityId === "string" && entityId.length > 0))) return null;
    return { type: type as TreeGroupKey, path, anchor, entityId };
  } catch {
    return null;
  }
}

export function serializeConnectionsGroupIdentity(identity: ConnectionsGroupIdentity): string {
  return JSON.stringify([identity.type, identity.path, identity.anchor, identity.entityId]);
}

/** Relative href to the connections workspace focused on a host. */
export function connectionsHostHref(targetId: string): string {
  return `${CONNECTIONS_PATH}?${CONNECTIONS_HOST_PARAM}=${encodeURIComponent(targetId)}`;
}

/** Absolute, shareable URL focusing a host in the connections workspace. */
export function connectionsHostUrl(targetId: string): string {
  return `${window.location.origin}${connectionsHostHref(targetId)}`;
}
