import type { TreeGroupKey } from "./treeConfig";
import { readRetiredLocalJson } from "@/shared/settings";

export const TREE_COLLAPSED_PREFERENCE_KEY = "connections.tree.collapsed";
export const RETIRED_TREE_COLLAPSED_STORAGE_KEY = "piping.connections.tree.collapsed";

export interface PersistedTreeCollapsedGroups {
  version: 1;
  byScope: Record<string, string[]>;
}

export const EMPTY_TREE_COLLAPSED_GROUPS: PersistedTreeCollapsedGroups = {
  version: 1,
  byScope: {},
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function treeCollapseScope(
  order: TreeGroupKey[],
  includeUnknown: boolean,
): string {
  return `v1:${includeUnknown ? "unknown" : "known"}:${order.join(">")}`;
}

export function sanitizeCollapsedTreeKeys(
  values: Iterable<unknown>,
  validKeys?: Set<string>,
): string[] {
  const keys = new Set<string>();
  for (const value of values) {
    if (typeof value !== "string") continue;
    const key = value.trim();
    if (!key) continue;
    if (validKeys && !validKeys.has(key)) continue;
    keys.add(key);
  }
  return [...keys].sort((a, b) => a.localeCompare(b));
}

export function normalizeTreeCollapsedGroups(
  value: unknown,
): PersistedTreeCollapsedGroups {
  if (!isRecord(value) || !isRecord(value.byScope)) {
    return EMPTY_TREE_COLLAPSED_GROUPS;
  }

  const byScope: Record<string, string[]> = {};
  for (const [scope, rawKeys] of Object.entries(value.byScope)) {
    if (!Array.isArray(rawKeys)) continue;
    const keys = sanitizeCollapsedTreeKeys(rawKeys);
    byScope[scope] = keys;
  }
  return { version: 1, byScope };
}

export function collapsedTreeGroupSetForScope(
  value: unknown,
  scope: string,
  validKeys: Set<string>,
  keysFromRetiredScope: string[] = [],
): Set<string> {
  const normalized = normalizeTreeCollapsedGroups(value);
  const keys = Object.prototype.hasOwnProperty.call(normalized.byScope, scope)
    ? (normalized.byScope[scope] ?? [])
    : keysFromRetiredScope;
  return new Set(sanitizeCollapsedTreeKeys(keys, validKeys));
}

export function withCollapsedTreeGroupScope(
  value: unknown,
  scope: string,
  keys: Iterable<unknown>,
  validKeys: Set<string>,
): PersistedTreeCollapsedGroups {
  const normalized = normalizeTreeCollapsedGroups(value);
  const nextKeys = sanitizeCollapsedTreeKeys(keys, validKeys);
  const byScope = { ...normalized.byScope };
  byScope[scope] = nextKeys;
  return { version: 1, byScope };
}

export function readRetiredCollapsedTreeGroups(): string[] {
  try {
    const parsed = readRetiredLocalJson(
      RETIRED_TREE_COLLAPSED_STORAGE_KEY,
      (value) => value,
    );
    return Array.isArray(parsed)
      ? sanitizeCollapsedTreeKeys(parsed)
      : [];
  } catch {
    return [];
  }
}
