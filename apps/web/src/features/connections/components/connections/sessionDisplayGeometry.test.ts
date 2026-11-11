import { describe, expect, it } from "vitest";
import { capDisplayResolution, fitDisplayScale } from "./sessionDisplayGeometry";

describe("session display geometry", () => {
  it("fits both axes without distorting the aspect ratio", () => {
    expect(fitDisplayScale({ width: 1600, height: 900 }, { width: 1024, height: 768 })).toBeCloseTo(
      900 / 768,
    );
    expect(fitDisplayScale({ width: 800, height: 600 }, { width: 1280, height: 720 })).toBeCloseTo(
      0.625,
    );
  });

  it("returns 1 for an unknown framebuffer", () => {
    expect(fitDisplayScale({ width: 800, height: 600 }, { width: 0, height: 0 })).toBe(1);
  });

  it("caps a large request while preserving its proportions", () => {
    expect(
      capDisplayResolution({ width: 1600, height: 900 }, { width: 1280, height: 720 }),
    ).toEqual({ width: 1280, height: 720 });
    expect(
      capDisplayResolution({ width: 1600, height: 1200 }, { width: 1280, height: 720 }),
    ).toEqual({ width: 960, height: 720 });
  });

  it("leaves a request within the cap untouched", () => {
    expect(
      capDisplayResolution({ width: 1024, height: 600 }, { width: 1280, height: 720 }),
    ).toEqual({ width: 1024, height: 600 });
  });
});
