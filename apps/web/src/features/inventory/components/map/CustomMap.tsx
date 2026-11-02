import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";

import { buildBasemapPaths } from "@/features/inventory/components/map/basemap";
import {
  fitToPoints,
  project,
  screenToLngLat,
  zoomAt,
  type MapView,
} from "@/features/inventory/components/map/mapProjection";
import { computeRingArcs } from "@/features/inventory/components/map/markerRing";
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
  defaultCenter,
  className,
}: CustomMapProps) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);

  const [size, setSize] = useState({ w: 0, h: 0 });
  const [view, setView] = useState<MapView>({ k: 1, tx: 0, ty: 0 });
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
  // the explicit triggers below. Matches the spec's fit triggers: first data,
  // {w,h}, fitKey.
  const fitInputsRef = useRef({ points, defaultCenter });
  useEffect(() => {
    fitInputsRef.current = { points, defaultCenter };
  }, [points, defaultCenter]);

  const fit = useCallback(() => {
    setSize((current) => {
      if (current.w <= 0 || current.h <= 0) return current;
      const { points: pts, defaultCenter: center } = fitInputsRef.current;
      const result = fitToPoints(pts, current, center);
      zoomBounds.current = { minK: result.minK, maxK: result.maxK };
      setView({ k: result.k, tx: result.tx, ty: result.ty });
      return current;
    });
  }, []);

  // One-time auto-fit: once the container is measured AND (the first non-empty
  // point set has arrived OR a defaultCenter frames an empty map). After that
  // the view is user-owned — only a size change or an explicit fitKey refit it.
  const didAutoFit = useRef(false);
  useEffect(() => {
    if (didAutoFit.current) return;
    if (size.w <= 0 || size.h <= 0) return;
    if (points.length === 0 && !defaultCenter) return;
    didAutoFit.current = true;
    fit();
  }, [fit, size.w, size.h, points.length, defaultCenter]);

  // Refit on a container resize (after the initial fit) so the framing tracks
  // the viewport. Skipped until the one-time auto-fit has run.
  const lastSize = useRef(size);
  useEffect(() => {
    if (!didAutoFit.current) return;
    if (lastSize.current.w === size.w && lastSize.current.h === size.h) return;
    lastSize.current = size;
    fit();
  }, [fit, size]);

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

  const markers = useMemo(
    () => {
      const labelBoxes: { left: number; right: number; top: number; bottom: number }[] = [];
      const labelOffsets = new Map<string, number>();
      const priorityPoints = [...points].sort((a, b) => Number(b.id === selectedId) - Number(a.id === selectedId) || (b.labelCount ?? 0) - (a.labelCount ?? 0));
      const pinBoxes = points.map(point => {
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
        const selected = point.id === selectedId;
        const fill = point.color ?? "hsl(var(--primary-fill))";
        const stroke = point.strokeColor ?? fill;
        const baseRadius = point.radius ?? BASE_R;
        const r = selected ? Math.max(SELECTED_R, baseRadius + 1.5) : baseRadius;
        const ringRadius = baseRadius + 5;

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

        return (
          <g key={point.id} role={onSelect ? "button" : undefined} tabIndex={onSelect ? 0 : undefined} aria-label={point.label ?? point.id} aria-pressed={onSelect ? selected : undefined} transform={`translate(${wx} ${wy}) scale(${1 / k})`} onPointerDown={event => event.stopPropagation()} onClick={event => { event.stopPropagation(); onSelect?.(point.id); }} onKeyDown={event => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect?.(point.id); } }}>
            <circle r={Math.max(22, r)} fill="transparent" style={{ cursor: "pointer" }} />
            {/* Soft halo behind the pin for legibility over land/water. */}
            <circle r={r + 5} fill={stroke} fillOpacity={selected ? 0.34 : 0.24} />
            <circle
              r={r}
              fill={fill}
              stroke={stroke}
              strokeWidth={selected ? 3 : 2.5}
              style={{ cursor: "pointer" }}
            />
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
                fontSize={point.labelCount === undefined ? BASE_R * 1.1 : Math.min(r, 10)}
                fontWeight="600"
                fill={point.color ? "hsl(var(--card))" : "hsl(var(--primary-foreground))"}
                style={{ pointerEvents: "none", userSelect: "none" }}
              >
                {point.labelCount ?? (totalRelated > 99 ? "99+" : totalRelated)}
              </text>
            )}
            {point.label && labelOffsets.has(point.id) && <text x={0} y={labelOffsets.get(point.id)} textAnchor="middle" fontSize={12} fontWeight={selected ? 600 : 500} fill="hsl(var(--foreground))" stroke="hsl(var(--card))" strokeWidth={4} paintOrder="stroke" style={{ pointerEvents: "none", userSelect: "none" }}>{point.label}</text>}
          </g>
        );
      });
    },
    [points, k, selectedId, onSelect, visibleLayerIds],
  );

  return (
    <div ref={hostRef} className={className} style={{ position: "relative", overflow: "hidden" }}>
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

    </div>
  );
};

export default CustomMap;
