/**
 * Pure coordinate parsing/validation shared by the inventory Map page and the
 * reusable {@link MapPicker}. Kept framework-free (no React, no map runtime) so
 * the unit tests can exercise it without mounting a map — mirrors the
 * `colorUtils.ts` / `ColorField.tsx` split already used in this folder.
 */

/** Inclusive WGS-84 bounds. Latitude spans poles; longitude spans the meridian. */
export const LAT_MIN = -90;
export const LAT_MAX = 90;
export const LNG_MIN = -180;
export const LNG_MAX = 180;

/**
 * Parses one raw coordinate string into a finite number, or `null` when it is
 * blank, non-numeric, or outside the supplied range. An empty/whitespace string
 * is `null` (NOT `0`) — `Number("")` is `0`, which would otherwise drop a marker
 * onto the equator/meridian.
 */
export function parseCoord(raw: string | null | undefined, min: number, max: number): number | null {
  if (raw == null) return null;
  const trimmed = raw.trim();
  if (trimmed === "") return null;
  const value = Number(trimmed);
  if (!Number.isFinite(value)) return null;
  if (value < min || value > max) return null;
  return value;
}

/** Latitude convenience over {@link parseCoord} (−90..90). */
export function parseLat(raw: string | null | undefined): number | null {
  return parseCoord(raw, LAT_MIN, LAT_MAX);
}

/** Longitude convenience over {@link parseCoord} (−180..180). */
export function parseLng(raw: string | null | undefined): number | null {
  return parseCoord(raw, LNG_MIN, LNG_MAX);
}

/** Canonical 6-dp string form written back into the form fields. */
export function formatCoord(value: number): string {
  return value.toFixed(6);
}
