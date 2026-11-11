import { useQueries, type QueryClient, type UseQueryOptions } from "@tanstack/react-query";
import { getConnectionLaunchOptions } from "@/features/connections/api";
import type { ConnectionLaunchOptions } from "@/shared/api/types";

/** Prefix shared by every target's launch-options entry. Mutations invalidate
 *  this prefix; use {@link connectionLaunchOptionsQueryKey} for one target. */
export const CONNECTION_LAUNCH_OPTIONS_QUERY_KEY = ["connections", "launch-options"] as const;

const CONNECTION_LAUNCH_OPTIONS_STALE_TIME = 60_000;

export function connectionLaunchOptionsQueryKey(hostId: string) {
  return [...CONNECTION_LAUNCH_OPTIONS_QUERY_KEY, hostId] as const;
}

/** Canonical query definition shared by the inspector, quick actions and
 *  intent prefetch so they read and refresh the same cache entry. */
export function connectionLaunchOptionsQueryOptions(
  hostId: string,
): UseQueryOptions<ConnectionLaunchOptions> {
  return {
    queryKey: connectionLaunchOptionsQueryKey(hostId),
    queryFn: ({ signal }) => getConnectionLaunchOptions(hostId, signal),
    staleTime: CONNECTION_LAUNCH_OPTIONS_STALE_TIME,
  };
}

/** Warm launch options for one host the user is targeting (selection/hover
 *  intent). Bound to an explicit target, so it never scans the inventory or
 *  fetches credentials. */
export function prefetchConnectionLaunchOptions(
  queryClient: QueryClient,
  hostId: string,
): Promise<void> {
  if (!hostId) return Promise.resolve();
  return queryClient.prefetchQuery(connectionLaunchOptionsQueryOptions(hostId));
}

/** Fetches connection launch options (protocols + accounts) for a target. */
export function useConnectionLaunchOptions(
  hostId: string,
  options: { enabled?: boolean } = {},
) {
  const active = !!hostId && (options.enabled ?? true);
  const queries: UseQueryOptions<ConnectionLaunchOptions>[] = active
    ? [connectionLaunchOptionsQueryOptions(hostId)]
    : [];
  const [query] = useQueries({ queries });
  return query ?? {
    data: undefined as ConnectionLaunchOptions | undefined,
    dataUpdatedAt: 0,
    isPending: true,
    isError: false,
    isFetching: false,
    refetch: () => Promise.resolve(undefined),
  };
}
