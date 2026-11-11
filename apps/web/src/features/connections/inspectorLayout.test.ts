import { describe, expect, it } from "vitest";
import {
  clampInspectorWidth,
  INSPECTOR_DEFAULT_WIDTH,
  INSPECTOR_MAX_WIDTH,
  INSPECTOR_MIN_WIDTH,
} from "./inspectorLayout";

describe("inspector width bounds", () => {
  it("clamps below-range widths up to the minimum", () => {
    expect(clampInspectorWidth(120)).toBe(INSPECTOR_MIN_WIDTH);
  });

  it("clamps above-range widths down to the maximum", () => {
    expect(clampInspectorWidth(9999)).toBe(INSPECTOR_MAX_WIDTH);
  });

  it("keeps in-range widths and falls back for non-numbers", () => {
    expect(clampInspectorWidth(400)).toBe(400);
    expect(clampInspectorWidth(Number.NaN)).toBe(INSPECTOR_DEFAULT_WIDTH);
    expect(clampInspectorWidth("wide")).toBe(INSPECTOR_DEFAULT_WIDTH);
    expect(clampInspectorWidth(undefined)).toBe(INSPECTOR_DEFAULT_WIDTH);
  });
});
