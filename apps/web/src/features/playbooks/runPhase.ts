import type { PlaybookRunEvent, PlaybookRunStatus } from "@/features/playbooks/api";

/**
 * Coarse lifecycle phase derived from the authoritative run `status`.
 *
 * The run row now refreshes through `GET /playbooks/runs/{run_id}`, so the live
 * lifecycle is read straight from the run's own `status` instead of inferred
 * from the event stream. This collapses the eight backend statuses into the
 * three visual phases the detail view renders (badge + glyph + poll gating).
 */
export type RunPhase = "pending" | "running" | "finished";

const TERMINAL_STATUSES: ReadonlySet<PlaybookRunStatus> = new Set([
  "succeeded",
  "failed",
  "canceled",
  "timed_out",
]);

const ACTIVE_STATUSES: ReadonlySet<PlaybookRunStatus> = new Set([
  "leased",
  "dispatching",
  "running",
]);

export function statusPhase(status: PlaybookRunStatus): RunPhase {
  if (TERMINAL_STATUSES.has(status)) return "finished";
  if (ACTIVE_STATUSES.has(status)) return "running";
  return "pending";
}

export function isTerminalStatus(status: PlaybookRunStatus): boolean {
  return TERMINAL_STATUSES.has(status);
}

/**
 * Single user-facing status vocabulary. The backend lifecycle statuses
 * are preserved upstream (in queries, filters, the API); this is the display
 * projection. The internal in-flight states (`leased`, `dispatching`) fold into
 * one `running` bucket so jargon never leaks into
 * badges or labels.
 */
export type RunStatusBucket =
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "canceled"
  | "timed_out";

export function statusBucket(status: PlaybookRunStatus): RunStatusBucket {
  switch (status) {
    case "queued":
      return "queued";
    case "leased":
    case "dispatching":
    case "running":
      return "running";
    case "succeeded":
      return "succeeded";
    case "failed":
      return "failed";
    case "canceled":
      return "canceled";
    case "timed_out":
      return "timed_out";
  }
}

/** i18n key for a status' user-facing label (`playbooks.runStatus.<bucket>`). */
export function runStatusLabelKey(status: PlaybookRunStatus): string {
  return `playbooks.runStatus.${statusBucket(status)}`;
}

/** Badge variant for a status, derived from its display bucket. */
export function runStatusVariant(status: PlaybookRunStatus) {
  switch (statusBucket(status)) {
    case "succeeded":
      return "status-active" as const;
    case "failed":
    case "timed_out":
      return "status-failed" as const;
    case "canceled":
      return "status-staging" as const;
    case "running":
      return "status-staging" as const;
    case "queued":
      return "secondary" as const;
  }
}

/**
 * Statuses offered in the run-history filter. One canonical bucket per row, each
 * binding 1:1 to a single API status. The `leased`/`dispatching` lifecycle
 * states fold into `running` for display and are not
 * separately filterable — a deliberate simplification (the list endpoint takes a
 * single exact status, so a client-side "running" union would silently drop
 * non-terminal runs past the page limit).
 */
export const RUN_FILTER_STATUSES: PlaybookRunStatus[] = [
  "queued",
  "running",
  "succeeded",
  "failed",
  "canceled",
  "timed_out",
];

/**
 * True when cancel has been requested on a run that hasn't yet reached a
 * terminal status. This is a client-side synthetic state — the backend doesn't
 * expose it as a status bucket; it's inferred from `cancel_requested_at` being
 * non-null while `status` is still non-terminal.
 */
export function isStopping(
  status: PlaybookRunStatus,
  cancelRequestedAt: string | null | undefined,
): boolean {
  return cancelRequestedAt != null && !isTerminalStatus(status);
}

/** Latest `status` event message, if any — the most human-readable live label. */
export function latestStatusMessage(events: PlaybookRunEvent[]): string | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const event = events[i];
    if (event && event.event_kind === "status" && event.message) return event.message;
  }
  return null;
}
