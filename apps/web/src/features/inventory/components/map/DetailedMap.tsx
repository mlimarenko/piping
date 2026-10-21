import "maplibre-gl/dist/maplibre-gl.css";

import { useCallback, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import * as maplibregl from "maplibre-gl";
import mapWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import type {
  FilterSpecification,
  GeoJSONSource,
  LngLatLike,
  Map,
  MapLayerMouseEvent,
  MapMouseEvent,
  StyleSpecification,
} from "maplibre-gl";
import { Protocol } from "pmtiles";

import type { CustomMapPoint } from "@/features/inventory/components/map/CustomMap";
import {
  detailedMapConfig,
  toBrowserAssetUrl,
  toPmtilesFetchUrl,
  toPmtilesProtocolUrl,
  type DetailedMapConfig,
} from "@/features/inventory/components/map/detailedMapConfig";
import {
  boundsForPoints,
  buildInventoryCompositionFeatureCollection,
  buildInventoryFeatureCollection,
} from "@/features/inventory/components/map/mapLibreObjects";
import {
  localizeMapStyleLabels,
  mapLabelLanguageForI18n,
  type MapLabelLanguage,
} from "@/features/inventory/components/map/mapStyleLanguage";
import type { MapLayerId } from "@/features/inventory/components/map/mapLayers";
import { cn } from "@/shared/lib/utils";

interface DetailedMapProps {
  points: CustomMapPoint[];
  selectedId?: string | null;
  onSelect?: (id: string | null) => void;
  visibleLayerIds?: ReadonlySet<MapLayerId>;
  onMapClick?: (lat: number, lng: number) => void;
  fitKey?: unknown;
  defaultCenter?: [number, number];
  className?: string;
  config?: DetailedMapConfig;
  onReady?: () => void;
  onUnavailable?: (reason: string) => void;
}

const OBJECT_SOURCE_ID = "piping-inventory-sites";
const OBJECT_HALO_LAYER_ID = "piping-inventory-sites-halo";
const OBJECT_CIRCLE_LAYER_ID = "piping-inventory-sites-circle";
const OBJECT_SELECTED_LAYER_ID = "piping-inventory-sites-selected";
const OBJECT_COUNT_LAYER_ID = "piping-inventory-sites-count";
const COMPOSITION_SOURCE_ID = "piping-inventory-site-composition";
const COMPOSITION_SLOT_COUNT = 4;
const COMPOSITION_CIRCLE_LAYER_IDS = Array.from(
  { length: COMPOSITION_SLOT_COUNT },
  (_, index) => `piping-inventory-composition-${index}-circle`,
);
const COMPOSITION_TEXT_LAYER_IDS = Array.from(
  { length: COMPOSITION_SLOT_COUNT },
  (_, index) => `piping-inventory-composition-${index}-text`,
);
const COMPOSITION_TRANSLATES: readonly [number, number][] = [
  [23, -12],
  [23, 12],
  [40, 0],
  [57, 0],
];

let pmtilesProtocolRegistered = false;

function ensurePmtilesProtocol(): void {
  if (pmtilesProtocolRegistered) return;
  // MapLibre's relative worker URL cannot survive Vite's hashed vendor chunks.
  maplibregl.setWorkerUrl(mapWorkerUrl);
  const protocol = new Protocol({ metadata: true });
  maplibregl.addProtocol("pmtiles", protocol.tile);
  pmtilesProtocolRegistered = true;
}

async function loadLocalStyle(
  config: DetailedMapConfig,
  language: MapLabelLanguage,
): Promise<StyleSpecification> {
  const response = await fetch(config.styleUrl, { credentials: "same-origin" });
  if (!response.ok) {
    throw new Error(`Map style request failed: ${response.status}`);
  }

  const style = (await response.json()) as StyleSpecification;
  const source = style.sources?.[config.sourceId];
  if (!source || source.type !== "vector") {
    throw new Error(`Map style does not define vector source "${config.sourceId}"`);
  }

  return localizeMapStyleLabels({
    ...style,
    glyphs: typeof style.glyphs === "string" ? toBrowserAssetUrl(style.glyphs) : style.glyphs,
    sprite: typeof style.sprite === "string" ? toBrowserAssetUrl(style.sprite) : style.sprite,
    sources: {
      ...style.sources,
      [config.sourceId]: {
        ...source,
        type: "vector",
        url: toPmtilesProtocolUrl(config.pmtilesUrl),
      },
    },
  }, language);
}

async function assertPmtilesReadable(config: DetailedMapConfig): Promise<void> {
  const url = toPmtilesFetchUrl(config.pmtilesUrl);
  const response = await fetch(url, {
    cache: "no-store",
    credentials: "same-origin",
    headers: { Range: "bytes=0-15" },
  });

  if (response.status !== 206) {
    throw new Error(`PMTiles range request failed: ${response.status}`);
  }

  const bytes = new Uint8Array(await response.arrayBuffer());
  const magic = String.fromCharCode(...bytes.slice(0, 7));
  if (magic !== "PMTiles") {
    throw new Error("PMTiles header mismatch");
  }
}

function inventoryIdFromFeature(event: MapLayerMouseEvent): string | null {
  const properties: unknown = event.features?.[0]?.properties;
  if (!properties || typeof properties !== "object" || !("id" in properties)) return null;
  const raw = (properties as { id?: unknown }).id;
  return typeof raw === "string" && raw.length > 0 ? raw : null;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.length > 0) return message;
  }
  return "MapLibre error";
}

function getObjectSource(map: Map): GeoJSONSource | null {
  const source = map.getSource(OBJECT_SOURCE_ID);
  return source && "setData" in source ? (source as GeoJSONSource) : null;
}

function getCompositionSource(map: Map): GeoJSONSource | null {
  const source = map.getSource(COMPOSITION_SOURCE_ID);
  return source && "setData" in source ? (source as GeoJSONSource) : null;
}

function inventoryIdFromAnyFeature(event: MapLayerMouseEvent): string | null {
  const properties: unknown = event.features?.[0]?.properties;
  if (!properties || typeof properties !== "object" || !("id" in properties)) return null;
  const raw = (properties as { id?: unknown }).id;
  return typeof raw === "string" && raw.length > 0 ? raw : null;
}

const DetailedMap = ({
  points,
  selectedId = null,
  onSelect,
  visibleLayerIds,
  onMapClick,
  fitKey,
  defaultCenter,
  className,
  config = detailedMapConfig,
  onReady,
  onUnavailable,
}: DetailedMapProps) => {
  const { i18n } = useTranslation();
  const mapLanguage = mapLabelLanguageForI18n(i18n.resolvedLanguage ?? i18n.language);
  const hostRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<Map | null>(null);
  const loadedRef = useRef(false);
  const didInitialFitRef = useRef(false);
  const callbacksRef = useRef({ onSelect, onMapClick, onReady, onUnavailable });
  const pointsRef = useRef(points);

  useEffect(() => { pointsRef.current = points; }, [points]);

  const fitCurrentPoints = useCallback(() => {
    const map = mapRef.current;
    if (!map || !loadedRef.current) return;
    const bounds = boundsForPoints(pointsRef.current);
    if (!bounds) return;
    map.fitBounds(bounds, { padding: 72, duration: didInitialFitRef.current ? 350 : 0 });
    didInitialFitRef.current = true;
  }, []);

  const featureCollection = useMemo(
    () => buildInventoryFeatureCollection(points, selectedId, visibleLayerIds),
    [points, selectedId, visibleLayerIds],
  );
  const compositionFeatureCollection = useMemo(
    () => buildInventoryCompositionFeatureCollection(points, selectedId),
    [points, selectedId],
  );
  const featureCollectionRef = useRef(featureCollection);
  const compositionFeatureCollectionRef = useRef(compositionFeatureCollection);

  useEffect(() => {
    featureCollectionRef.current = featureCollection;
  }, [featureCollection]);

  useEffect(() => {
    compositionFeatureCollectionRef.current = compositionFeatureCollection;
  }, [compositionFeatureCollection]);

  useEffect(() => {
    callbacksRef.current = { onSelect, onMapClick, onReady, onUnavailable };
  }, [onSelect, onMapClick, onReady, onUnavailable]);

  useEffect(() => {
    let cancelled = false;
    let timeoutId: number | undefined;

    const failBeforeLoad = (reason: string) => {
      if (cancelled || loadedRef.current) return;
      callbacksRef.current.onUnavailable?.(reason);
    };

    async function initMap() {
      const host = hostRef.current;
      if (!host) return;

      try {
        ensurePmtilesProtocol();
        const style = await loadLocalStyle(config, mapLanguage);
        await assertPmtilesReadable(config);
        if (cancelled) return;

        const center: LngLatLike = defaultCenter
          ? [defaultCenter[1], defaultCenter[0]]
          : config.initialCenter;

        const activeMap = new maplibregl.Map({
          container: host,
          style,
          center,
          zoom: config.initialZoom,
          attributionControl: false,
          cooperativeGestures: false,
          hash: false,
        });
        mapRef.current = activeMap;

        timeoutId = window.setTimeout(() => {
          failBeforeLoad("Map load timeout");
        }, config.loadTimeoutMs);

        activeMap.addControl(new maplibregl.NavigationControl({ visualizePitch: false }), "top-right");
        activeMap.addControl(new maplibregl.AttributionControl({ compact: true }), "bottom-right");

        activeMap.on("error", (event) => {
          const error: unknown = event.error;
          console.warn("[inventory-map] detailed map resource warning:", errorMessage(error), error);
        });

        activeMap.on("load", () => {
          if (cancelled) return;
          window.clearTimeout(timeoutId);
          loadedRef.current = true;

          activeMap.addSource(OBJECT_SOURCE_ID, {
            type: "geojson",
            data: featureCollectionRef.current,
          });
          activeMap.addSource(COMPOSITION_SOURCE_ID, {
            type: "geojson",
            data: compositionFeatureCollectionRef.current,
          });

          activeMap.addLayer({
            id: OBJECT_HALO_LAYER_ID,
            type: "circle",
            source: OBJECT_SOURCE_ID,
            paint: {
              "circle-color": ["get", "strokeColor"],
              "circle-radius": ["+", ["get", "radius"], ["case", ["get", "selected"], 8, 6]],
              "circle-blur": 0.35,
              "circle-opacity": ["case", ["get", "selected"], 0.42, 0.28],
            },
          });

          activeMap.addLayer({
            id: OBJECT_CIRCLE_LAYER_ID,
            type: "circle",
            source: OBJECT_SOURCE_ID,
            paint: {
              "circle-color": ["get", "color"],
              "circle-radius": ["case", ["get", "selected"], ["+", ["get", "radius"], 1.5], ["get", "radius"]],
              "circle-stroke-color": ["get", "strokeColor"],
              "circle-stroke-width": ["case", ["get", "selected"], 3, 2.5],
            },
          });

          activeMap.addLayer({
            id: OBJECT_SELECTED_LAYER_ID,
            type: "circle",
            source: OBJECT_SOURCE_ID,
            filter: ["==", ["get", "selected"], true],
            paint: {
              "circle-color": "rgba(246, 180, 25, 0)",
              "circle-radius": ["+", ["get", "radius"], 8],
              "circle-stroke-color": ["get", "strokeColor"],
              "circle-stroke-width": 2,
            },
          });

          activeMap.addLayer({
            id: OBJECT_COUNT_LAYER_ID,
            type: "symbol",
            source: OBJECT_SOURCE_ID,
            layout: {
              "text-field": [
                "case",
                [">", ["get", "count"], 0],
                ["to-string", ["get", "count"]],
                "",
              ],
              "text-font": ["Noto Sans Regular"],
              "text-size": 10,
              "text-offset": [0.9, -0.9],
              "text-allow-overlap": false,
            },
            paint: {
              "text-color": "#f8fafc",
              "text-halo-color": "#0d1117",
              "text-halo-width": 1.6,
            },
          });

          for (let slot = 0; slot < COMPOSITION_SLOT_COUNT; slot += 1) {
            const filter = ["==", ["get", "slot"], slot] as FilterSpecification;
            const translate = COMPOSITION_TRANSLATES[slot] ?? [0, 0];
            activeMap.addLayer({
              id: COMPOSITION_CIRCLE_LAYER_IDS[slot]!,
              type: "circle",
              source: COMPOSITION_SOURCE_ID,
              minzoom: 6.6,
              filter,
              paint: {
                "circle-translate": translate,
                "circle-color": ["get", "color"],
                "circle-radius": ["case", ["get", "selected"], 7.5, 6],
                "circle-stroke-color": "#0d1117",
                "circle-stroke-width": ["case", ["get", "selected"], 2.2, 1.6],
                "circle-opacity": 0.96,
              },
            });
            activeMap.addLayer({
              id: COMPOSITION_TEXT_LAYER_IDS[slot]!,
              type: "symbol",
              source: COMPOSITION_SOURCE_ID,
              minzoom: 6.6,
              filter,
              layout: {
                "text-field": [
                  "concat",
                  ["get", "glyph"],
                  [
                    "case",
                    [">", ["get", "count"], 1],
                    ["concat", " ", ["to-string", ["get", "count"]]],
                    "",
                  ],
                ],
                "text-font": ["Noto Sans Regular"],
                "text-size": ["case", [">", ["get", "count"], 1], 8.5, 10],
                "text-allow-overlap": false,
                "text-ignore-placement": false,
              },
              paint: {
                "text-translate": translate,
                "text-color": "#0d1117",
                "text-halo-color": "rgba(248,250,252,0.75)",
                "text-halo-width": 0.8,
              },
            });
          }

          activeMap.on("click", OBJECT_CIRCLE_LAYER_ID, (event) => {
            event.preventDefault();
            callbacksRef.current.onSelect?.(inventoryIdFromFeature(event));
          });
          for (const layerId of COMPOSITION_CIRCLE_LAYER_IDS) {
            activeMap.on("click", layerId, (event) => {
              event.preventDefault();
              callbacksRef.current.onSelect?.(inventoryIdFromAnyFeature(event));
            });
            activeMap.on("mouseenter", layerId, () => {
              activeMap.getCanvas().style.cursor = "pointer";
            });
            activeMap.on("mouseleave", layerId, () => {
              activeMap.getCanvas().style.cursor = "";
            });
          }
          activeMap.on("mouseenter", OBJECT_CIRCLE_LAYER_ID, () => {
            activeMap.getCanvas().style.cursor = "pointer";
          });
          activeMap.on("mouseleave", OBJECT_CIRCLE_LAYER_ID, () => {
            activeMap.getCanvas().style.cursor = "";
          });
          activeMap.on("click", (event: MapMouseEvent) => {
            if (event.defaultPrevented) return;
            const latestOnMapClick = callbacksRef.current.onMapClick;
            if (latestOnMapClick) {
              latestOnMapClick(event.lngLat.lat, event.lngLat.lng);
              return;
            }
            callbacksRef.current.onSelect?.(null);
          });

          fitCurrentPoints();
          callbacksRef.current.onReady?.();
        });
      } catch (error) {
        failBeforeLoad(error instanceof Error ? error.message : "Map initialization failed");
      }
    }

    void initMap();

    return () => {
      cancelled = true;
      window.clearTimeout(timeoutId);
      loadedRef.current = false;
      didInitialFitRef.current = false;
      mapRef.current?.remove();
      mapRef.current = null;
    };
  }, [config, defaultCenter, mapLanguage, fitCurrentPoints]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !loadedRef.current) return;
    void getObjectSource(map)?.setData(featureCollection);
  }, [featureCollection]);

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !loadedRef.current) return;
    void getCompositionSource(map)?.setData(compositionFeatureCollection);
  }, [compositionFeatureCollection]);

  useEffect(() => {
    fitCurrentPoints();
  }, [points, fitKey, fitCurrentPoints]);

  return (
    <div className={cn("relative h-full w-full overflow-hidden bg-[#0d1117]", className)}>
      <div ref={hostRef} className="h-full w-full" />
    </div>
  );
};

export default DetailedMap;
