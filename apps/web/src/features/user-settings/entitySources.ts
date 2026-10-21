import { searchConnectionTargets } from "@/features/connections/api";
import { listContexts } from "@/features/inventory/config-contexts-api";
import {
  listGroups,
  listPolicies,
  listRoles,
  listUsers,
} from "@/features/access-control/api";

/**
 * Entity sources for the token-scope "Advanced targeting" picker.
 *
 * A token scope (`action:resource`) can be narrowed from "all resources" (`*`)
 * to specific materialized entities. Each selected entity is encoded as a
 * resource pattern `<prefix>:<id>` where `<prefix>` is the backend
 * `ResourceType::as_str()` (underscore form), so it matches the authorization
 * layer's `resource_id` (`<type>:<id>`) via `matches_resource_pattern`.
 *
 * Sources resolve names by querying the real list endpoints. `serverSearch`
 * sources push the search term to the backend (for resource types with
 * thousands of rows, e.g. connection targets); the rest load a bounded set and
 * are filtered client-side by the picker.
 */
export interface EntityOption {
  id: string;
  name: string;
}

export interface EntitySource {
  /** resource_id type prefix — must equal the backend ResourceType::as_str(). */
  prefix: string;
  /** When true the search term is sent to the server; otherwise filter locally. */
  serverSearch: boolean;
  /** Fetch matching entities. `term` is honored only when `serverSearch`. */
  search: (term: string, limit: number) => Promise<EntityOption[]>;
}

export const ENTITY_SOURCES: Record<string, EntitySource> = {
  connection_target: {
    prefix: "connection_target",
    serverSearch: true,
    search: (term, limit) => searchConnectionTargets(term, limit),
  },
  config_context: {
    prefix: "config_context",
    serverSearch: false,
    search: async () =>
      (await listContexts()).items.map((c) => ({ id: c.id, name: c.name })),
  },
  user: {
    prefix: "user",
    serverSearch: true,
    search: async (term, limit) =>
      (await listUsers({ search: term, limit })).items.map((u) => ({
        id: u.id,
        name: u.username,
      })),
  },
  group: {
    prefix: "group",
    serverSearch: true,
    search: async (term, limit) =>
      (await listGroups({ search: term, limit })).items.map((g) => ({
        id: g.id,
        name: g.name,
      })),
  },
  role: {
    prefix: "role",
    serverSearch: true,
    search: async (term, limit) =>
      (await listRoles({ search: term, limit })).items.map((r) => ({
        id: r.id,
        name: r.name,
      })),
  },
  policy: {
    prefix: "policy",
    serverSearch: true,
    search: async (term, limit) =>
      (await listPolicies({ search: term, limit })).items.map((p) => ({
        id: p.id,
        name: p.name,
      })),
  },
};

/**
 * Resolve a catalog resource type to its entity source. Catalog types use the
 * underscore form already, but accept the hyphenated variant defensively.
 */
export function getEntitySource(resourceType: string): EntitySource | undefined {
  return (
    ENTITY_SOURCES[resourceType] ??
    ENTITY_SOURCES[resourceType.replace(/-/g, "_")]
  );
}

/** Wire pattern for a single selected entity, e.g. `connection_target:<id>`. */
export function patternForEntity(prefix: string, id: string): string {
  return `${prefix}:${id}`;
}

/** Extract the entity id from a `<prefix>:<id>` pattern, or null if it doesn't match. */
export function entityIdFromPattern(prefix: string, pattern: string): string | null {
  const head = `${prefix}:`;
  return pattern.startsWith(head) ? pattern.slice(head.length) : null;
}

/** True when the pattern set means "all resources" (empty or a single `*`). */
export function isAllPattern(patterns: string[]): boolean {
  return patterns.length === 0 || (patterns.length === 1 && patterns[0] === "*");
}
