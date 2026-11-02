import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import PageState from "@/shared/ui-shared/PageState";

import { cn } from "@/shared/lib/utils";
import {
  cancelPlaybookRun,
  cancelPlaybookRunGroup,
  getPlaybookRunGroup,
  type PlaybookRun,
} from "@/features/playbooks/api";
import {
  isTerminalStatus,
  isStopping,
  statusPhase,
  type RunPhase,
} from "@/features/playbooks/runPhase";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { type RunGroupOutputAggregateDisplay } from "./components/run-detail/RunOutputView";
import { RunEventsPanel } from "./components/run-detail/RunEventsPanel";
import { RunGroupEventsPanel } from "./components/run-detail/RunGroupEventsPanel";
import { RunRow } from "./components/run-detail/RunRow";
import { RunStatusBar } from "./components/run-detail/RunStatusBar";
import { RunTargetChip } from "./components/run-detail/RunTargetChip";

/**
 * What the sheet is bound to:
 *   - `seed`: runs already in hand (post-launch `create_run_group` payload), so
 *     the list renders immediately while the group then refreshes via its own GET.
 *   - `group`: a run group from history — the group + its runs are fetched
 *     through `GET /playbooks/run-groups/{id}`.
 */
export type RunDetailTarget =
  | { kind: "seed"; runs: PlaybookRun[]; groupId?: string | null }
  | { kind: "group"; groupId: string };

export interface RunDetailPanelProps {
  target: RunDetailTarget | null;
  playbookName?: string | null;
  visible?: boolean;
  compact?: boolean;
  className?: string;
  onClose?: () => void;
}

/** How many targets to show before collapsing behind a "show all" toggle. */
const TARGET_VISIBLE_CAP = 24;
const RUN_GROUP_DETAIL_LIMIT = 50_000;
/** Stable empty seed list so the runs memo below has a referentially stable dep. */
const EMPTY_RUNS: PlaybookRun[] = [];
/** Page size for the run-group event fetch, owned by the accumulator that spends
 *  it; re-exported here so the panel's existing consumers keep one source. */
export { RUN_GROUP_EVENTS_PAGE } from "./components/run-detail/useAccumulatedRunGroupEvents";

// ---------------------------------------------------------------------------
// Elapsed timer — counts from a reference ISO timestamp, stops when not active
// ---------------------------------------------------------------------------

function useElapsedFrom(startIso: string | null | undefined, active: boolean) {
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!active || !startIso) {
      return;
    }
    const origin = new Date(startIso).getTime();
    // Clamp at 0: server timestamps can be a few seconds ahead of the client
    // clock — a negative elapsed ("-7s") must never render.
    const tick = () => setElapsed(Math.max(0, Math.round((Date.now() - origin) / 1000)));
    tick();
    const id = setInterval(tick, 1000);
    return () => {
      clearInterval(id);
      // Reset elapsed when unmounting so it starts fresh next open.
      setElapsed(0);
    };
  }, [active, startIso]);

  return elapsed;
}

// ---------------------------------------------------------------------------
// RunDetailPanel — the shared run-result workspace panel
// ---------------------------------------------------------------------------

/**
 * Inline run-detail panel. It keeps the previous run behavior intact, but does
 * not own placement: Playbooks embeds it as a two-pane run workspace, while
 * Connections embeds it as a compact side rail or bottom dock.
 *
 * Header counters / phase / elapsed all derive from `groupQuery` in both seed
 * and group modes (the group id is always present post-launch), so they
 * automatically update to the terminal state without a manual refresh.
 */
export const RunDetailPanel = ({
  target,
  playbookName,
  visible = true,
  compact = false,
  className,
  onClose,
}: RunDetailPanelProps) => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { can } = usePermissions();
  const titleId = useId();
  const descriptionId = useId();
  const panelActive = visible && target !== null;
  // Mirrors the backend cancel grant ("you can stop what you can run" —
  // Execute×Playbook in `authorize_cancel_for_playbook`). The per-playbook
  // owner short-circuit is a per-object decision and stays server-side.
  const canCancel = can("EXECUTE", "PLAYBOOK");
  const canReadGroup = can("READ", "PLAYBOOK_RUN_GROUP");
  const canReadRun = can("READ", "PLAYBOOK_RUN");

  const groupId =
    target?.kind === "group" ? target.groupId : (target?.groupId ?? null);

  // Group query — single source of truth for aggregate counters, phase and
  // runs list in both seed and group modes.
  const groupQuery = useQuery({
    queryKey: ["playbooks", "run-group", groupId],
    queryFn: () => getPlaybookRunGroup(groupId ?? "", RUN_GROUP_DETAIL_LIMIT),
    enabled: panelActive && groupId !== null && canReadGroup,
    refetchInterval: (query) => {
      const status = query.state.data?.run_group.status;
      return status && isTerminalStatus(status) ? false : 3000;
    },
  });

  // Runs list: live group data once available; seed payload during initial load.
  const seedRuns: PlaybookRun[] = target?.kind === "seed" ? target.runs : EMPTY_RUNS;
  const liveRuns = groupQuery.data?.runs ?? null;
  const runs = useMemo<PlaybookRun[]>(
    () => (liveRuns !== null ? liveRuns : seedRuns),
    [liveRuns, seedRuns],
  );
  const loadingGroup = target?.kind === "group" && canReadGroup && groupQuery.isPending;
  const awaitingGroupData = loadingGroup && liveRuns === null && seedRuns.length === 0;

  // No output-scoping selection: the group console is always the full Ansible
  // stream (see RunGroupEventsPanel). Only the legacy single-run path (no group)
  // renders a per-run panel.
  const fallbackRun = groupId === null ? (runs[0] ?? null) : null;

  // Large fan-outs (hundreds of targets) must not let the target list eat the
  // whole panel. Cap the visible targets and reveal the rest on demand.
  const [showAllTargets, setShowAllTargets] = useState(false);
  const loadedRunCount = runs.length;

  // Per-run polling is only meaningful when there is no group driving the list.
  const pollPerRun = groupId === null && canReadRun;

  // Derive overall phase from the live group status when available.
  const groupStatus = groupQuery.data?.run_group.status ?? null;
  const overallPhase: RunPhase = ((): RunPhase => {
    if (groupStatus) return statusPhase(groupStatus);
    if (awaitingGroupData) return "pending";
    const phases = runs.map((r) => statusPhase(r.status));
    if (phases.some((p) => p === "running") || phases.some((p) => p === "pending")) {
      return phases.some((p) => p === "running") ? "running" : "pending";
    }
    return "finished";
  })();

  const countsFromRuns = useMemo(
    () => ({
      failed: runs.filter((r) => r.status === "failed" || r.status === "timed_out").length,
      canceled: runs.filter((r) => r.status === "canceled").length,
      done: runs.filter((r) => isTerminalStatus(r.status)).length,
      total: loadedRunCount,
    }),
    [loadedRunCount, runs],
  );

  // Aggregate fields can drift for historical groups created before the current
  // grouped-run storage model. When the fetched run list is below the detail
  // limit, it is complete and therefore the safer source for visible counters.
  const hasCompleteRunList = loadedRunCount > 0 && loadedRunCount < RUN_GROUP_DETAIL_LIMIT;

  // Aggregate counters from complete live runs first, then from live group fields.
  const liveGroup = groupQuery.data?.run_group;
  const failedCount = hasCompleteRunList
    ? countsFromRuns.failed
    : liveGroup
    ? liveGroup.failed_runs + liveGroup.timeout_runs
    : countsFromRuns.failed;
  const canceledCount = hasCompleteRunList
    ? countsFromRuns.canceled
    : liveGroup
    ? liveGroup.canceled_runs
    : countsFromRuns.canceled;
  const doneCount = hasCompleteRunList
    ? countsFromRuns.done
    : liveGroup
    ? liveGroup.succeeded_runs + liveGroup.failed_runs + liveGroup.canceled_runs + liveGroup.timeout_runs
    : countsFromRuns.done;
  const totalCount = hasCompleteRunList
    ? countsFromRuns.total
    : (liveGroup?.total_runs ?? countsFromRuns.total);
  const visibleRuns = showAllTargets ? runs : runs.slice(0, TARGET_VISIBLE_CAP);
  const hasMoreTargets = totalCount > TARGET_VISIBLE_CAP;
  const anyFailed = failedCount > 0;
  const anyCanceled = !anyFailed && (groupStatus === "canceled" || canceledCount > 0);
  const isRunning =
    !awaitingGroupData && (overallPhase === "running" || overallPhase === "pending");

  // Elapsed: count from the group's started_at timestamp; stop when terminal.
  const startedAt = liveGroup?.started_at ?? null;
  const elapsed = useElapsedFrom(startedAt, panelActive && isRunning);

  const stopMutation = useMutation({
    mutationFn: async () => {
      if (groupId) {
        await cancelPlaybookRunGroup(groupId);
        return;
      }
      const active = runs.filter((run) => !isTerminalStatus(run.status));
      await Promise.all(active.map((run) => cancelPlaybookRun(run.id)));
    },
    onSuccess: () => {
      if (groupId) {
        void queryClient.invalidateQueries({ queryKey: ["playbooks", "run-group", groupId] });
      }
      runs.forEach((run) => {
        void queryClient.invalidateQueries({ queryKey: ["playbooks", "run", run.id] });
      });
      // Sync the runs history table so it stops showing stale «В очереди».
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "run-groups"] });
    },
  });

  // Header enters "all-stopping" state when every active run has cancel_requested_at set.
  // Uses the live runs list from the group query (which carries cancel_requested_at).
  const activeRuns = runs.filter((r) => !isTerminalStatus(r.status));
  const allActiveStopping =
    activeRuns.length > 0 &&
    activeRuns.every((r) => isStopping(r.status, r.cancel_requested_at));
  const aggregateDisplay: RunGroupOutputAggregateDisplay = {
    doneCount,
    totalCount,
    failedCount,
    canceledCount,
    anyCanceled,
    allActiveStopping,
  };

  // Invalidate the runs-list query exactly once when the group reaches terminal
  // so the history table refreshes without a manual reload.
  const prevTerminalRef = useRef(false);
  useEffect(() => {
    const isTerminal = groupStatus !== null && isTerminalStatus(groupStatus);
    if (isTerminal && !prevTerminalRef.current) {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "run-groups"] });
    }
    prevTerminalRef.current = isTerminal;
  }, [groupStatus, queryClient]);

  const resolvedPlaybookName =
    playbookName ?? t("playbooks.runDetail.unknownPlaybook");

  let eventStreamContent: ReactNode;
  if ((groupId && !canReadGroup) || (!groupId && fallbackRun && !canReadRun)) {
    eventStreamContent = <PageState kind="error" title={t("accessDenied.title")} />;
  } else if (awaitingGroupData) {
    eventStreamContent = (
      <PageState kind="loading" title={t("loading")} />
    );
  } else if (groupId) {
    eventStreamContent = (
      <RunGroupEventsPanel
        key={groupId}
        groupId={groupId}
        groupStatus={groupStatus}
        aggregateDisplay={aggregateDisplay}
        panelActive={panelActive}
      />
    );
  } else if (fallbackRun) {
    eventStreamContent = (
      <RunEventsPanel
        key={fallbackRun.id}
        run={fallbackRun}
        panelActive={panelActive}
        pollPerRun={pollPerRun}
      />
    );
  } else {
    eventStreamContent = (
      <PageState kind="empty" title={t("playbooks.runDetail.selectRun")} />
    );
  }

  if (!visible) {
    return null;
  }

  return (
    <section
      aria-labelledby={titleId}
      aria-describedby={descriptionId}
      className={cn(
        "flex h-full min-h-0 flex-col overflow-hidden rounded-md border border-border bg-card",
        className,
      )}
    >
      <h2 id={titleId} className="sr-only">
        {t("playbooks.runDetail.title")}
      </h2>
      <p id={descriptionId} className="sr-only">
        {t("playbooks.runDetail.description", {
          name: resolvedPlaybookName,
          count: totalCount,
        })}
      </p>

      <RunStatusBar
        playbookName={resolvedPlaybookName}
        phase={overallPhase}
        anyFailed={anyFailed}
        anyCanceled={anyCanceled}
        failedCount={failedCount}
        doneCount={doneCount}
        totalCount={totalCount}
        elapsed={elapsed}
        isRunning={isRunning}
        onStop={isRunning && canCancel ? () => stopMutation.mutate() : undefined}
        stopping={stopMutation.isPending}
        allStopping={allActiveStopping}
        onClose={onClose}
        t={t}
      />

      {target ? (
        <div
          className={cn(
            "grid min-h-0 flex-1 grid-rows-[auto_minmax(0,1fr)] gap-0",
            !compact && "md:grid-cols-[minmax(0,11rem)_minmax(0,1fr)] md:grid-rows-1",
          )}
        >
            {/* Target selector — chips (compact) or rows (wide), capped so a
                large fan-out never pushes the log out of reach. */}
            <div className={cn("flex min-h-0 flex-col gap-0", !compact && "md:border-r md:border-border/30")}>
              <p className="px-3 py-2 text-xs font-medium text-muted-foreground">
                {t("playbooks.runDetail.targets")}
                {totalCount > 0 ? (
                  <span className="ml-1 tabular-nums text-muted-foreground/60">{totalCount}</span>
                ) : null}
              </p>
              {loadingGroup ? (
                <PageState kind="loading" title={t("loading")} />
              ) : runs.length === 0 ? (
                <PageState kind="empty" title={t("playbooks.runDetail.noRuns")} />
              ) : compact ? (
                <div className="min-h-0 shrink-0">
                  <div className="flex max-h-[22vh] flex-wrap gap-1.5 overflow-y-auto px-3 pb-1.5">
                    {visibleRuns.map((run) => (
                      <RunTargetChip
                        key={run.id}
                        run={run}
                        panelActive={panelActive}
                        pollPerRun={pollPerRun}
                      />
                    ))}
                  </div>
                  {hasMoreTargets ? (
                    <div className="px-3 pb-2 pt-1">
                      <button
                        type="button"
                        onClick={() => setShowAllTargets((v) => !v)}
                        className="min-h-9 rounded-md px-2 text-sm font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {showAllTargets
                          ? t("playbooks.runDetail.showFewerTargets")
                          : t("playbooks.runDetail.showAllTargets", { count: totalCount })}
                      </button>
                    </div>
                  ) : null}
                </div>
              ) : (
                <div className="min-h-0 flex-1 overflow-y-auto p-1.5 space-y-0.5">
                  {visibleRuns.map((run) => (
                    <RunRow
                      key={run.id}
                      run={run}
                      panelActive={panelActive}
                      pollPerRun={pollPerRun}
                      groupId={groupId}
                      groupedExecution={Boolean(groupId && totalCount > 1)}
                    />
                  ))}
                  {hasMoreTargets ? (
                    <button
                      type="button"
                      onClick={() => setShowAllTargets((v) => !v)}
                      className="min-h-9 w-full rounded-md px-2 py-2 text-left text-sm font-medium text-primary hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      {showAllTargets
                        ? t("playbooks.runDetail.showFewerTargets")
                        : t("playbooks.runDetail.showAllTargets", { count: totalCount })}
                    </button>
                  ) : null}
                </div>
              )}
            </div>

            {/* Event stream */}
            <div className="flex min-h-0 flex-1 flex-col p-3">
              {eventStreamContent}
            </div>
          </div>
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center p-6">
          <div className="max-w-sm rounded-md border border-border bg-background px-4 py-6 text-center text-sm text-muted-foreground">
            {t("playbooks.runDetail.selectRun")}
          </div>
        </div>
      )}
    </section>
  );
};
