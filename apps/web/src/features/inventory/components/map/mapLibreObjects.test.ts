import { describe, expect, it } from "vitest";

import {
  toBrowserAssetUrl,
  toPmtilesFetchUrl,
  toPmtilesProtocolUrl,
} from "@/features/inventory/components/map/detailedMapConfig";
import {
  boundsForPoints,
  buildInventoryFeatureCollection,
  countVisibleRelated,
  hasFiniteLngLat,
} from "@/features/inventory/components/map/mapLibreObjects";

describe("mapLibreObjects", () => {
  it("filters invalid coordinates before building GeoJSON features", () => {
    const collection = buildInventoryFeatureCollection(
      [
        {
          id: "valid",
          lat: 59.93,
          lng: 30.31,
          color: "#f6b419",
          strokeColor: "#2686d7",
          radius: 10,
          labelCount: 7,
        },
        { id: "bad-lat", lat: 95, lng: 30 },
        { id: "bad-lng", lat: 50, lng: 300 },
      ],
      "valid",
      undefined,
    );

    expect(collection.features).toHaveLength(1);
    expect(collection.features[0]?.properties).toMatchObject({
      id: "valid",
      color: "#f6b419",
      strokeColor: "#2686d7",
      radius: 10,
      count: 7,
      selected: true,
    });
    expect(collection.features[0]?.geometry.coordinates).toEqual([30.31, 59.93]);
  });

  it("counts only visible related layers", () => {
    expect(
      countVisibleRelated(
        { devices: 2, sites: 3 },
        new Set(["devices"]),
      ),
    ).toBe(2);
  });

  it("pads bounds for a single point", () => {
    const bounds = boundsForPoints([{ id: "spb", lat: 59.93, lng: 30.31 }]);
    expect(bounds?.[0]?.[0]).toBeCloseTo(29.91);
    expect(bounds?.[0]?.[1]).toBeCloseTo(59.53);
    expect(bounds?.[1]?.[0]).toBeCloseTo(30.71);
    expect(bounds?.[1]?.[1]).toBeCloseTo(60.33);
  });

  it("validates WGS-84 locations and projects the poles into the Mercator band", () => {
    expect(hasFiniteLngLat({ lat: 85.05, lng: 180 })).toBe(true);
    expect(hasFiniteLngLat({ lat: 90, lng: 0 })).toBe(true);
    expect(hasFiniteLngLat({ lat: 91, lng: 0 })).toBe(false);
    expect(hasFiniteLngLat({ lat: 0, lng: Number.NaN })).toBe(false);
    const pole = { id: "pole", lat: 90, lng: 180 };
    const collection = buildInventoryFeatureCollection([pole], null, undefined);
    expect(collection.features[0]?.geometry.coordinates).toEqual([180, 85.05112878]);
    expect(boundsForPoints([pole])?.[1]).toEqual([180, 85.05112878]);
  });
});

describe("toPmtilesProtocolUrl", () => {
  it("normalizes app-relative style assets to absolute browser URLs", () => {
    expect(toBrowserAssetUrl("/map-style/sprites/sprite", "http://localhost:9080")).toBe(
      "http://localhost:9080/map-style/sprites/sprite",
    );
  });

  it("preserves MapLibre glyph template tokens while normalizing asset URLs", () => {
    expect(
      toBrowserAssetUrl("/map-style/fonts/{fontstack}/{range}.pbf", "http://localhost:9080"),
    ).toBe("http://localhost:9080/map-style/fonts/{fontstack}/{range}.pbf");
  });

  it("keeps already normalized PMTiles URLs unchanged", () => {
    expect(toPmtilesProtocolUrl("pmtiles://https://example.com/world.pmtiles")).toBe(
      "pmtiles://https://example.com/world.pmtiles",
    );
  });

  it("normalizes app-relative PMTiles paths to absolute browser URLs", () => {
    expect(toPmtilesProtocolUrl("/maps/world.pmtiles", "http://localhost:9080")).toBe(
      "pmtiles://http://localhost:9080/maps/world.pmtiles",
    );
  });

  it("normalizes root-relative PMTiles protocol URLs to absolute browser URLs", () => {
    expect(toPmtilesProtocolUrl("pmtiles:///maps/world.pmtiles", "http://localhost:9080")).toBe(
      "pmtiles://http://localhost:9080/maps/world.pmtiles",
    );
  });

  it("returns the fetchable PMTiles URL without the custom protocol prefix", () => {
    expect(toPmtilesFetchUrl("pmtiles://https://example.com/world.pmtiles")).toBe(
      "https://example.com/world.pmtiles",
    );
  });
});
