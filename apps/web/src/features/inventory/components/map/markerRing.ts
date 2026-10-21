/**
 * Pure geometry for the composite site marker's segmented ring. Each visible
 * entity type with a non-zero count at a site contributes one arc, sized by its
 * share of the total, rendered as a `stroke-dasharray`/`strokeDashoffset` overlay
 * `<circle>`. DOM-free so the arc math is unit-tested without mounting the map.
 *
 * Arcs start at the top of the circle (12 o'clock) and proceed clockwise, with a
 * fixed pixel gap between adjacent arcs so the segments read as distinct.
 */
import { LAYER_COLOR, LAYER_DISPLAY_ORDER } from "@/features/inventory/components/map/mapLayers";
import type { MapLayerId } from "@/features/inventory/components/map/mapLayers";

export interface RingArc {
  layerId: MapLayerId;
  color: string;
  /** `<segLen> <gapLen>` for `stroke-dasharray` (gapLen spans the rest of `C`). */
  dashArray: string;
  /** Offset that rotates the segment to its clockwise start angle. */
  dashOffset: number;
}

/** Pixel gap between adjacent ring segments. */
const GAP_PX = 2;

/**
 * Build the ring arcs for one site. Only `relatedCounts` entries whose layer is
 * in `visibleLayerIds`, is not the `sites` layer itself, and has count > 0 get an
 * arc. Iterating {@link LAYER_DISPLAY_ORDER} keeps the arc order deterministic.
 *
 * With a single contributing layer the arc spans the whole circle minus one gap.
 * An empty result (no visible related counts) means "render no ring".
 */
export function computeRingArcs(
  relatedCounts: Record<string, number>,
  visibleLayerIds: ReadonlySet<MapLayerId>,
  ringR: number,
): RingArc[] {
  const contributing = LAYER_DISPLAY_ORDER.filter(
    (id) => id !== "sites" && visibleLayerIds.has(id) && (relatedCounts[id] ?? 0) > 0,
  );
  if (contributing.length === 0) return [];

  const circumference = 2 * Math.PI * ringR;
  const total = contributing.reduce((sum, id) => sum + (relatedCounts[id] ?? 0), 0);
  const gap = contributing.length > 1 ? GAP_PX : 0;
  // Length available for the coloured segments after reserving one gap per arc.
  const drawable = Math.max(0, circumference - gap * contributing.length);

  const arcs: RingArc[] = [];
  // Start segments at 12 o'clock: SVG dash starts at 3 o'clock, so shift by a
  // quarter-circumference. Clockwise progression accumulates negative offset.
  let cursor = circumference / 4;
  for (const id of contributing) {
    const share = (relatedCounts[id] ?? 0) / total;
    const segLen = drawable * share;
    arcs.push({
      layerId: id,
      color: LAYER_COLOR[id],
      dashArray: `${segLen} ${circumference - segLen}`,
      dashOffset: cursor,
    });
    cursor -= segLen + gap;
  }
  return arcs;
}
