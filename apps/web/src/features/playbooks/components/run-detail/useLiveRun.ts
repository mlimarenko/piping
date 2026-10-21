import { useQuery } from "@tanstack/react-query";

import { getPlaybookRun, type PlaybookRun } from "@/features/playbooks/api";
import { isTerminalStatus } from "@/features/playbooks/runPhase";

// ---------------------------------------------------------------------------
// useLiveRun — per-run status poll, only active when no group drives the list
// ---------------------------------------------------------------------------

export function useLiveRun(seed: PlaybookRun, panelActive: boolean, enabled: boolean) {
  const seedTerminal = isTerminalStatus(seed.status);
  const liveQuery = useQuery({
    queryKey: ["playbooks", "run", seed.id],
    queryFn: () => getPlaybookRun(seed.id),
    enabled: panelActive && enabled && !seedTerminal,
    initialData: seedTerminal ? seed : undefined,
    refetchInterval: (query) =>
      query.state.data && isTerminalStatus(query.state.data.status) ? false : 2500,
  });
  return enabled ? (liveQuery.data ?? seed) : seed;
}
