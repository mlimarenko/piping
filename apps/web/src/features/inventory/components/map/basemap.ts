/**
 * Typed accessor for the bundled offline basemap. The GeoJSON
 * (`basemap.geo.json` — Natural Earth 50m worldwide countries + admin-1
 * provinces/oblasts) is imported directly so Vite inlines it into the bundle
 * (no tiles, no network — works in a closed network).
 *
 * Every feature carries `properties.layer`: `"country"` for the filled national
 * shapes and `"subnational"` for the stroke-only admin-1 (state/province/oblast)
 * borders drawn on top.
 *
 * The raw `.json` import has no useful inferred type under bundler resolution,
 * so we narrow it once here through a minimal GeoJSON-subset interface and
 * re-export the typed value. This is the single cast for the whole feature.
 */
import basemapRaw from "./basemap.geo.json";
import { project } from "./mapProjection";

/** A GeoJSON ring is a closed list of `[lng, lat]` positions. */
export type Ring = [number, number][];

/** A Polygon is an outer ring plus optional holes. */
export type PolygonCoords = Ring[];

/** A MultiPolygon is a list of polygons. */
export type MultiPolygonCoords = PolygonCoords[];

export interface PolygonGeometry {
  type: "Polygon";
  coordinates: PolygonCoords;
}

export interface MultiPolygonGeometry {
  type: "MultiPolygon";
  coordinates: MultiPolygonCoords;
}

/** Which render group a basemap feature belongs to: filled national shapes
 *  (`country`) or stroke-only admin-1 borders (`subnational`). */
export type BasemapLayerKind = "country" | "subnational";

export interface BasemapFeature {
  type: "Feature";
  properties: { name: string; layer: BasemapLayerKind };
  geometry: PolygonGeometry | MultiPolygonGeometry;
}

export interface BasemapCollection {
  type: "FeatureCollection";
  features: BasemapFeature[];
}

/** The bundled basemap, narrowed to the subset of GeoJSON we render. */
export const basemap = basemapRaw as BasemapCollection;

/** A single basemap feature reduced to its renderable SVG path data. */
export interface BasemapPathDatum {
  name: string;
  /** SVG `d` in **world** coords (`M wx wy L … Z` per ring), or "" if empty. */
  d: string;
  /** Render group: filled country vs stroke-only subnational border. */
  layer: BasemapLayerKind;
}

/** Project one ring to a closed `M…L…Z` sub-path in world coords. */
function ringToPath(ring: Ring): string {
  let d = "";
  for (let i = 0; i < ring.length; i += 1) {
    const point = ring[i];
    if (!point) continue;
    const [wx, wy] = project(point[0], point[1]);
    d += `${i === 0 ? "M" : "L"}${wx} ${wy}`;
  }
  return d === "" ? "" : `${d}Z`;
}

/**
 * Build the per-feature SVG `d` strings from the basemap geometry. The paths are
 * in **world** coordinates (projected once, view-independent) so the parent
 * `<g transform>` applies pan/zoom and the path vnodes stay referentially stable
 * across pans. Pure (no DOM) so it is unit-testable. A MultiPolygon concatenates
 * one sub-path per ring of every polygon.
 */
export function buildBasemapPaths(
  features: readonly BasemapFeature[] = basemap.features,
): BasemapPathDatum[] {
  return features.map((feature) => {
    const { geometry } = feature;
    const polygons =
      geometry.type === "Polygon" ? [geometry.coordinates] : geometry.coordinates;
    let d = "";
    for (const polygon of polygons) {
      for (const ring of polygon) {
        d += ringToPath(ring);
      }
    }
    return { name: feature.properties.name, d, layer: feature.properties.layer };
  });
}
