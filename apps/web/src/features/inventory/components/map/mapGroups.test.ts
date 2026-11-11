import { describe, expect, it } from "vitest";
import { canSeparateGroup, groupMapPoints } from "./mapGroups";
import { invert } from "./mapProjection";

describe("map groups", () => {
  it("groups by screen distance and separates when zoom increases", () => {
    const points = [{ id: "a", lat: 0, lng: 0, labelCount: 500 }, { id: "b", lat: 0, lng: 1, labelCount: 7 }, { id: "c", lat: 20, lng: 20 }];
    const groups = groupMapPoints(points, 4096, "unwrapped");
    expect(groups.map(group => group.members.length)).toEqual([2, 1]);
    expect(groups[0]?.point.labelCount).toBe(2);
    expect(groupMapPoints(points, 65536, "unwrapped")).toHaveLength(3);
  });

  it("identifies points that cannot separate at the available zoom", () => {
    const points = [{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 0, lng: 0 }];
    expect(canSeparateGroup(points, 1e9, "unwrapped")).toBe(false);
    expect(groupMapPoints([...points, { id: "invalid", lat: 91, lng: 0 }], 4096, "unwrapped")).toHaveLength(1);
  });

  it("keeps nearby 44px targets and selection rings in one group", () => {
    const scale = 3600;
    expect(groupMapPoints([{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 0, lng: 6 }], scale, "unwrapped")).toHaveLength(1);
    expect(groupMapPoints([{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 0, lng: 8 }], scale, "unwrapped")).toHaveLength(2);
  });

  it("merges a cluster whose displayed centroid would cover another marker", () => {
    const scale = 36000;
    const ring = Array.from({ length: 8 }, (_, index) => {
      const angle = index * Math.PI / 4;
      const [lng, lat] = invert(0.5 + 80 * Math.cos(angle) / scale, 0.5 + 80 * Math.sin(angle) / scale);
      return { id: String(index), lng, lat };
    });
    expect(groupMapPoints([...ring, { id: "center", lng: 0, lat: 0 }], scale, "unwrapped")).toHaveLength(1);
  });

  it("groups nearby dateline points only for the wrapped world and centers them at the seam", () => {
    const points = [{ id: "east", lat: 0, lng: 179.99 }, { id: "west", lat: 0, lng: -179.99 }];
    for (const order of [points, [...points].reverse()]) {
      const groups = groupMapPoints(order, 512 * 2 ** 8, "wrapped");
      expect(groups).toHaveLength(1);
      expect(Math.abs(groups[0]!.point.lng)).toBeCloseTo(180, 5);
      expect(canSeparateGroup(order, 512 * 2 ** 22, "wrapped")).toBe(true);
    }
    expect(groupMapPoints(points, 512 * 2 ** 8, "unwrapped")).toHaveLength(2);
  });

  it("recognizes opposite representations of the seam as co-located", () => {
    const points = [{ id: "east", lat: 0, lng: 180 }, { id: "west", lat: 0, lng: -180 }];
    expect(groupMapPoints(points, 512 * 2 ** 22, "wrapped")).toHaveLength(1);
    expect(canSeparateGroup(points, 512 * 2 ** 22, "wrapped")).toBe(false);
    expect(canSeparateGroup(points, 512 * 2 ** 22, "unwrapped")).toBe(true);
  });

  it("merges a wrapped cluster whose center covers a separate marker at the seam", () => {
    const scale = 36000;
    const ring = Array.from({ length: 8 }, (_, index) => {
      const angle = index * Math.PI / 4;
      const x = (1 + 80 * Math.cos(angle) / scale) % 1;
      const [lng, lat] = invert(x, 0.5 + 80 * Math.sin(angle) / scale);
      return { id: String(index), lng, lat };
    });
    const groups = groupMapPoints([...ring, { id: "center", lat: 0, lng: 180 }], scale, "wrapped");
    expect(groups).toHaveLength(1);
    expect(Math.abs(groups[0]!.point.lng)).toBeCloseTo(180, 5);
  });
});
