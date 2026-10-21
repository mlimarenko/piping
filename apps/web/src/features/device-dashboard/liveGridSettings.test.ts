import { beforeEach, describe, expect, it } from "vitest";

import {
  defaultCells,
  defaultSettings,
  loadLiveViewSettings,
  resizeCells,
  saveLiveViewSettings,
} from "./liveGridSettings";

describe("liveGridSettings", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("round-trips settings through localStorage per target", () => {
    const settings = {
      mode: "grid" as const,
      source: "stream" as const,
      layout: 3 as const,
      cells: [4, 5, 6, 1, 2, 3, 7, 8, 9],
    };
    saveLiveViewSettings("target-a", settings);
    expect(loadLiveViewSettings("target-a")).toEqual(settings);
    expect(loadLiveViewSettings("target-b")).toEqual(defaultSettings());
  });

  it("falls back to defaults on corrupt or foreign JSON", () => {
    window.localStorage.setItem("cctv:live:target-a", "{not json");
    expect(loadLiveViewSettings("target-a")).toEqual(defaultSettings());
    window.localStorage.setItem("cctv:live:target-a", JSON.stringify({ layout: 99, cells: "x" }));
    const restored = loadLiveViewSettings("target-a");
    expect(restored.layout).toBe(2);
    expect(restored.cells).toEqual(defaultCells(2));
  });

  it("resizeCells keeps what fits and extends with row-major defaults", () => {
    expect(resizeCells([7, 8, 9, 10, 11, 12, 13, 14, 15], 2)).toEqual([7, 8, 9, 10]);
    expect(resizeCells([7, 8], 2)).toEqual([7, 8, 3, 4]);
  });

  it("drops non-channel garbage from stored cells", () => {
    window.localStorage.setItem(
      "cctv:live:target-a",
      JSON.stringify({ mode: "grid", layout: 2, cells: [3, -1, "x", 2.5, 4] }),
    );
    const restored = loadLiveViewSettings("target-a");
    expect(restored.cells).toEqual([3, 4, 3, 4]);
  });
});
