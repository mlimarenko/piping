/**
 * Pure width/mode helpers for the Automation "Runs" tab master-detail layout
 * (`PlaybooksPage.tsx`'s `runs` TabsContent). Kept separate from the page so
 * the resize/clamp math has focused unit coverage without dragging in the
 * page's React Query wiring.
 *
 * Two modes, switched on the MEASURED CONTAINER width (ResizeObserver), not
 * the viewport — the left nav sidebar steals width from the tab content, so a
 * viewport-based breakpoint would trigger the wrong mode.
 */

// Persisted setting: `{ width: 0 }` means "unset / auto-compute the default".
export const RUNS_DETAIL_WIDTH_SETTING_KEY = "automation.runs.detail_width";

export interface RunsDetailWidthSetting {
  width: number;
}

export const RUNS_DETAIL_WIDTH_DEFAULT: RunsDetailWidthSetting = { width: 0 };

// Below this container width, the runs list and run detail panel drill-in
// (one at a time, with a back button) instead of sitting side by side.
export const RUNS_WIDE_MODE_MIN_WIDTH = 900;

// Hard resize bounds for the detail pane: the list always keeps at least
// this many px, and the detail pane never shrinks below this either.
const RUNS_DETAIL_MIN_WIDTH = 380;
const RUNS_LIST_RESERVE_WIDTH = 380;

// Below this resolved detail width, RunDetailPanel switches to its compact
// (stacked host-list-above-events) internal layout.
export const RUNS_DETAIL_COMPACT_MAX_WIDTH = 560;

export function isWideRunsLayout(containerWidth: number): boolean {
  return containerWidth >= RUNS_WIDE_MODE_MIN_WIDTH;
}

/** Largest the detail pane may be while keeping the list usable. */
export function maxRunDetailWidth(containerWidth: number): number {
  return Math.max(RUNS_DETAIL_MIN_WIDTH, containerWidth - RUNS_LIST_RESERVE_WIDTH);
}

/** Clamps a candidate detail width to the current resize bounds. */
export function clampRunDetailWidth(width: number, containerWidth: number): number {
  const max = maxRunDetailWidth(containerWidth);
  if (!Number.isFinite(width)) return Math.min(RUNS_DETAIL_COMPACT_MAX_WIDTH, max);
  return Math.min(max, Math.max(RUNS_DETAIL_MIN_WIDTH, Math.round(width)));
}

/**
 * Default detail width when no persisted value exists: half the container,
 * biased toward a roomy 2-column layout, clamped to the resize bounds.
 */
export function defaultRunDetailWidth(containerWidth: number): number {
  const max = maxRunDetailWidth(containerWidth);
  const preferred = Math.max(RUNS_DETAIL_COMPACT_MAX_WIDTH, Math.round(containerWidth * 0.5));
  return Math.min(max, preferred);
}

/**
 * Resolves the effective, clamped detail width. `storedWidth <= 0` (incl. the
 * `{ width: 0 }` persisted default) means "unset / auto".
 */
export function resolveRunDetailWidth(storedWidth: number, containerWidth: number): number {
  if (!Number.isFinite(storedWidth) || storedWidth <= 0) {
    return defaultRunDetailWidth(containerWidth);
  }
  return clampRunDetailWidth(storedWidth, containerWidth);
}
