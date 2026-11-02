import { describe, expect, it } from "vitest";
import {
  DEFAULT_FIT_K,
  MAX_K_FACTOR,
  MIN_K_FACTOR,
  clamp,
  fitToPoints,
  invert,
  project,
  screenToLngLat,
  zoomAt,
} from "@/features/inventory/components/map/mapProjection";

describe("project / invert", () => {
  it("maps the origin (0,0) to the centre of the world square", () => {
    const [x, y] = project(0, 0);
    expect(x).toBeCloseTo(0.5, 10);
    expect(y).toBeCloseTo(0.5, 10);
  });

  it("maps the lng/lat extents to the world-square corners", () => {
    expect(project(-180, 85.05112878)[0]).toBeCloseTo(0, 6);
    expect(project(-180, 85.05112878)[1]).toBeCloseTo(0, 6);
    expect(project(180, -85.05112878)[0]).toBeCloseTo(1, 6);
    expect(project(180, -85.05112878)[1]).toBeCloseTo(1, 6);
  });

  it("round-trips lng/lat through project → invert", () => {
    const samples: [number, number][] = [
      [37.62, 55.75], // Moscow
      [30.33, 59.93], // St. Petersburg
      [-122.42, 37.77], // San Francisco
      [151.21, -33.87], // Sydney
      [0, 0],
      [-180, 80],
      [179.9, -80],
    ];
    for (const [lng, lat] of samples) {
      const [wx, wy] = project(lng, lat);
      const [rlng, rlat] = invert(wx, wy);
      expect(rlng).toBeCloseTo(lng, 6);
      expect(rlat).toBeCloseTo(lat, 6);
    }
  });

  it("clamps latitudes beyond the Web Mercator band", () => {
    // Both 89° and the clamp limit project to (nearly) the same world y.
    const [, yHigh] = project(0, 89);
    const [, yClamp] = project(0, 85.05112878);
    expect(yHigh).toBeCloseTo(yClamp, 6);
  });
});

describe("fitToPoints", () => {
  const size = { w: 800, h: 600 };

  it("falls back to a world-centred default for an empty point set", () => {
    const fit = fitToPoints([], size);
    expect(fit.k).toBe(DEFAULT_FIT_K);
    // World centre (0.5,0.5) lands at the container centre.
    expect(0.5 * fit.k + fit.tx).toBeCloseTo(size.w / 2, 6);
    expect(0.5 * fit.k + fit.ty).toBeCloseTo(size.h / 2, 6);
  });

  it("falls back to the default when the container is not yet measured", () => {
    const fit = fitToPoints([{ lat: 10, lng: 10 }], { w: 0, h: 0 });
    expect(fit.k).toBe(DEFAULT_FIT_K);
  });

  it("frames the supplied defaultCenter for an empty point set", () => {
    const center: [number, number] = [55.75, 37.62]; // Moscow [lat, lng]
    const fit = fitToPoints([], size, center);
    expect(fit.k).toBe(DEFAULT_FIT_K);
    const [wx, wy] = project(center[1], center[0]);
    // The default centre lands at the container centre (not the Atlantic).
    expect(wx * fit.k + fit.tx).toBeCloseTo(size.w / 2, 6);
    expect(wy * fit.k + fit.ty).toBeCloseTo(size.h / 2, 6);
  });

  it("uses the default city-zoom k for a single point and centres on it", () => {
    const point = { lat: 55.75, lng: 37.62 };
    const fit = fitToPoints([point], size);
    expect(fit.k).toBe(DEFAULT_FIT_K);
    const [wx, wy] = project(point.lng, point.lat);
    expect(wx * fit.k + fit.tx).toBeCloseTo(size.w / 2, 6);
    expect(wy * fit.k + fit.ty).toBeCloseTo(size.h / 2, 6);
  });

  it("frames a multi-point bbox centred in the container", () => {
    const points = [
      { lat: 55.75, lng: 37.62 },
      { lat: 59.93, lng: 30.33 },
      { lat: 56.84, lng: 60.6 },
    ];
    const fit = fitToPoints(points, size);

    let minWx = Infinity;
    let minWy = Infinity;
    let maxWx = -Infinity;
    let maxWy = -Infinity;
    for (const p of points) {
      const [wx, wy] = project(p.lng, p.lat);
      minWx = Math.min(minWx, wx);
      minWy = Math.min(minWy, wy);
      maxWx = Math.max(maxWx, wx);
      maxWy = Math.max(maxWy, wy);
    }
    const centerWx = (minWx + maxWx) / 2;
    const centerWy = (minWy + maxWy) / 2;

    // The bbox centre maps to the container centre.
    expect(centerWx * fit.k + fit.tx).toBeCloseTo(size.w / 2, 6);
    expect(centerWy * fit.k + fit.ty).toBeCloseTo(size.h / 2, 6);

    // The whole bbox fits inside the container (with padding margin).
    expect(minWx * fit.k + fit.tx).toBeGreaterThanOrEqual(0);
    expect(maxWx * fit.k + fit.tx).toBeLessThanOrEqual(size.w);
    expect(minWy * fit.k + fit.ty).toBeGreaterThanOrEqual(0);
    expect(maxWy * fit.k + fit.ty).toBeLessThanOrEqual(size.h);
  });

  it("derives the zoom clamp from the fitted k", () => {
    const fit = fitToPoints([{ lat: 0, lng: 0 }], size);
    expect(fit.minK).toBeCloseTo(fit.k * MIN_K_FACTOR, 6);
    expect(fit.maxK).toBeCloseTo(fit.k * MAX_K_FACTOR, 6);
  });
  it.each([
    [{ lat: 0, lng: -170 }, { lat: 0, lng: 170 }],
    [{ lat: -80, lng: 0 }, { lat: 80, lng: 0 }],
  ])("fits collinear sites rather than using a single-site city zoom", (a, b) => {
    const fit = fitToPoints([a, b], size);
    for (const point of [a, b]) {
      const [x, y] = project(point.lng, point.lat);
      expect(x * fit.k + fit.tx).toBeGreaterThanOrEqual(24);
      expect(x * fit.k + fit.tx).toBeLessThanOrEqual(size.w - 24);
      expect(y * fit.k + fit.ty).toBeGreaterThanOrEqual(24);
      expect(y * fit.k + fit.ty).toBeLessThanOrEqual(size.h - 24);
    }
  });
});

describe("zoomAt", () => {
  it("keeps the world point under the cursor pixel-fixed", () => {
    const view = { k: 1000, tx: 50, ty: 80 };
    const cx = 300;
    const cy = 220;
    const next = zoomAt(view, 1.2, cx, cy, 1, 1e9);

    const wxBefore = (cx - view.tx) / view.k;
    const wyBefore = (cy - view.ty) / view.k;
    // After zoom, that same world point still projects to the cursor pixel.
    expect(wxBefore * next.k + next.tx).toBeCloseTo(cx, 6);
    expect(wyBefore * next.k + next.ty).toBeCloseTo(cy, 6);
  });

  it("clamps k to the supplied bounds", () => {
    const view = { k: 100, tx: 0, ty: 0 };
    expect(zoomAt(view, 100, 0, 0, 1, 200).k).toBe(200);
    expect(zoomAt(view, 0.001, 0, 0, 50, 1000).k).toBe(50);
  });
});

describe("clamp", () => {
  it("bounds a value to [min, max]", () => {
    expect(clamp(5, 0, 10)).toBe(5);
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
  });
});

describe("screenToLngLat", () => {
  it("inverts the screen transform back to lng/lat", () => {
    const view = { k: 4096, tx: 120, ty: -340 };
    const [lng, lat] = [37.62, 55.75];
    const [wx, wy] = project(lng, lat);
    const sx = wx * view.k + view.tx;
    const sy = wy * view.k + view.ty;
    const [rlng, rlat] = screenToLngLat(sx, sy, view);
    expect(rlng).toBeCloseTo(lng, 6);
    expect(rlat).toBeCloseTo(lat, 6);
  });
});
