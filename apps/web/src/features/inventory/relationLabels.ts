import { useMemo } from "react";
import { useQueries } from "@tanstack/react-query";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { getResource } from "./api";
import { inventoryResourceDisplayName } from "./resourceOptions";

/**
 * A reference from one inventory record to another. `id` is the FK UUID the
 * backend stores; `entity` is the tab that owns it.
 */
export interface InventoryRelationRef {
  entity: InventoryEntityKey;
  id: string;
}

const FIVE_MINUTES = 5 * 60 * 1000;

export const inventoryRelationKey = (entity: InventoryEntityKey, id: string): string =>
  `${entity}:${id}`;

/**
 * Resolve relation UUIDs to their inventory records. Detail endpoints return
 * only `*_id`, so read-only surfaces call this instead of printing an opaque
 * UUID.
 *
 * One cached GET per distinct relation keeps large related catalogues out of
 * the request (a full `listAllResources` for a device role would be wasteful).
 * A missing or forbidden relation resolves to no entry; callers keep their own
 * explicit unknown fallback and never invent a name.
 */
export function useInventoryRelations(
  refs: readonly InventoryRelationRef[],
  enabled = true,
): Map<string, Resource> {
  const unique = useMemo(() => {
    const seen = new Set<string>();
    const values: InventoryRelationRef[] = [];
    for (const ref of refs) {
      const key = inventoryRelationKey(ref.entity, ref.id);
      if (!ref.id || seen.has(key)) continue;
      seen.add(key);
      values.push(ref);
    }
    return values;
  }, [refs]);

  const results = useQueries({
    queries: unique.map((ref) => ({
      queryKey: ["inventory", "resource", ref.entity, ref.id],
      queryFn: () => getResource(ref.entity, ref.id),
      enabled: enabled && Boolean(ref.id),
      staleTime: FIVE_MINUTES,
      retry: false,
    })),
  });

  return useMemo(() => {
    const resources = new Map<string, Resource>();
    unique.forEach((ref, index) => {
      const resource = results[index]?.data;
      if (resource) resources.set(inventoryRelationKey(ref.entity, ref.id), resource);
    });
    return resources;
  }, [unique, results]);
}

export function useInventoryRelationLabels(
  refs: readonly InventoryRelationRef[],
  enabled = true,
): Map<string, string> {
  const resources = useInventoryRelations(refs, enabled);
  return useMemo(() => new Map(
    [...resources].map(([key, resource]) => [key, inventoryResourceDisplayName(resource)]),
  ), [resources]);
}
