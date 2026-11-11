import { describe, expect, it } from "vitest";
import {
  basemap,
  buildBasemapPaths,
} from "@/features/inventory/components/map/basemap";

describe("basemap data", () => {
  it("bundles a non-empty FeatureCollection of polygons", () => {
    expect(basemap.type).toBe("FeatureCollection");
    expect(basemap.features.length).toBeGreaterThan(0);
    for (const feature of basemap.features) {
      expect(feature.properties.name).toBeTruthy();
      expect(["Polygon", "MultiPolygon"]).toContain(feature.geometry.type);
    }
  });

  it("is a worldwide basemap (not region-locked)", () => {
    const names = basemap.features.map((f) => f.properties.name);
    // Worldwide so the map works for sites on any continent — not one region.
    expect(basemap.features.length).toBeGreaterThan(100);
    expect(names).toContain("Russia");
    expect(names).toContain("China");
    expect(names).toContain("Brazil");
  });

  it("carries both country and subnational layers on its features", () => {
    const layers = new Set(basemap.features.map((f) => f.properties.layer));
    expect(layers.has("country")).toBe(true);
    expect(layers.has("subnational")).toBe(true);
    // Every feature is tagged with exactly one of the two render groups.
    for (const feature of basemap.features) {
      expect(["country", "subnational"]).toContain(feature.properties.layer);
    }
  });
});

describe("buildBasemapPaths", () => {
  it("emits one entry per feature, each a non-empty closed world-coord path", () => {
    const paths = buildBasemapPaths();
    expect(paths.length).toBe(basemap.features.length);
    for (const path of paths) {
      expect(path.name).toBeTruthy();
      // Every feature draws: a non-empty `d` that starts with a moveto and
      // closes with Z (so the country renders as a filled region, not a sliver).
      expect(path.d.length).toBeGreaterThan(0);
      expect(path.d.startsWith("M")).toBe(true);
      expect(path.d.endsWith("Z")).toBe(true);
    }
  });

  it("carries each feature's layer through onto its path datum", () => {
    const paths = buildBasemapPaths();
    const kinds = new Set(paths.map((p) => p.layer));
    expect(kinds.has("country")).toBe(true);
    expect(kinds.has("subnational")).toBe(true);
    // The layer on the path datum matches the source feature index-for-index.
    paths.forEach((path, i) => {
      expect(path.layer).toBe(basemap.features[i]?.properties.layer);
    });
  });

  it(
    "keeps all coordinates inside the [0,1] world square",
    () => {
      const paths = buildBasemapPaths();
      const numbers = paths
        .flatMap((p) => p.d.replace(/[MLZ]/g, " ").trim().split(/\s+/))
        .filter((token) => token !== "")
        .map(Number);
      expect(numbers.length).toBeGreaterThan(0);
      for (const value of numbers) {
        expect(Number.isFinite(value)).toBe(true);
        expect(value).toBeGreaterThanOrEqual(0);
        expect(value).toBeLessThanOrEqual(1);
      }
    },
    // The full basemap GeoJSON yields a very large coordinate array; the
    // default 5s vitest timeout is too tight for slower CI runners (~6s
    // wall-clock locally), but the assertion logic itself is O(n) so a
    // generous bump is safe.
    15_000,
  );

  it("concatenates a sub-path per ring for a MultiPolygon feature", () => {
    const multi = basemap.features.find((f) => f.geometry.type === "MultiPolygon");
    expect(multi).toBeDefined();
    if (!multi || multi.geometry.type !== "MultiPolygon") return;
    const ringCount = multi.geometry.coordinates.reduce(
      (sum, polygon) => sum + polygon.length,
      0,
    );
    const [built] = buildBasemapPaths([multi]);
    // One "M…Z" sub-path per ring → the moveto count equals the ring count.
    const moveCount = (built?.d.match(/M/g) ?? []).length;
    expect(moveCount).toBe(ringCount);
  });
});
