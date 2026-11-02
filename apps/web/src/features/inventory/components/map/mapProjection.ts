/**
 * Pure, framework-free projection math for the offline {@link CustomMap}. Kept
 * DOM-free (no React, no SVG) so the unit test can exercise `project`/`invert`
 * round-trips and the fit math without mounting a map — mirrors the
 * `coordUtils.ts` split already used for the picker.
 *
 * Model: Web Mercator → a normalized [0,1]×[0,1] "world square". The map view is
 * then `{ k, tx, ty }`: `k` is px per world-unit, `tx`/`ty` the px translate.
 * Screen position of a world point `(wx, wy)` is `sx = wx*k + tx`,
 * `sy = wy*k + ty`. Everything below is that one transform and its inverse.
 */

/** Web Mercator clamps latitude to ±85.05° (where the projection diverges). */
const LAT_CLAMP = 85.05112878;

/** Clamp a latitude into the Web Mercator valid band. */
function clampLat(lat: number): number {
  return Math.max(-LAT_CLAMP, Math.min(LAT_CLAMP, lat));
}

/** Project `(lng, lat)` to normalized world coords, each in `[0, 1]`. */
export function project(lng: number, lat: number): [number, number] {
  const x = (lng + 180) / 360;
  const s = Math.sin((clampLat(lat) * Math.PI) / 180);
  const y = 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
  return [x, y];
}

/** Inverse of {@link project}: normalized world coords → `[lng, lat]` degrees. */
export function invert(wx: number, wy: number): [number, number] {
  const lng = wx * 360 - 180;
  const lat = (Math.atan(Math.sinh((1 - 2 * wy) * Math.PI)) * 180) / Math.PI;
  return [lng, lat];
}

/** A single screen-space view transform. `k` = px per world-unit. */
export interface MapView {
  k: number;
  tx: number;
  ty: number;
}

/** Container pixel size measured from the host element. */
export interface MapSize {
  w: number;
  h: number;
}

/** A point to fit, already carrying geographic coordinates. */
export interface GeoPoint {
  lat: number;
  lng: number;
}

/**
 * Default `k` (px per world-unit) used when the point set has no spatial extent
 * (single point or all-coincident). At this scale one world-unit spans ~city
 * zoom, which keeps a lone marker comfortably framed instead of filling the
 * viewport with a single country.
 */
export const DEFAULT_FIT_K = 4096;

/** Fraction of the viewport the fitted bbox should occupy (margin on all sides). */
const FIT_PADDING = 0.75;

/** Multipliers turning the fitted `k` into the zoom clamp `[minK, maxK]`. */
export const MIN_K_FACTOR = 1 / 8;
export const MAX_K_FACTOR = 64;

export interface FitResult extends MapView {
  /** Lower zoom bound derived from the fit (`k * MIN_K_FACTOR`). */
  minK: number;
  /** Upper zoom bound derived from the fit (`k * MAX_K_FACTOR`). */
  maxK: number;
}

/**
 * Compute the `{ k, tx, ty }` that frames every point inside a `w × h`
 * container, plus the derived `minK`/`maxK` zoom clamp. Empty point sets fall
 * back to `defaultCenter` (default world-centre) at {@link DEFAULT_FIT_K}; a
 * zero-span bbox (one point) also uses {@link DEFAULT_FIT_K}. A zero-size
 * container yields a transform anyway (the caller guards `w > 0 && h > 0`
 * before applying it), so this stays total for the test.
 *
 * `defaultCenter` is `[lat, lng]` — matching the form/picker convention — so an
 * empty picker can open framed on a region (e.g. Moscow) instead of the
 * equatorial Atlantic.
 */
export function fitToPoints(
  points: readonly GeoPoint[],
  size: MapSize,
  defaultCenter?: [number, number],
): FitResult {
  const { w, h } = size;

  // No points, or a not-yet-measured container: frame the default centre
  // (world-centre when none is supplied) at the city-zoom default k.
  if (points.length === 0 || w <= 0 || h <= 0) {
    const k = DEFAULT_FIT_K;
    const [cwx, cwy] = defaultCenter
      ? project(defaultCenter[1], defaultCenter[0])
      : [0.5, 0.5];
    return {
      k,
      tx: w / 2 - cwx * k,
      ty: h / 2 - cwy * k,
      minK: k * MIN_K_FACTOR,
      maxK: k * MAX_K_FACTOR,
    };
  }

  let minWx = Infinity;
  let minWy = Infinity;
  let maxWx = -Infinity;
  let maxWy = -Infinity;

  for (const point of points) {
    const [wx, wy] = project(point.lng, point.lat);
    if (wx < minWx) minWx = wx;
    if (wy < minWy) minWy = wy;
    if (wx > maxWx) maxWx = wx;
    if (wy > maxWy) maxWy = wy;
  }

  const spanX = maxWx - minWx;
  const spanY = maxWy - minWy;

  // Zero (or near-zero) span → a single point / coincident set: use the default
  // city-zoom k rather than dividing by ~0 (which would explode k to Infinity).
  const k =
    spanX > 1e-9 || spanY > 1e-9
      ? FIT_PADDING * Math.min(spanX > 1e-9 ? w / spanX : Infinity, spanY > 1e-9 ? h / spanY : Infinity)
      : DEFAULT_FIT_K;

  const centerWx = (minWx + maxWx) / 2;
  const centerWy = (minWy + maxWy) / 2;

  return {
    k,
    tx: w / 2 - centerWx * k,
    ty: h / 2 - centerWy * k,
    minK: k * MIN_K_FACTOR,
    maxK: k * MAX_K_FACTOR,
  };
}

/** Clamp `value` into `[min, max]`. */
export function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

/**
 * New view after an anchored wheel zoom: scale `k` by `factor` (clamped to
 * `[minK, maxK]`) while keeping the world point under the cursor `(cx, cy)`
 * pixel-fixed. Pure so the test can assert the anchor invariant.
 */
export function zoomAt(
  view: MapView,
  factor: number,
  cx: number,
  cy: number,
  minK: number,
  maxK: number,
): MapView {
  const { k, tx, ty } = view;
  const wx = (cx - tx) / k;
  const wy = (cy - ty) / k;
  const nk = clamp(k * factor, minK, maxK);
  return { k: nk, tx: cx - wx * nk, ty: cy - wy * nk };
}

/** Screen pixel `(sx, sy)` → world coords, then to `[lng, lat]` degrees. */
export function screenToLngLat(sx: number, sy: number, view: MapView): [number, number] {
  const wx = (sx - view.tx) / view.k;
  const wy = (sy - view.ty) / view.k;
  return invert(wx, wy);
}
