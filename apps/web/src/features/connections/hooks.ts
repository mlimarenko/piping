import { useQuery } from "@tanstack/react-query";
import { getConnectionLaunchOptions } from "@/features/connections/api";
import type { ConnectionLaunchOptions } from "@/shared/api/types";

/** Fetches connection launch options (protocols + accounts) for a target. */
export function useConnectionLaunchOptions(
  hostId: string,
  options: { enabled?: boolean; staleTime?: number } = {},
) {
  return useQuery<ConnectionLaunchOptions>({
    queryKey: ["connections", "launch-options", hostId],
    queryFn: () => getConnectionLaunchOptions(hostId),
    staleTime: options.staleTime ?? 60_000,
    enabled: !!hostId && (options.enabled ?? true),
  });
}
