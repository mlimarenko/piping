import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clampObservationPreviewHeight,
  clampSessionInspectorWidth,
  defaultSessionInspectorLayout,
  loadSessionInspectorLayout,
  normalizeSessionInspectorLayout,
  observationPreviewMaxHeight,
  saveSessionInspectorLayout,
  sessionInspectorMaxWidth,
  OBSERVATION_PREVIEW_DEFAULT_HEIGHT,
  OBSERVATION_PREVIEW_MIN_HEIGHT,
  SESSION_INSPECTOR_DEFAULT_WIDTH,
  SESSION_INSPECTOR_MIN_WIDTH,
} from "./sessionInspectorLayout";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("session inspector layout", () => {
  it("keeps the panel width inside the readable band", () => {
    expect(clampSessionInspectorWidth(10)).toBe(SESSION_INSPECTOR_MIN_WIDTH);
    expect(clampSessionInspectorWidth(Number.NaN)).toBe(SESSION_INSPECTOR_DEFAULT_WIDTH);
    expect(clampSessionInspectorWidth(10_000)).toBe(sessionInspectorMaxWidth());
    expect(clampSessionInspectorWidth(456)).toBe(456);
  });

  it("keeps the observation preview above the minimum and inside the viewport", () => {
    expect(clampObservationPreviewHeight(10)).toBe(OBSERVATION_PREVIEW_MIN_HEIGHT);
    expect(clampObservationPreviewHeight(Number.NaN)).toBe(OBSERVATION_PREVIEW_DEFAULT_HEIGHT);
    expect(clampObservationPreviewHeight(10_000)).toBe(observationPreviewMaxHeight());
    expect(clampObservationPreviewHeight(420)).toBe(420);
  });

  it("persists both sizes and falls back to defaults for malformed storage", () => {
    saveSessionInspectorLayout({ width: 512, previewHeight: 480 });
    expect(loadSessionInspectorLayout()).toEqual({ width: 512, previewHeight: 480 });
    expect(normalizeSessionInspectorLayout({ width: "wide", previewHeight: null }))
      .toEqual(defaultSessionInspectorLayout());
  });
});
