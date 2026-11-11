/**
 * Per-target persisted settings for the CCTV live view: single-vs-grid mode,
 * stream-vs-stills source, grid size and the channel each cell shows.
 * Stored in localStorage — a lightweight operator preference, not server state.
 */

export type LiveViewSource = "stream" | "stills";
export type LiveViewMode = "single" | "grid";
/** Grid side length; a layout of 2 renders 2×2, 3 → 3×3, 4 → 4×4. */
export type GridLayout = 2 | 3 | 4;

export interface LiveViewSettings {
  mode: LiveViewMode;
  source: LiveViewSource;
  layout: GridLayout;
  /** 1-based channel per cell, or 0 for empty, row-major. */
  cells: number[];
}

const STORAGE_PREFIX = "cctv:live:";

export function defaultSettings(): LiveViewSettings {
  return { mode: "single", source: "stream", layout: 2, cells: [1, 2, 3, 4] };
}

/** The initial four channels, followed by empty cells. */
export function defaultCells(layout: GridLayout): number[] {
  return Array.from({ length: layout * layout }, (_unused, index) => index < 4 ? index + 1 : 0);
}

function isGridLayout(value: unknown): value is GridLayout {
  return value === 2 || value === 3 || value === 4;
}

/**
 * Re-shape a cell list for a new layout: keep what fits, extend with the
 * empty cells for the added positions.
 */
export function resizeCells(cells: readonly number[], layout: GridLayout): number[] {
  const size = layout * layout;
  const next = cells.slice(0, size);
  while (next.length < size) next.push(0);
  return next;
}

export function loadLiveViewSettings(targetId: string): LiveViewSettings {
  const fallback = defaultSettings();
  try {
    const raw = window.localStorage.getItem(`${STORAGE_PREFIX}${targetId}`);
    if (!raw) return fallback;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return fallback;
    const candidate = parsed as Partial<LiveViewSettings>;
    const layout = isGridLayout(candidate.layout) ? candidate.layout : fallback.layout;
    const cells = Array.isArray(candidate.cells)
      ? resizeCells(
          candidate.cells.map((value: unknown) =>
            typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 512 ? value : 0,
          ),
          layout,
        )
      : defaultCells(layout);
    return {
      mode: candidate.mode === "grid" ? "grid" : "single",
      source: candidate.source === "stills" ? "stills" : "stream",
      layout,
      cells,
    };
  } catch {
    return fallback;
  }
}

export function saveLiveViewSettings(targetId: string, settings: LiveViewSettings): void {
  try {
    window.localStorage.setItem(`${STORAGE_PREFIX}${targetId}`, JSON.stringify(settings));
  } catch {
    // Quota/denied storage is non-fatal: the view still works, just unsaved.
  }
}
