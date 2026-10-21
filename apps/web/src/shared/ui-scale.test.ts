import { describe, expect, it } from "vitest";
import {
  applyUiScale,
  DEFAULT_UI_SCALE,
  MAX_UI_SCALE,
  MIN_UI_SCALE,
  percentToUiScale,
  readAppliedUiScale,
  sanitizeUiScale,
  uiScaleToPercent,
} from "@/shared/ui-scale";

describe("ui scale helpers", () => {
  it("falls back to default for invalid values", () => {
    expect(sanitizeUiScale(undefined)).toBe(DEFAULT_UI_SCALE);
    expect(sanitizeUiScale(Number.NaN)).toBe(DEFAULT_UI_SCALE);
    expect(sanitizeUiScale("not-a-number")).toBe(DEFAULT_UI_SCALE);
  });

  it("clamps supported scale values", () => {
    expect(sanitizeUiScale(0.5)).toBe(MIN_UI_SCALE);
    expect(sanitizeUiScale(2)).toBe(MAX_UI_SCALE);
  });

  it("rounds to stable five percent increments", () => {
    expect(sanitizeUiScale(1.13)).toBe(1.15);
    expect(sanitizeUiScale(1.27)).toBe(1.25);
  });

  it("converts between scale and percent", () => {
    expect(uiScaleToPercent(1.25)).toBe(125);
    expect(percentToUiScale(140)).toBe(1.4);
  });

  it("scales the root once and keeps body text at the shared 14px rem baseline", () => {
    const root = document.documentElement;
    const body = document.body;
    // Model the stylesheet's rem baseline; applying a preference must not replace it.
    body.style.fontSize = "0.875rem";

    applyUiScale(1.25, root);

    expect(root.style.fontSize).toBe("20px");
    expect(root.dataset.uiScale).toBe("1.25");
    expect(body.style.fontSize).toBe("0.875rem");
    expect(readAppliedUiScale(root)).toBe(1.25);
    applyUiScale(1.5, root);
    expect(root.style.fontSize).toBe("24px");
    expect(body.style.fontSize).toBe("0.875rem");
    root.style.removeProperty("font-size");
    delete root.dataset.uiScale;
    body.style.removeProperty("font-size");
  });
});
