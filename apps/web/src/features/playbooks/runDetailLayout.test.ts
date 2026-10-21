import { describe, expect, it } from "vitest";

import {
  clampRunDetailWidth,
  defaultRunDetailWidth,
  isWideRunsLayout,
  maxRunDetailWidth,
  resolveRunDetailWidth,
  RUNS_WIDE_MODE_MIN_WIDTH,
} from "@/features/playbooks/runDetailLayout";

describe("runDetailLayout", () => {
  it("switches to wide mode at the 900px threshold", () => {
    expect(isWideRunsLayout(899)).toBe(false);
    expect(isWideRunsLayout(900)).toBe(true);
    expect(isWideRunsLayout(RUNS_WIDE_MODE_MIN_WIDTH)).toBe(true);
  });

  it("keeps the list at least 380px via maxRunDetailWidth", () => {
    expect(maxRunDetailWidth(900)).toBe(520);
    expect(maxRunDetailWidth(760)).toBe(380);
    expect(maxRunDetailWidth(500)).toBe(380);
  });

  it("clamps a stored width to the resize bounds", () => {
    expect(clampRunDetailWidth(200, 1600)).toBe(380);
    expect(clampRunDetailWidth(5000, 1600)).toBe(1220);
    expect(clampRunDetailWidth(800, 1600)).toBe(800);
  });

  it("computes the default width as half the container, clamped to [560, max]", () => {
    expect(defaultRunDetailWidth(1600)).toBe(800);
    // round(1000 * 0.5) = 500 < 560 -> floors at the 560 preference.
    expect(defaultRunDetailWidth(1000)).toBe(560);
    // max = 920 - 380 = 540 < 560 -> capped by the hard max instead.
    expect(defaultRunDetailWidth(920)).toBe(540);
  });

  it("resolves an unset (<= 0) stored width to the computed default", () => {
    expect(resolveRunDetailWidth(0, 1600)).toBe(defaultRunDetailWidth(1600));
    expect(resolveRunDetailWidth(-1, 1600)).toBe(defaultRunDetailWidth(1600));
    expect(resolveRunDetailWidth(Number.NaN, 1600)).toBe(defaultRunDetailWidth(1600));
    expect(resolveRunDetailWidth(700, 1600)).toBe(700);
  });
});
