import { useQuery } from "@tanstack/react-query";

import { listPlaybookRunGroups, type PlaybookRunGroup } from "@/features/playbooks/api";
import { statusPhase } from "@/features/playbooks/runPhase";

/** Comma-separated non-terminal statuses; the list endpoint matches any (`IN`). */
export const PLAYBOOK_ACTIVE_GROUP_STATUSES = "queued,leased,dispatching,running";
export const PLAYBOOK_ACTIVE_GROUPS_POLL_MS = 5_000;
export const PLAYBOOK_ACTIVE_GROUPS_LIMIT = 50;

export interface UseActivePlaybookGroupsResult {
  groups: PlaybookRunGroup[];
  isLoading: boolean;
  /** True when more active groups exist server-side than this page returned
   *  (`total > items.length`) — the dock's header count is a lower bound, not
   *  the true total, whenever this is set. */
  isTruncated: boolean;
}

/**
 * Polls for every currently-active playbook run group across the workspace
 * (the bottom dock), not just the ones the caller launched. Polling
 * only — no WebSocket; the live event stream stays scoped to whichever tab is
 * actually open (`usePlaybookRunStream`).
 */
export function useActivePlaybookGroups(enabled = true): UseActivePlaybookGroupsResult {
  const query = useQuery({
    queryKey: ["playbooks", "run-groups", "active-dock"],
    queryFn: () =>
      listPlaybookRunGroups({
        status: PLAYBOOK_ACTIVE_GROUP_STATUSES,
        limit: PLAYBOOK_ACTIVE_GROUPS_LIMIT,
      }),
    enabled,
    refetchInterval: PLAYBOOK_ACTIVE_GROUPS_POLL_MS,
    refetchIntervalInBackground: false,
  });

  const items = query.data?.items ?? [];
  // Defence in depth: the server-side status filter is the primary guard, but
  // re-filter client-side so a stale/misbehaving response never leaks a
  // terminal group into the dock.
  const groups = items.filter((group) => statusPhase(group.status) !== "finished");
  const isTruncated = (query.data?.total ?? 0) > items.length;

  return { groups, isLoading: query.isPending, isTruncated };
}
