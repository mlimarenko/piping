import { useMemo } from "react";
import { useQueries } from "@tanstack/react-query";
import {
  CONNECTION_TARGETS_QUERY_KEY,
  listConnectionTargets,
} from "@/features/connections/api";
import { listResources } from "@/features/inventory/api";
import type { PlaybookAssignmentScopeKind } from "@/features/playbooks/api";
import {
  PLAYBOOK_SCOPE_ENTITY,
  PLAYBOOK_TARGET_SCOPE_KINDS,
} from "./scopeCatalog";

interface ScopeOption {
  /** Always a stable id (UUID) — this is what is stored in `scope_id`. */
  id: string;
  name: string;
}

export interface PlaybookScopeEntities {
  /** Options keyed by scope kind, for the entity picker. */
  optionsByScope: Record<Exclude<PlaybookAssignmentScopeKind, "global">, ScopeOption[]>;
  /** Resolve a `scope_kind` + `scope_id` to a display name. */
  resolveName: (kind: string, id: string | null | undefined) => string;
  isPending: boolean;
}

/**
 * Eagerly loads every targetable playbook scope entity (connection targets,
 * device roles, sites, regions, tenants, clusters, platforms) so the assignment
 * dialog can both offer ids in a picker and resolve `scope_id` → name without
 * leaking raw UUIDs. Mirrors the observability `useScopeEntities` pattern: every
 * non-`global` scope binds the entity **id**, never its display name.
 */
export function usePlaybookScopeEntities(
  globalLabel: string,
  enabled = true,
): PlaybookScopeEntities {
  const queries = useQueries({
    queries: PLAYBOOK_TARGET_SCOPE_KINDS.map((scope) => ({
      queryKey:
        scope === "connection_target"
          ? CONNECTION_TARGETS_QUERY_KEY
          : ["playbooks", "scope-entity", PLAYBOOK_SCOPE_ENTITY[scope]],
      queryFn: () =>
        scope === "connection_target"
          ? listConnectionTargets()
          : listResources(PLAYBOOK_SCOPE_ENTITY[scope], { limit: 1000, offset: 0 }),
      enabled,
      staleTime: 60_000,
    })),
  });

  const optionsByScope = useMemo(() => {
    const map = {} as Record<Exclude<PlaybookAssignmentScopeKind, "global">, ScopeOption[]>;
    PLAYBOOK_TARGET_SCOPE_KINDS.forEach((scope, index) => {
      map[scope] = (queries[index]?.data?.items ?? []).map((r) => ({ id: r.id, name: r.name }));
    });
    return map;
  }, [queries]);

  const resolveName = useMemo(
    () => (kind: string, id: string | null | undefined): string => {
      if (kind === "global") return globalLabel;
      if (!id) return "";
      const scope = kind as Exclude<PlaybookAssignmentScopeKind, "global">;
      const opts = optionsByScope[scope] ?? [];
      const match = opts.find((o) => o.id === id)?.name;
      if (match) return match;
      const queryIndex = PLAYBOOK_TARGET_SCOPE_KINDS.indexOf(scope);
      if (queryIndex !== -1 && queries[queryIndex]?.isPending) return "…";
      return id;
    },
    [globalLabel, optionsByScope, queries],
  );

  return {
    optionsByScope,
    resolveName,
    isPending: queries.some((q) => q.isPending),
  };
}
