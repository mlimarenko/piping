import type { TFunction } from "i18next";

import { ApiError } from "@/shared/api/client";
import type {
  PlaybookRun,
  PlaybookRunGroupDetail,
  PlaybookRunStatus,
} from "@/features/playbooks/api";
import type { RunDetailTarget } from "@/features/playbooks/RunDetailPanel";
import { isTerminalStatus, statusBucket } from "@/features/playbooks/runPhase";

/**
 * A playbook run group the Connections workspace renders as its own tab.
 * Kept entirely separate from `LiveSession` — this file never touches
 * `sessions[]` or the session resume/detach plumbing.
 */
export interface PlaybookTab {
  /** Stable tab identity. Equals `groupId` whenever a group id is known. */
  id: string;
  /** Empty string for the (practically unreachable) no-group edge case; see `buildPlaybookTab`. */
  groupId: string;
  playbookName: string;
  /** "Касса 603-1" for a single host, or an "N hosts" summary for bulk/group launches. */
  hostSummary: string;
  /**
   * Raw backend run-group status, used to color the tab's status dot (4 tones:
   * active/succeeded/failed/canceled). The coarser 3-value `RunPhase` collapses
   * succeeded/failed/canceled into one "finished" bucket, which can't drive the
   * 4-tone dot the UX spec calls for — so this field intentionally carries the
   * raw `PlaybookRunStatus` instead. Set once at open time (creation or
   * dock-driven restore) and not live-refreshed while the tab stays open; that
   * simplification is intentional.
   */
  phase: PlaybookRunStatus;
  target: RunDetailTarget;
  /** ISO timestamp. */
  startedAt: string;
}

export interface PersistedPlaybookTab {
  id: string;
  groupId: string;
  playbookName: string;
  hostSummary: string;
  startedAt: string;
}

export interface PersistedPlaybookTabs {
  version: 1;
  tabs: PersistedPlaybookTab[];
}

export const PLAYBOOK_TABS_SETTING_KEY = "connections.workspace.playbook_tabs";

export const EMPTY_PERSISTED_PLAYBOOK_TABS: PersistedPlaybookTabs = {
  version: 1,
  tabs: [],
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

/** Parses a persisted setting value; any shape mismatch drops to the empty state instead of throwing. */
export function normalizePersistedPlaybookTabs(value: unknown): PersistedPlaybookTabs {
  if (!isRecord(value) || value.version !== 1 || !Array.isArray(value.tabs)) {
    return EMPTY_PERSISTED_PLAYBOOK_TABS;
  }
  const tabs = value.tabs.flatMap((item): PersistedPlaybookTab[] => {
    if (!isRecord(item)) return [];
    const { id, groupId, playbookName, hostSummary, startedAt } = item;
    if (
      !isNonEmptyString(id) ||
      !isNonEmptyString(groupId) ||
      !isNonEmptyString(playbookName) ||
      !isNonEmptyString(hostSummary) ||
      !isNonEmptyString(startedAt)
    ) {
      return [];
    }
    return [{ id, groupId, playbookName, hostSummary, startedAt }];
  });
  return { version: 1, tabs };
}

/** Projects live tabs down to the persisted shape, dropping any tab with no restorable group id. */
export function toPersistedPlaybookTabs(tabs: PlaybookTab[]): PersistedPlaybookTabs {
  return {
    version: 1,
    tabs: tabs
      .filter((tab) => tab.groupId.length > 0)
      .map(({ id, groupId, playbookName, hostSummary, startedAt }) => ({
        id,
        groupId,
        playbookName,
        hostSummary,
        startedAt,
      })),
  };
}

/** "Касса 603-1" for a single run, "N hosts" once resolved run targets are known (tab creation from a seed payload). */
export function playbookHostSummaryFromRuns(runs: PlaybookRun[], t: TFunction): string {
  if (runs.length === 1) {
    const run = runs[0];
    return run?.target_name || run?.target_address || t("playbookTab.singleHostFallback");
  }
  return t("playbookTab.hostsCount", { count: runs.length });
}

/** Same "N hosts" summary, derived from a run-group's `total_runs` count alone (dock rows / group-only tabs). */
export function playbookHostSummaryFromCount(totalRuns: number, t: TFunction): string {
  return t("playbookTab.hostsCount", { count: totalRuns });
}

function makeSeedPlaybookTabId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `seed:${crypto.randomUUID()}`;
  }
  return `seed:${Date.now().toString(16)}-${Math.random().toString(16).slice(2, 10)}`;
}

/** Builds a tab from a freshly launched run (seed target) or a history run group (group target). */
export function buildPlaybookTab(
  target: RunDetailTarget,
  playbookName: string,
  t: TFunction,
): PlaybookTab {
  const groupId = target.groupId ?? null;
  const runs = target.kind === "seed" ? target.runs : [];
  return {
    id: groupId ?? makeSeedPlaybookTabId(),
    groupId: groupId ?? "",
    playbookName,
    hostSummary: playbookHostSummaryFromRuns(runs, t),
    // A freshly created run group always starts `queued`; this is refined the
    // next time the dock or a restore observes the group's live status.
    phase: "queued",
    target,
    startedAt: new Date().toISOString(),
  };
}

export type PlaybookRunGroupFetcher = (groupId: string) => Promise<PlaybookRunGroupDetail>;

/**
 * Restores persisted tabs on boot: fetches each group's current status in
 * parallel and keeps only the ones that are still non-terminal. A fetch error
 * (including a 404 for a group that no longer exists) drops that tab silently.
 */
export async function restorePersistedPlaybookTabs(
  persisted: PersistedPlaybookTabs,
  fetchGroup: PlaybookRunGroupFetcher,
): Promise<PlaybookTab[]> {
  const restored = await Promise.all(
    persisted.tabs.map(async (entry): Promise<PlaybookTab | null> => {
      try {
        const detail = await fetchGroup(entry.groupId);
        if (isTerminalStatus(detail.run_group.status)) return null;
        return {
          id: entry.id,
          groupId: entry.groupId,
          playbookName: entry.playbookName,
          hostSummary: entry.hostSummary,
          phase: detail.run_group.status,
          target: { kind: "group", groupId: entry.groupId },
          startedAt: entry.startedAt,
        };
      } catch (error) {
        // Only drop the tab when the group is definitely gone (404). A
        // transient failure (network blip, 5xx, auth-refresh race) at boot
        // must not permanently erase a still-active run's tab — the persist
        // effect writes the restore result straight back to the server
        // setting, so losing it here is not a display glitch, it is data
        // loss. Keep the tab with a best-effort "running" phase; its own live
        // query (RunDetailPanel / the dock) corrects the status dot as soon
        // as it fetches successfully.
        if (error instanceof ApiError && error.status === 404) return null;
        return {
          id: entry.id,
          groupId: entry.groupId,
          playbookName: entry.playbookName,
          hostSummary: entry.hostSummary,
          phase: "running",
          target: { kind: "group", groupId: entry.groupId },
          startedAt: entry.startedAt,
        };
      }
    }),
  );
  return restored.filter((tab): tab is PlaybookTab => tab !== null);
}

export interface MixedTabRef {
  kind: "session" | "playbook";
  id: string;
}

/** Alt+1..9 ordering: session tabs first, then playbook tabs. */
export function buildMixedTabOrder(sessionIds: string[], playbookTabIds: string[]): MixedTabRef[] {
  return [
    ...sessionIds.map((id): MixedTabRef => ({ kind: "session", id })),
    ...playbookTabIds.map((id): MixedTabRef => ({ kind: "playbook", id })),
  ];
}

/** Tailwind background-color class for the tab/dock status dot, from the run group's raw status. */
export function playbookStatusDotClassName(status: PlaybookRunStatus): string {
  switch (statusBucket(status)) {
    case "succeeded":
      return "bg-muted-foreground/50";
    case "failed":
    case "timed_out":
      return "bg-status-negative";
    case "canceled":
      return "bg-status-warning";
    case "queued":
    case "running":
      return "bg-status-positive";
  }
}
