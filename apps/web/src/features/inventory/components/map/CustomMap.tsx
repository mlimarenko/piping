import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { useTranslation } from "react-i18next";

import { buildBasemapPaths } from "@/features/inventory/components/map/basemap";
import {
  fitToPoints,
  project,
  screenToLngLat,
  zoomAt,
  type MapView,
} from "@/features/inventory/components/map/mapProjection";
import { computeRingArcs } from "@/features/inventory/components/map/markerRing";
import { formatMapCount, MAP_COUNT_TEXT_SIZE, MAP_FOCUS_SCALE, MAP_HIT_RADIUS, mapMarkerRadius } from "@/features/inventory/components/map/mapCount";
import { canSeparateGroup, groupMapPoints, type MapGroup } from "@/features/inventory/components/map/mapGroups";
import { MapGroupList } from "@/features/inventory/components/map/MapGroupList";
import { countVisibleRelated } from "@/features/inventory/components/map/mapLibreObjects";
import type { MapLayerId } from "@/features/inventory/components/map/mapLayers";
import type { MapMarkerCompositionItem } from "@/features/inventory/components/map/mapMarkerComposition";

export interface CustomMapPoint {
  id: string;
  lat: number;
  lng: number;
  label?: string;
  /** Marker paint; defaults to `hsl(var(--primary-fill))`, not primary text ink. */
  color?: string;
  /** Theme-aware outline/halo colour for map legibility. */
  strokeColor?: string;
  /** Constant-screen marker radius in pixels. */
  radius?: number;
  /** Optional numeric badge. Defaults to derived visible related counts. */
  labelCount?: number;
  /** Internal presentation metadata for a grouped location marker. */
  clusterMemberCount?: number;
  /** Related-entity counts by layer id. Drives the segmented ring + count badge.
   *  Absent → plain circle marker (no ring, no badge). */
  relatedCounts?: Record<MapLayerId, number>;
  /** Device/site composition for rich map markers in the detailed map and
   *  compact colour chips in the SVG fallback. */
  composition?: MapMarkerCompositionItem[];
}

export interface CustomMapProps {
  points: CustomMapPoint[];
  selectedId?: string | null;
  onSelect?: (id: string | null) => void;
  /** Layer ids the legend currently shows — filters which related counts feed the
   *  marker ring + badge. Omitted → no ring/badge (plain markers). */
  visibleLayerIds?: ReadonlySet<MapLayerId>;
  /** Picker mode: a map click reports the clicked `lat`/`lng`. */
  onMapClick?: (lat: number, lng: number) => void;
  /** Change this to refit the view to `points` (e.g. a "fit all" button). */
  fitKey?: unknown;
  /** Explicitly focus a point; selection alone never moves the camera. */
  focusId?: string | null;
  focusKey?: unknown;
  /** `[lat, lng]` framed when `points` is empty (defaults to world-centre). */
  defaultCenter?: [number, number];
  className?: string;
}

/** Below this drag distance (px) a pointer gesture counts as a click. */
const CLICK_SLOP = 4;
/** Wheel zoom step. */
const ZOOM_IN = 1.2;
const ZOOM_OUT = 1 / 1.2;

/** Composite marker geometry (world units; strokes are non-scaling). */
const BASE_R = 6;
const SELECTED_R = 7.5;
/** Ring radius — sits 5px outside the base circle edge. */
const RING_STROKE_W = 2.5;
const COMPOSITION_DOT_R = 2.2;

/**
 * Precomputed basemap path data (world coords, projected once at module load).
 * Computing it here keeps the per-feature `<path>` vnodes referentially stable
 * across every pan, which is what lets React skip re-rendering the basemap on
 * drag. The builder lives in `basemap.ts` so it can be unit-tested pure.
 */
const BASEMAP_PATHS = buildBasemapPaths();
const COUNTRY_PATHS = BASEMAP_PATHS.filter((feature) => feature.layer === "country");
const SUBNATIONAL_PATHS = BASEMAP_PATHS.filter((feature) => feature.layer === "subnational");

/**
 * Two stacked `<g>` groups: filled national shapes first, then stroke-only
 * admin-1 (province/oblast) borders on top. Constant vnodes (module-precomputed)
 * so a pan never re-renders the basemap.
 */
const BasemapLayer = (
  <g>
    <g>
      {COUNTRY_PATHS.map((feature) => (
        <path
          key={feature.name}
          d={feature.d}
          fill="hsl(var(--muted))"
          fillOpacity={0.55}
          stroke="hsl(var(--border))"
          strokeWidth={0.75}
          vectorEffect="non-scaling-stroke"
        />
      ))}
    </g>
    <g>
      {SUBNATIONAL_PATHS.map((feature) => (
        <path
          key={feature.name}
          d={feature.d}
          fill="none"
          stroke="hsl(var(--border))"
          strokeWidth={0.4}
          strokeOpacity={0.6}
          strokeDasharray="2 3"
          vectorEffect="non-scaling-stroke"
        />
      ))}
    </g>
  </g>
);

/**
 * Dependency-free, fully-offline SVG map. Renders the bundled basemap and a set
 * of constant-screen-size markers under a single pan/zoom transform, with an
 * HTML popup overlay and an optional picker click-to-place mode. No tiles, no
 * map framework — usable in a closed network.
 */
const CustomMap = ({
  points,
  selectedId = null,
  onSelect,
  visibleLayerIds,
  onMapClick,
  fitKey,
  focusId,
  focusKey,
  defaultCenter,
  className,
}: CustomMapProps) => {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);

  const [size, setSize] = useState({ w: 0, h: 0 });
  const sizeRef = useRef(size);
  const [view, setView] = useState<MapView>({ k: 1, tx: 0, ty: 0 });
  const [revealedIds, setRevealedIds] = useState<string[]>([]);
  const zoomBounds = useRef({ minK: 1 / 8, maxK: 64 });

  // Latest view in a ref so the picker click handler can invert against fresh
  // values without re-subscribing. Synced in an effect (never during render).
  const viewRef = useRef(view);
  useEffect(() => {
    viewRef.current = view;
  }, [view]);

  // --- Container sizing ---------------------------------------------------
  // Measure synchronously on mount (so the first paint has a real size without
  // waiting for a ResizeObserver tick), then track changes via ResizeObserver.
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const apply = (width: number, height: number) => {
      sizeRef.current = { w: width, h: height };
      setSize((prev) =>
        prev.w === width && prev.h === height ? prev : { w: width, h: height },
      );
    };

    const initial = host.getBoundingClientRect();
    apply(initial.width, initial.height);

    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const { width, height } = entry.contentRect;
      apply(width, height);
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  // --- Fit-to-points -----------------------------------------------------
  // Fresh `points`/`defaultCenter` in refs so `fit` has a stable identity and
  // does NOT re-fit on every points change (e.g. a search keystroke) — only on
  // the explicit triggers below: first data and fitKey.
  const fitInputsRef = useRef({ points, defaultCenter });
  useEffect(() => {
    fitInputsRef.current = { points, defaultCenter };
  }, [points, defaultCenter]);

  const fit = useCallback(() => {
    const current = sizeRef.current;
    if (current.w <= 0 || current.h <= 0) return;
    const { points: pts, defaultCenter: center } = fitInputsRef.current;
    const result = fitToPoints(pts, current, center);
    zoomBounds.current = { minK: result.minK, maxK: result.maxK };
    setView({ k: result.k, tx: result.tx, ty: result.ty });
  }, []);

  // One-time auto-fit: once the container is measured AND (the first non-empty
  // point set has arrived OR a defaultCenter frames an empty map). After that
  // the view is user-owned — only an explicit fitKey refits it.
  const didAutoFit = useRef(false);
  useEffect(() => {
    if (didAutoFit.current) return;
    if (size.w <= 0 || size.h <= 0) return;
    if (points.length === 0 && !defaultCenter) return;
    didAutoFit.current = true;
    fit();
  }, [fit, size.w, size.h, points.length, defaultCenter]);

  // Preserve the geographic center and scale when the viewport changes.
  const lastSize = useRef(size);
  useEffect(() => {
    const previous = lastSize.current;
    lastSize.current = size;
    if (!didAutoFit.current || previous.w <= 0 || previous.h <= 0 || size.w <= 0 || size.h <= 0) return;
    if (previous.w === size.w && previous.h === size.h) return;
    setView(view => ({ ...view, tx: view.tx + (size.w - previous.w) / 2, ty: view.ty + (size.h - previous.h) / 2 }));
  }, [size]);

  // Explicit refit when the caller bumps fitKey (e.g. a "fit to all" button).
  // Skip the initial render so it only fires on a real change.
  const didMountFitKey = useRef(false);
  useEffect(() => {
    if (!didMountFitKey.current) {
      didMountFitKey.current = true;
      return;
    }
    fit();
  }, [fit, fitKey]);

  const lastFocus = useRef<{ id: string; key: unknown } | null>(null);
  useEffect(() => {
    if (!focusId) {
      lastFocus.current = null;
      return;
    }
    if (lastFocus.current?.id === focusId && Object.is(lastFocus.current.key, focusKey)) return;
    if (!focusId || size.w <= 0 || size.h <= 0) return;
    const point = fitInputsRef.current.points.find((item) => item.id === focusId);
    if (!point || !Number.isFinite(point.lng) || !Number.isFinite(point.lat) || Math.abs(point.lng) > 180 || Math.abs(point.lat) > 90) return;
    lastFocus.current = { id: focusId, key: focusKey };
    const [x, y] = project(point.lng, point.lat);
    setView((previous) => {
      const k = Math.max(previous.k, MAP_FOCUS_SCALE);
      zoomBounds.current.maxK = Math.max(zoomBounds.current.maxK, k);
      return { k, tx: size.w / 2 - x * k, ty: size.h / 2 - y * k };
    });
  }, [focusId, focusKey, points, size.w, size.h]);

  // --- Pan (pointer drag, with click discrimination) ---------------------
  const drag = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    lastX: number;
    lastY: number;
    moved: boolean;
  } | null>(null);

  const handlePointerDown = useCallback((event: ReactPointerEvent<SVGSVGElement>) => {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      moved: false,
    };
  }, []);

  const handlePointerMove = useCallback((event: ReactPointerEvent<SVGSVGElement>) => {
    const state = drag.current;
    if (!state || state.pointerId !== event.pointerId) return;
    const dx = event.clientX - state.lastX;
    const dy = event.clientY - state.lastY;
    state.lastX = event.clientX;
    state.lastY = event.clientY;
    if (
      !state.moved &&
      Math.hypot(event.clientX - state.startX, event.clientY - state.startY) > CLICK_SLOP
    ) {
      state.moved = true;
    }
    if (state.moved) {
      setView((prev) => ({ k: prev.k, tx: prev.tx + dx, ty: prev.ty + dy }));
    }
  }, []);

  const endDrag = useCallback((event: ReactPointerEvent<SVGSVGElement>) => {
    const state = drag.current;
    if (!state || state.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    drag.current = null;
  }, []);

  /** True when the just-finished gesture was a drag, not a click. */
  const consumedDragRef = useRef(false);

  const handlePointerUp = useCallback(
    (event: ReactPointerEvent<SVGSVGElement>) => {
      consumedDragRef.current = drag.current?.moved ?? false;
      endDrag(event);
    },
    [endDrag],
  );

  // --- Wheel zoom (non-passive native listener so preventDefault works) ---
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = svg.getBoundingClientRect();
      const cx = event.clientX - rect.left;
      const cy = event.clientY - rect.top;
      const factor = event.deltaY < 0 ? ZOOM_IN : ZOOM_OUT;
      const { minK, maxK } = zoomBounds.current;
      setView((prev) => zoomAt(prev, factor, cx, cy, minK, maxK));
    };
    svg.addEventListener("wheel", onWheel, { passive: false });
    return () => svg.removeEventListener("wheel", onWheel);
    // Re-run once the SVG actually mounts: on first paint `size` is {0,0} so the
    // `<svg>` (and thus `svgRef.current`) does not exist yet and the listener
    // would never attach. Re-binding on size change fixes wheel-zoom and keeps
    // the anchor math correct across container resizes.
  }, [size.w, size.h]);

  // --- Click handling (map / background) ---------------------------------
  const handleClick = useCallback(
    (event: ReactPointerEvent<SVGSVGElement>) => {
      // Swallow the click that terminates a pan so a drag never selects/places.
      if (consumedDragRef.current) {
        consumedDragRef.current = false;
        return;
      }
      const svg = svgRef.current;
      if (!svg) return;
      const rect = svg.getBoundingClientRect();
      const sx = event.clientX - rect.left;
      const sy = event.clientY - rect.top;
      if (onMapClick) {
        const [lng, lat] = screenToLngLat(sx, sy, viewRef.current);
        onMapClick(lat, lng);
        return;
      }
      // Non-picker: a background click dismisses any open popup.
      onSelect?.(null);
    },
    [onMapClick, onSelect],
  );

  // --- Markers (memoized; independent of tx/ty so a pan reuses them) ------
  const { k, tx, ty } = view;
  const groups = useMemo(() => groupMapPoints(points, k, "unwrapped"), [points, k]);
  const activateGroup = useCallback((group: MapGroup) => {
    if (group.members.length === 1) {
      onSelect?.(group.point.id);
      return;
    }
    const max = zoomBounds.current.maxK;
    if (k >= max || !canSeparateGroup(group.members, max, "unwrapped")) {
      setRevealedIds(group.members.map(point => point.id));
      return;
    }
    const fitted = fitToPoints(group.members, size);
    const nextK = Math.min(max, Math.max(k * 2, fitted.k));
    const [x, y] = project(group.point.lng, group.point.lat);
    setView({ k: nextK, tx: size.w / 2 - x * nextK, ty: size.h / 2 - y * nextK });
  }, [k, onSelect, size]);
  const revealedPoints = points.filter(point => revealedIds.includes(point.id));

  const markers = useMemo(
    () => {
      const labelBoxes: { left: number; right: number; top: number; bottom: number }[] = [];
      const labelOffsets = new Map<string, number>();
      const groupedPoints = groups.map(group => group.point);
      const selectedGroupId = groups.find(group => group.members.some(point => point.id === selectedId))?.point.id;
      const priorityPoints = [...groupedPoints].sort((a, b) => Number(b.id === selectedGroupId) - Number(a.id === selectedGroupId) || (b.labelCount ?? 0) - (a.labelCount ?? 0));
      const pinBoxes = groupedPoints.map(point => {
        const [x, y] = project(point.lng, point.lat);
        const radius = (point.radius ?? BASE_R) + 4;
        return { id: point.id, x: x * k, y: y * k, radius, left: x * k - radius, right: x * k + radius, top: y * k - radius, bottom: y * k + radius };
      });
      const visiblePins: typeof pinBoxes = [];
      for (const point of priorityPoints) {
        const pin = pinBoxes.find(box => box.id === point.id);
        if (pin && !visiblePins.some(other => Math.hypot(pin.x - other.x, pin.y - other.y) < pin.radius + other.radius)) visiblePins.push(pin);
      }
      const visiblePinIds = new Set(visiblePins.map(pin => pin.id));
      // SVG has no symbol collision engine. Keep selected labels first and hide
      // overlapping neighbours; every marker remains keyboard/click accessible.
      for (const point of priorityPoints) {
        if (!point.label || !visiblePinIds.has(point.id)) continue;
        const [x, y] = project(point.lng, point.lat);
        const halfWidth = point.label.length * 3.5 + 6;
        const baseRadius = point.radius ?? BASE_R;
        const radius = point.id === selectedId ? Math.max(SELECTED_R, baseRadius + 1.5) : baseRadius;
        for (const offset of [radius + 18, -radius - 12]) {
          const top = y * k + offset - 12;
          const box = { left: x * k - halfWidth, right: x * k + halfWidth, top, bottom: top + 20 };
          const overlaps = (other: typeof box) => box.left < other.right && box.right > other.left && box.top < other.bottom && box.bottom > other.top;
          if (labelBoxes.some(overlaps) || visiblePins.some(other => other.id !== point.id && overlaps(other))) continue;
          labelBoxes.push(box);
          labelOffsets.set(point.id, offset);
          break;
        }
      }
      // Preserve real positions, but paint selected/dense pins above nearby small
      // pins so an overlapping count cannot obscure the chosen location's badge.
      return [...priorityPoints].reverse().map((point) => {
        const [wx, wy] = project(point.lng, point.lat);
        const group = groups.find(item => item.point.id === point.id)!;
        const selected = point.id === selectedGroupId;
        const fill = point.color ?? "hsl(var(--primary-fill))";
        const stroke = point.strokeColor ?? fill;
        const baseRadius = point.radius ?? BASE_R;
        const ringRadius = mapMarkerRadius(baseRadius, point.labelCount ?? countVisibleRelated(point.relatedCounts, visibleLayerIds)) + 5;

        // Composite ring + count badge from the related-entity counts, scoped to
        // the visible layers. Empty arcs (no visible related counts) → plain pin.
        const counts = point.relatedCounts;
        const arcs =
          counts && visibleLayerIds
            ? computeRingArcs(counts, visibleLayerIds, ringRadius)
            : [];
        const totalRelated = point.labelCount ?? arcs.reduce(
          (sum, arc) => sum + (counts?.[arc.layerId] ?? 0),
          0,
        );
        const badgeRadius = mapMarkerRadius(baseRadius, totalRelated);
        const r = badgeRadius;
        const markerLabel = group.members.length > 1 ? t("inventory.map.clusterCount", { count: group.members.length, defaultValue: "{{count}} locations" }) : point.label ?? point.id;

        return (
          <g key={point.id} role={onSelect ? "button" : undefined} tabIndex={onSelect ? 0 : undefined} aria-label={markerLabel} aria-description={group.members.length === 1 && totalRelated > 0 ? String(totalRelated) : undefined} aria-pressed={onSelect ? selected : undefined} transform={`translate(${wx} ${wy}) scale(${1 / k})`} onPointerDown={event => { if (onSelect) event.stopPropagation(); }} onClick={event => { if (onSelect) { event.stopPropagation(); activateGroup(group); } }} onKeyDown={event => { if (onSelect && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); activateGroup(group); } }}>
            {totalRelated > 0 && <title>{group.members.length > 1 ? markerLabel : `${markerLabel}: ${totalRelated}`}</title>}
            <circle r={Math.max(MAP_HIT_RADIUS, r)} fill="transparent" style={{ cursor: "pointer" }} />
            {/* Soft halo behind the pin for legibility over land/water. */}
            <circle r={r + 5} fill={stroke} fillOpacity={selected ? 0.34 : 0.24} />
            <circle
              r={r}
              fill={fill}
              stroke={stroke}
              strokeWidth={selected ? 3 : 2.5}
              style={{ cursor: "pointer" }}
            />
            {selected && <circle r={r + 3} fill="none" stroke="hsl(var(--foreground))" strokeWidth={2} style={{ pointerEvents: "none" }} />}
            {group.members.length > 1 && <circle r={r + 4} fill="none" stroke="hsl(var(--foreground))" strokeWidth={2} strokeDasharray="3 2" style={{ pointerEvents: "none" }} />}
            {/* Segmented ring — one arc per visible related entity type. */}
            {arcs.map((arc) => (
              <circle
                key={arc.layerId}
                r={ringRadius}
                fill="none"
                stroke={arc.color}
                strokeWidth={RING_STROKE_W}
                strokeDasharray={arc.dashArray}
                strokeDashoffset={arc.dashOffset}
                vectorEffect="non-scaling-stroke"
                style={{ pointerEvents: "none" }}
              />
            ))}
            {point.composition?.slice(0, 4).map((item, index, items) => {
              const angle = -Math.PI / 2 + (Math.PI * 2 * index) / Math.max(items.length, 1);
              const dotDistance = ringRadius + 4;
              return (
                <circle
                  key={item.value}
                  cx={Math.cos(angle) * dotDistance}
                  cy={Math.sin(angle) * dotDistance}
                  r={COMPOSITION_DOT_R}
                  fill={item.color}
                  stroke="hsl(var(--card))"
                  strokeWidth={1}
                  style={{ pointerEvents: "none" }}
                />
              );
            })}
            {/* Centered count badge — total visible related entities (hidden at 0). */}
            {totalRelated > 0 && (
              <text
                x={0}
                y={1}
                textAnchor="middle"
                dominantBaseline="middle"
                fontSize={MAP_COUNT_TEXT_SIZE}
                fontWeight="600"
                fill={point.color ? "hsl(var(--card))" : "hsl(var(--primary-foreground))"}
                style={{ pointerEvents: "none", userSelect: "none" }}
              >
                {formatMapCount(totalRelated)}
              </text>
            )}
            {group.members.length > 1 && <text x={0} y={r + 19} textAnchor="middle" fontSize={12} fontWeight={600} fill="hsl(var(--foreground))" stroke="hsl(var(--card))" strokeWidth={4} paintOrder="stroke" style={{ pointerEvents: "none", userSelect: "none" }}>{markerLabel}</text>}
            {point.label && labelOffsets.has(point.id) && <text x={0} y={labelOffsets.get(point.id)} textAnchor="middle" fontSize={12} fontWeight={selected ? 600 : 500} fill="hsl(var(--foreground))" stroke="hsl(var(--card))" strokeWidth={4} paintOrder="stroke" style={{ pointerEvents: "none", userSelect: "none" }}>{point.label}</text>}
          </g>
        );
      });
    },
    [groups, k, selectedId, onSelect, visibleLayerIds, activateGroup, t],
  );

  return (
    <div ref={hostRef} tabIndex={-1} className={className} style={{ position: "relative", overflow: "hidden" }}>
      {size.w > 0 && size.h > 0 && (
        <svg
          ref={svgRef}
          width={size.w}
          height={size.h}
          viewBox={`0 0 ${size.w} ${size.h}`}
          style={{ display: "block", touchAction: "none", cursor: onMapClick ? "crosshair" : "grab" }}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={endDrag}
          onClick={handleClick}
        >
          <rect x={0} y={0} width={size.w} height={size.h} fill="hsl(var(--card))" />
          <g transform={`translate(${tx} ${ty}) scale(${k})`}>
            {BasemapLayer}
            {markers}
          </g>
        </svg>
      )}
      {revealedPoints.length > 0 && <MapGroupList points={revealedPoints} onSelect={onSelect} onClose={() => setRevealedIds([])} fallbackFocusRef={hostRef} />}

    </div>
  );
};

export default CustomMap;
