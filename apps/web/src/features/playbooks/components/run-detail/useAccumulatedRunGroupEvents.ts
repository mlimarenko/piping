import { useRef } from "react";
import { useQuery } from "@tanstack/react-query";

import {
  listPlaybookRunGroupEvents,
  type PlaybookRunEvent,
} from "@/features/playbooks/api";

/** Page size for the run-group event fetch. Large so a finished run's whole
 *  backlog lands in ONE request instead of a serial `after_sequence` cursor walk
 *  (a 17k-event run was ~9 sequential round-trips at 2_000). The server caps a
 *  page at PLAYBOOK_RUN_MAX_TARGET_LIMIT (50_000), so this stays well within it.
 *  On the LIVE path the cursor already bounds each poll to only-new events, so a
 *  big limit only caps a burst — it never enlarges a steady-state poll. Exported
 *  so the test asserts against a single source of truth. */
export const RUN_GROUP_EVENTS_PAGE = 20_000;

// Incrementally accumulate a run group's events across polls so the console is
// the COMPLETE stream, never a truncated window. Each poll fetches only what is
// new (`after_sequence` cursor) and appends it; a full page triggers an
// immediate loop-drain so a burst larger than one page lands in one poll instead
// of one page per interval. Accumulating outside the query result (refs) is what
// makes it lossless — `data` is a growing snapshot, not the latest page.
export function useAccumulatedRunGroupEvents(
  groupId: string,
  panelActive: boolean,
  terminal: boolean,
): { events: PlaybookRunEvent[]; isPending: boolean; isError: boolean } {
  const accRef = useRef<PlaybookRunEvent[]>([]);
  const seenRef = useRef<Set<number>>(new Set());
  const cursorRef = useRef<number | null>(null);
  const activeGroupRef = useRef<string | null>(null);
  // After the group goes terminal, keep polling until two consecutive drains add
  // nothing (the final flush has landed), then stop.
  const settledPollsRef = useRef(0);

  const query = useQuery({
    // Cursor is intentionally NOT in the key: one cache entry per group that the
    // accumulator grows, not a new entry per page.
    queryKey: ["playbooks", "run-group-events", groupId],
    enabled: panelActive,
    refetchInterval: () => (!terminal || settledPollsRef.current < 2 ? 2500 : false),
    queryFn: async () => {
      // Refs persist across a groupId change (the query keys on it, the refs do
      // not) — reset the accumulation when the panel switches groups.
      if (activeGroupRef.current !== groupId) {
        activeGroupRef.current = groupId;
        accRef.current = [];
        seenRef.current = new Set();
        cursorRef.current = null;
        settledPollsRef.current = 0;
      }
      let addedThisCall = 0;
      // Bound the loop-drain (100 × 20k = 2M events/poll) so an ever-growing tail can never
      // wedge a single poll; the next interval picks up the remainder.
      for (let page = 0; page < 100; page += 1) {
        const response = await listPlaybookRunGroupEvents(groupId, {
          afterSequence: cursorRef.current ?? undefined,
          limit: RUN_GROUP_EVENTS_PAGE,
        });
        const items = response.items ?? [];
        for (const event of items) {
          if (seenRef.current.has(event.event_sequence)) continue;
          seenRef.current.add(event.event_sequence);
          accRef.current.push(event);
          addedThisCall += 1;
          if (cursorRef.current === null || event.event_sequence > cursorRef.current) {
            cursorRef.current = event.event_sequence;
          }
        }
        // Backend orders by event_sequence ASC; a short page means the backlog is
        // drained. A full page means keep going now.
        if (items.length < RUN_GROUP_EVENTS_PAGE) break;
      }
      if (terminal) {
        settledPollsRef.current = addedThisCall === 0 ? settledPollsRef.current + 1 : 0;
      }
      // New array reference each poll so consumers re-render as the log grows.
      return accRef.current.slice();
    },
  });

  return {
    events: query.data ?? [],
    isPending: query.isPending,
    isError: query.isError,
  };
}
