import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, ExternalLink, ListChecks } from "lucide-react";

import { cn } from "@/shared/lib/utils";
import { useIsMobile } from "@/shared/hooks/use-mobile";
import { useSetting } from "@/shared/settings";
import { listPlaybooks, type PlaybookRunGroup } from "@/features/playbooks/api";
import {
  playbookHostSummaryFromCount,
  playbookStatusDotClassName,
} from "@/features/playbooks/playbookTabs";
import { useActivePlaybookGroups } from "@/features/playbooks/useActivePlaybookGroups";

export const PLAYBOOK_DOCK_SETTING_KEY = "connections.workspace.playbooks_dock";

interface PersistedPlaybookDockState {
  expanded: boolean;
}

const DEFAULT_DOCK_STATE: PersistedPlaybookDockState = { expanded: false };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function normalizePlaybookDockState(value: unknown): PersistedPlaybookDockState {
  if (isRecord(value) && typeof value.expanded === "boolean") {
    return { expanded: value.expanded };
  }
  return DEFAULT_DOCK_STATE;
}

function formatDockElapsed(startedAt: string | null | undefined): string | null {
  if (!startedAt) return null;
  const originMs = new Date(startedAt).getTime();
  if (Number.isNaN(originMs)) return null;
  const seconds = Math.max(0, Math.round((Date.now() - originMs) / 1000));
  const minutes = Math.floor(seconds / 60);
  const remainderSeconds = seconds % 60;
  return minutes > 0 ? `${minutes}m ${remainderSeconds}s` : `${remainderSeconds}s`;
}

function doneCount(group: PlaybookRunGroup): number {
  return group.succeeded_runs + group.failed_runs + group.canceled_runs + group.timeout_runs;
}

interface PlaybookRunsDockProps {
  /** Group ids already open as a tab — hidden from the dock to avoid doubling. */
  openTabGroupIds: Set<string>;
  /** Focus (or open) the tab for this group. Receives the resolved playbook name
   *  so the caller does not need its own playbook-catalog query. */
  onFocusRun: (group: PlaybookRunGroup, playbookName: string) => void;
}

/**
 * Thin sticky bottom dock listing every currently-active playbook run across
 * the workspace — not just the ones the caller launched. Polling
 * only (`useActivePlaybookGroups`, ~5s), no WebSocket.
 */
function PlaybookRunsDock({ openTabGroupIds, onFocusRun }: PlaybookRunsDockProps) {
  const { t } = useTranslation();
  const isMobile = useIsMobile();
  const { groups, isTruncated } = useActivePlaybookGroups();
  const visibleGroups = useMemo(
    () => groups.filter((group) => !openTabGroupIds.has(group.id)),
    [groups, openTabGroupIds],
  );

  const { value: dockState, setValue: setDockState } = useSetting<PersistedPlaybookDockState>(
    PLAYBOOK_DOCK_SETTING_KEY,
    DEFAULT_DOCK_STATE,
    { normalize: normalizePlaybookDockState },
  );

  const playbooksQuery = useQuery({
    queryKey: ["playbooks", "catalog", "dock"],
    // The dock only needs id → name for whatever is currently active, but the
    // catalog list has no per-id lookup endpoint, so pull a generous page
    // rather than the app's usual 100-row default; beyond this, rows fall
    // back to the generic "Playbook" label instead of silently mismatching.
    queryFn: () => listPlaybooks({ limit: 500, offset: 0 }),
    enabled: visibleGroups.length > 0,
    staleTime: 60_000,
  });
  const playbookNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const item of playbooksQuery.data?.items ?? []) {
      map.set(item.playbook.id, item.playbook.name);
    }
    return map;
  }, [playbooksQuery.data]);

  if (visibleGroups.length === 0) return null;

  const resolvedName = (group: PlaybookRunGroup) =>
    playbookNameById.get(group.playbook_id) ?? t("playbooks.runDetail.unknownPlaybook");

  // The header count is a lower bound (not the true total) once the active-groups
  // page is truncated server-side — say so instead of quietly under-reporting.
  const activeCountLabel = isTruncated
    ? t("playbookDock.activeCountAtLeast", { count: visibleGroups.length })
    : t("playbookDock.activeCount", { count: visibleGroups.length });

  if (!dockState.expanded) {
    return (
      <button
        type="button"
        onClick={() => setDockState({ expanded: true })}
        className={cn(
          "fixed z-20 flex min-h-9 items-center gap-2 border border-border bg-card text-sm font-medium text-foreground transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          isMobile
            ? "inset-x-0 bottom-0 justify-center rounded-none py-2"
            : "bottom-3 right-3 rounded-md px-3 py-2",
        )}
      >
        <ListChecks className="size-4 text-muted-foreground" />
        {activeCountLabel}
      </button>
    );
  }

  return (
    <div
      className={cn(
        "fixed z-20 flex flex-col overflow-hidden border border-border bg-card",
        isMobile
          ? "inset-x-0 bottom-0 max-h-[40vh] rounded-t-lg"
          : "bottom-3 right-3 w-[min(28rem,92vw)] max-h-[30vh] rounded-lg",
      )}
    >
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2">
        <ListChecks className="size-4 text-muted-foreground" />
        <span className="flex-1 text-sm font-medium text-foreground">{activeCountLabel}</span>
        <button
          type="button"
          onClick={() => setDockState({ expanded: false })}
          aria-label={t("playbookDock.collapse")}
          title={t("playbookDock.collapse")}
          className="flex size-9 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ChevronDown className="h-4 w-4" />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {visibleGroups.map((group) => {
          const elapsed = formatDockElapsed(group.started_at);
          return (
            <button
              key={group.id}
              type="button"
              onClick={() => onFocusRun(group, resolvedName(group))}
              className="flex min-h-9 w-full items-center gap-2 border-b border-border px-3 py-2 text-left text-sm transition-colors last:border-b-0 hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <span
                aria-hidden
                className={cn("h-2 w-2 shrink-0 rounded-full", playbookStatusDotClassName(group.status))}
              />
              <span className="min-w-0 flex-1 truncate font-medium text-foreground">
                {resolvedName(group)}
              </span>
              <span className="hidden shrink-0 text-xs text-muted-foreground sm:inline">
                {playbookHostSummaryFromCount(group.total_runs, t)}
              </span>
              <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                {doneCount(group)}/{group.total_runs}
              </span>
              {elapsed ? (
                <span className="hidden shrink-0 text-xs tabular-nums text-muted-foreground sm:inline">
                  {elapsed}
                </span>
              ) : null}
              <ExternalLink className="h-3 w-3 shrink-0 text-muted-foreground" aria-hidden />
            </button>
          );
        })}
      </div>
    </div>
  );
}

export default PlaybookRunsDock;
