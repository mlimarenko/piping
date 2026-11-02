/**
 * Inspector panel sizing is independent of the docked session panel. The
 * inspector is a side column on wide layouts; its width is user-resizable in a
 * fixed band and persisted across sessions.
 */
export const INSPECTOR_MIN_WIDTH = 280;
export const INSPECTOR_MAX_WIDTH = 480;
export const INSPECTOR_DEFAULT_WIDTH = 320;
/** Main area width below which list + inspector stack vertically instead of columns. */
export const INSPECTOR_STACK_BREAKPOINT = 720;

export function clampInspectorWidth(width: unknown): number {
  const numeric = typeof width === "number" && Number.isFinite(width) ? width : INSPECTOR_DEFAULT_WIDTH;
  return Math.round(
    Math.max(INSPECTOR_MIN_WIDTH, Math.min(INSPECTOR_MAX_WIDTH, numeric)),
  );
}
