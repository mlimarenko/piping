import { describe, expect, it } from "vitest";
import { computeRingArcs } from "@/features/inventory/components/map/markerRing";
import type { MapLayerId } from "@/features/inventory/components/map/mapLayers";

const RING_R = 11;
const visible = (...ids: MapLayerId[]) => new Set<MapLayerId>(ids);

describe("computeRingArcs", () => {
  it("returns no arcs when no visible related layer has a count", () => {
    expect(computeRingArcs({ sites: 1, devices: 0 }, visible("sites", "devices"), RING_R)).toEqual([]);
  });

  it("ignores the sites layer itself (it is the marker, not a related count)", () => {
    // Only `sites` is visible/counted → the marker has no related ring.
    expect(computeRingArcs({ sites: 1, devices: 5 }, visible("sites"), RING_R)).toEqual([]);
  });

  it("omits a layer that is hidden by the legend", () => {
    const arcs = computeRingArcs({ sites: 1, devices: 5 }, visible("sites"), RING_R);
    expect(arcs).toHaveLength(0);
  });

  it("emits one arc per visible non-sites layer with a positive count", () => {
    const arcs = computeRingArcs({ sites: 1, devices: 5 }, visible("sites", "devices"), RING_R);
    expect(arcs).toHaveLength(1);
    expect(arcs[0]?.layerId).toBe("devices");
    // Single contributing layer → segment spans (nearly) the whole circumference.
    const circumference = 2 * Math.PI * RING_R;
    const [segLen] = (arcs[0]?.dashArray ?? "0 0").split(" ").map(Number);
    expect(segLen).toBeGreaterThan(0);
    expect(segLen).toBeLessThanOrEqual(circumference);
  });

  it("keeps the dasharray segment + gap within one circumference", () => {
    const arcs = computeRingArcs({ devices: 7 }, visible("devices"), RING_R);
    const circumference = 2 * Math.PI * RING_R;
    for (const arc of arcs) {
      const parts = arc.dashArray.split(" ").map(Number);
      const sum = parts.reduce((a, b) => a + b, 0);
      expect(sum).toBeCloseTo(circumference, 6);
    }
  });
});
