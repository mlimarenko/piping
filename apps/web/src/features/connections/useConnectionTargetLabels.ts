import { useQuery } from "@tanstack/react-query";
import { getConnectionTarget } from "./api";
import type { ConnectionTarget } from "@/shared/api/types";

/** Hydrate only selected identities; an unavailable label never removes a host. */
export function useConnectionTargetLabels(ids: string[], enabled = true) {
  const selectedIds = [...new Set(ids)].sort();
  return useQuery({
    queryKey: ["connections", "target-labels", selectedIds],
    enabled: enabled && selectedIds.length > 0,
    queryFn: async ({ signal }) => {
      const items: ConnectionTarget[] = [];
      let unavailable = 0;
      for (let offset = 0; offset < selectedIds.length; offset += 4) {
        signal.throwIfAborted();
        const batch = await Promise.allSettled(selectedIds.slice(offset, offset + 4).map(id => getConnectionTarget(id, signal)));
        for (const result of batch) {
          if (result.status === "fulfilled") items.push(result.value);
          else unavailable += 1;
        }
      }
      return { items, unavailable };
    },
  });
}
