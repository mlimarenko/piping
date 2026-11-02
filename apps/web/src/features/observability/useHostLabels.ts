import { useMemo } from "react";
import { useQueries, type UseQueryResult } from "@tanstack/react-query";

import {
  getHostLabels,
  type HostLabelEntry,
  type HostLabelsResponse,
} from "@/features/observability/api";

// `host_id` is serialised as repeated `host_id=<uuid>` query params (~44 bytes
// each). A single GET for the whole visible set crosses the 8KB request-line
// buffer at ~180 ids and 414s, silently dropping labels for the entire list, so
// the ids are split into bounded batches that each stay well under the buffer.
const HOST_LABEL_BATCH_SIZE = 50;
const HOST_LABEL_CONCURRENCY = 2;

export interface UseHostLabelsResult {
  /** host_id → cached list-label entry, for O(1) per-row lookup. */
  byHost: Map<string, HostLabelEntry>;
  isLoading: boolean;
  /** True when any returned entry is a cache miss (stale). */
  stale: boolean;
}

function chunk<T>(items: T[], size: number): T[][] {
  const out: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    out.push(items.slice(index, index + size));
  }
  return out;
}

function createHostLabelRequestQueue() {
  let active = 0;
  const waiting: (() => void)[] = [];
  return (ids: string[], signal: AbortSignal): Promise<HostLabelsResponse> => new Promise((resolve, reject) => {
    const cancel = () => {
      const index = waiting.indexOf(start);
      if (index !== -1) waiting.splice(index, 1);
      reject(new DOMException("Cancelled", "AbortError"));
    };
    const start = () => {
      signal.removeEventListener("abort", cancel);
      active += 1;
      void Promise.resolve().then(() => {
        signal.throwIfAborted();
        return getHostLabels(ids, signal);
      }).then(resolve, reject).finally(() => {
        active -= 1;
        waiting.shift()?.();
      });
    };
    if (signal.aborted) {
      cancel();
    } else if (active < HOST_LABEL_CONCURRENCY) {
      start();
    } else {
      waiting.push(start);
      signal.addEventListener("abort", cancel, { once: true });
    }
  });
}

// Stable combine identity preserves map references until a batch's data changes.
function combineHostLabels(results: UseQueryResult<HostLabelsResponse>[]): UseHostLabelsResult {
  const byHost = new Map<string, HostLabelEntry>();
  for (const result of results) {
    for (const entry of result.data?.hosts ?? []) byHost.set(entry.host_id, entry);
  }
  return {
    byHost,
    isLoading: results.some((result) => result.isLoading),
    stale: Array.from(byHost.values()).some((entry) => entry.stale),
  };
}

/** Fetch only the rendered window, with bounded requests and cancellable old windows. */
export function useHostLabels(hostIds: string[]): UseHostLabelsResult {
  const batches = useMemo(() => chunk(Array.from(new Set(hostIds.filter(Boolean))), HOST_LABEL_BATCH_SIZE), [hostIds]);
  const request = useMemo(() => createHostLabelRequestQueue(), []);
  return useQueries({
    queries: batches.map((batch) => ({
      queryKey: ["monitoring", "host-labels", batch] as const,
      queryFn: ({ signal }: { signal: AbortSignal }) => request(batch, signal),
      refetchInterval: 30_000,
      refetchIntervalInBackground: false,
      staleTime: 15_000,
    })),
    combine: combineHostLabels,
  });
}
