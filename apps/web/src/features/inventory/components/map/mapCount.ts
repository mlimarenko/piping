/** Compact marker text stays bounded; the full count remains available separately. */
export function formatMapCount(count: number): string {
  if (!Number.isFinite(count) || count <= 0) return "";
  if (count < 1000) return String(Math.floor(count));
  if (count < 1_000_000) return `${Math.floor(count / 1000)}k`;
  if (count < 1_000_000_000) return `${Math.floor(count / 1_000_000)}M`;
  return "1B+";
}
/** Shared screen-space marker geometry for both renderers. */
export const MAP_COUNT_TEXT_SIZE = 12;
export const MAP_BADGE_RADIUS = 20;
export const MAP_MAX_MARKER_RADIUS = 22;
export const MAP_HIT_RADIUS = 22;
export const MAP_FOCUS_SCALE = 512 * 2 ** 8;

export function mapMarkerRadius(radius: number | undefined, count: number): number {
  const requested = Number.isFinite(radius) ? radius! : 6;
  return Math.min(MAP_MAX_MARKER_RADIUS, Math.max(count > 0 ? MAP_BADGE_RADIUS : 6, requested));
}
