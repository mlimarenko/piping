import { useMemo } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listAllResources } from "@/features/inventory/api";
import { CONNECTION_TARGETS_QUERY_KEY, listConnectionTargets } from "@/features/connections/api";
import { usePermissions } from "@/shared/auth/permissionsContext";
import {
  MONITORING_SCOPE_ENTITY,
  MONITORING_TARGET_SCOPES,
  type MonitoringScopeType,
} from "./scope";

interface ScopeOption {
  id: string;
  name: string;
}

export interface ScopeEntities {
  /** Options keyed by scope layer, for the target picker. */
  optionsByScope: Record<Exclude<MonitoringScopeType, "global">, ScopeOption[]>;
  /** Resolve a `scope_type` + `scope_id` to a display name. */
  resolveName: (type: string, id: string | null | undefined) => string;
  isPending: boolean;
}

interface UseScopeEntitiesOptions {
  enabled?: boolean;
  includeConnectionTargets?: boolean;
}

type DictionaryScope = Exclude<MonitoringScopeType, "global" | "connection_target">;

/**
 * Loads complete monitoring scope dictionaries (sites, tenants, roles) so list
 * rows can resolve `scope_id` → name without leaking raw UUIDs.
 *
 * Connection targets are intentionally not loaded by default: large installs can
 * have tens of thousands of targets, so those pickers use server-side search.
 * Shared by all assignment panels (exporter / query-source / metric-set).
 */
export function useScopeEntities(
  globalLabel: string,
  options: UseScopeEntitiesOptions = {},
): ScopeEntities {
  const { enabled = true, includeConnectionTargets = false } = options;
  const { t } = useTranslation();
  const { can } = usePermissions();
  const canReadTargets = can("READ", "CONNECTION_TARGET");
  const targets = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    enabled: enabled && includeConnectionTargets && canReadTargets,
    staleTime: 60_000,
    retry: false,
  });
  const queryScopes = useMemo(
    () =>
      MONITORING_TARGET_SCOPES.filter(
        (scope): scope is DictionaryScope => scope !== "connection_target",
      ),
    [],
  );

  const queries = useQueries({
    queries: queryScopes.map((scope) => ({
      queryKey: ["observability", "scope-entity", MONITORING_SCOPE_ENTITY[scope]],
      queryFn: () => listAllResources(MONITORING_SCOPE_ENTITY[scope]),
      staleTime: 60_000,
      enabled,
    })),
  });

  const optionsByScope = useMemo(() => {
    const map = {} as Record<Exclude<MonitoringScopeType, "global">, ScopeOption[]>;
    MONITORING_TARGET_SCOPES.forEach((scope) => {
      const index = scope === "connection_target" ? -1 : queryScopes.indexOf(scope);
      map[scope] =
        index >= 0
           ? (queries[index]?.isError ? [] : queries[index]?.data ?? []).map((r) => ({ id: r.id, name: r.name }))
          : [];
    });
    return map;
  }, [queries, queryScopes]);

  const names = useMemo(() => {
    const map = new Map<string, string>();
    for (const [scope, rows] of Object.entries(optionsByScope)) {
      for (const row of rows) map.set(`${scope}:${row.id}`, row.name);
    }
    // Never expose stale cached names after READ visibility has been removed.
    if (includeConnectionTargets && canReadTargets && !targets.isError) {
      for (const row of targets.data?.items ?? []) {
        map.set(`connection_target:${row.id}`, [row.name, row.targetKind, row.primaryIp].filter(Boolean).join(" · "));
      }
    }
    return map;
  }, [optionsByScope, includeConnectionTargets, canReadTargets, targets.data, targets.isError]);

  const resolveName = useMemo(
    () => (type: string, id: string | null | undefined): string => {
      if (type === "global") return globalLabel;
      if (!id) return "";
      const match = names.get(`${type}:${id}`);
      if (match) return match;
      return `${t("observability.workflow.nameUnavailable", { defaultValue: "Name unavailable" })} · ${id}`;
    },
    [globalLabel, names, t],
  );

  return {
    optionsByScope,
    resolveName,
    isPending: queries.some((q) => q.isPending),
  };
}
