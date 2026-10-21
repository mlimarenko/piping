import type { CustomMapPoint } from "@/features/inventory/components/map/CustomMap";
import type { MapLayerId } from "@/features/inventory/components/map/mapLayers";

export interface InventoryMapFeatureProperties {
  id: string;
  color: string;
  strokeColor: string;
  selected: boolean;
  count: number;
  radius: number;
}

export interface InventoryMapFeature {
  type: "Feature";
  geometry: {
    type: "Point";
    coordinates: [number, number];
  };
  properties: InventoryMapFeatureProperties;
}

export interface InventoryMapFeatureCollection {
  type: "FeatureCollection";
  features: InventoryMapFeature[];
}

export interface InventoryMapCompositionFeatureProperties {
  id: string;
  color: string;
  count: number;
  glyph: string;
  slot: number;
  selected: boolean;
}

export interface InventoryMapCompositionFeature {
  type: "Feature";
  geometry: {
    type: "Point";
    coordinates: [number, number];
  };
  properties: InventoryMapCompositionFeatureProperties;
}

export interface InventoryMapCompositionFeatureCollection {
  type: "FeatureCollection";
  features: InventoryMapCompositionFeature[];
}

function mercatorLatitude(latitude: number): number {
  return Math.max(-85.05112878, Math.min(85.05112878, latitude));
}

export function hasFiniteLngLat(point: Pick<CustomMapPoint, "lat" | "lng">): boolean {
  return (
    Number.isFinite(point.lng) &&
    Number.isFinite(point.lat) &&
    point.lng >= -180 &&
    point.lng <= 180 &&
    point.lat >= -90 &&
    point.lat <= 90
  );
}

export function countVisibleRelated(
  counts: Record<MapLayerId, number> | undefined,
  visibleLayerIds: ReadonlySet<MapLayerId> | undefined,
): number {
  if (!counts || !visibleLayerIds || visibleLayerIds.size === 0) return 0;
  let total = 0;
  for (const layerId of visibleLayerIds) {
    total += counts[layerId] ?? 0;
  }
  return total;
}

export function buildInventoryFeatureCollection(
  points: readonly CustomMapPoint[],
  selectedId: string | null | undefined,
  visibleLayerIds: ReadonlySet<MapLayerId> | undefined,
): InventoryMapFeatureCollection {
  return {
    type: "FeatureCollection",
    features: points.filter(hasFiniteLngLat).map((point) => ({
      type: "Feature",
      geometry: {
        type: "Point",
        // WGS-84 poles are valid locations; Mercator renders them at its edge.
        coordinates: [point.lng, mercatorLatitude(point.lat)],
      },
      properties: {
        id: point.id,
        color: point.color ?? "hsl(43 91% 53%)",
        strokeColor: point.strokeColor ?? point.color ?? "hsl(43 91% 53%)",
        selected: point.id === selectedId,
        count: point.labelCount ?? countVisibleRelated(point.relatedCounts, visibleLayerIds),
        radius: point.radius ?? 6,
      },
    })),
  };
}

function compositionGlyph(iconName: string): string {
  switch (iconName) {
    case "monitor":
    case "laptop":
      return "▣";
    case "cctv":
    case "camera":
      return "◉";
    case "router":
    case "switch":
    case "network":
      return "⌁";
    case "wifi":
      return "≋";
    case "phone":
      return "☎";
    case "server":
    case "device":
      return "▦";
    case "printer":
      return "▤";
    case "tenant":
      return "●";
    case "cluster":
      return "◆";
    case "platform":
      return "▧";
    case "shield":
      return "◆";
    default:
      return "●";
  }
}

export function buildInventoryCompositionFeatureCollection(
  points: readonly CustomMapPoint[],
  selectedId: string | null | undefined,
): InventoryMapCompositionFeatureCollection {
  return {
    type: "FeatureCollection",
    features: points.filter(hasFiniteLngLat).flatMap((point) =>
      (point.composition ?? []).slice(0, 4).map((item, slot) => ({
        type: "Feature" as const,
        geometry: {
          type: "Point" as const,
          coordinates: [point.lng, mercatorLatitude(point.lat)] as [number, number],
        },
        properties: {
          id: point.id,
          color: item.color,
          count: item.count,
          glyph: compositionGlyph(item.iconName),
          slot,
          selected: point.id === selectedId,
        },
      })),
    ),
  };
}

export function boundsForPoints(points: readonly CustomMapPoint[]): [[number, number], [number, number]] | null {
  const valid = points.filter(hasFiniteLngLat);
  if (valid.length === 0) return null;

  let minLng = Infinity;
  let minLat = Infinity;
  let maxLng = -Infinity;
  let maxLat = -Infinity;

  for (const point of valid) {
    minLng = Math.min(minLng, point.lng);
    minLat = Math.min(minLat, point.lat);
    maxLng = Math.max(maxLng, point.lng);
    maxLat = Math.max(maxLat, point.lat);
  }

  if (minLng === maxLng && minLat === maxLat) {
    const padding = 0.4;
    return [
      [Math.max(-180, minLng - padding), mercatorLatitude(minLat - padding)],
      [Math.min(180, maxLng + padding), mercatorLatitude(maxLat + padding)],
    ];
  }

  return [
    [minLng, mercatorLatitude(minLat)],
    [maxLng, mercatorLatitude(maxLat)],
  ];
}
